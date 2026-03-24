// SPDX-License-Identifier: MPL-2.0

use core::time::Duration;

use aster_block::bio::BioWaiter;
use aster_virtio::device::transport9p::protocol::{
    P9Qid, P9_GETATTR_ALL,
    P9_SETATTR_ATIME, P9_SETATTR_ATIME_SET,
    P9_SETATTR_GID, P9_SETATTR_MODE, P9_SETATTR_MTIME, P9_SETATTR_MTIME_SET,
    P9_SETATTR_SIZE, P9_SETATTR_UID, AT_REMOVEDIR,
};
use super::{
    O_RDONLY, O_RDWR, O_WRONLY, P9_READDIR_BUF_SIZE,
    S_IFBLK, S_IFCHR, S_IFDIR, S_IFIFO, S_IFREG, S_IFSOCK,
    Virtio9P, handle::Virtio9PHandle, inode_type_from_dirent_type, p9_attr_to_metadata,
};
use crate::{
    fs::{
        inode_handle::FileIo,
        utils::{
            AccessMode, CachePage, DirentVisitor, Extension, FileSystem, Inode, InodeIo, InodeMode,
            InodeType, Metadata, MknodType, PageCache, PageCacheBackend, StatusFlags, SymbolicLink,
        },
    },
    prelude::*,
    process::{Gid, Uid},
    vm::vmo::Vmo,
};

const PAGE_SIZE: usize = 4096;

pub(super) struct Virtio9PInode {
    this: Weak<Virtio9PInode>,
    fid: u32,
    qid: RwLock<P9Qid>,
    metadata: RwLock<Metadata>,
    page_cache: Option<Mutex<PageCache>>,
    /// A persistent open FID for page cache I/O.
    page_cache_fid: Mutex<Option<u32>>,
    fs: Weak<Virtio9P>,
    extension: Extension,
}

impl Virtio9PInode {
    pub(super) fn new(
        fid: u32,
        qid: P9Qid,
        metadata: Metadata,
        fs: Weak<Virtio9P>,
    ) -> Arc<Self> {
        Arc::new_cyclic(|weak_self| Self {
            this: weak_self.clone(),
            fid,
            qid: RwLock::new(qid),
            metadata: RwLock::new(metadata),
            page_cache: metadata.type_.is_regular_file().then(|| {
                Mutex::new(PageCache::with_capacity(metadata.size, weak_self.clone() as _).unwrap())
            }),
            page_cache_fid: Mutex::new(None),
            fs,
            extension: Extension::new(),
        })
    }

    pub(super) fn fs_ref(&self) -> Arc<Virtio9P> {
        self.fs.upgrade().unwrap()
    }

    pub(super) fn try_fs_ref(&self) -> Option<Arc<Virtio9P>> {
        self.fs.upgrade()
    }

    pub(super) fn fid(&self) -> u32 {
        self.fid
    }

    pub(super) fn revalidate_attr(&self) -> Result<()> {
        let fs = self.fs_ref();
        let attr = fs
            .fid_mgr
            .device()
            .p9_getattr(self.fid, P9_GETATTR_ALL)
            .map_err(|_| Error::with_message(Errno::EIO, "9p getattr failed"))?;

        let old_metadata = self.metadata();
        let new_metadata = p9_attr_to_metadata(&attr);
        if old_metadata.mtime != new_metadata.mtime {
            self.invalidate_page_cache(new_metadata.size)?;
        }
        *self.metadata.write() = new_metadata;
        *self.qid.write() = attr.qid;

        Ok(())
    }

    fn invalidate_page_cache(&self, new_size: usize) -> Result<()> {
        let Some(page_cache) = &self.page_cache else {
            return Ok(());
        };

        let page_cache = &mut page_cache.lock();
        let cached_size = page_cache.pages().size();
        if cached_size > 0 {
            page_cache.discard_range(0..cached_size);
            page_cache.resize(0)?;
        }
        page_cache.resize(new_size)?;

        Ok(())
    }

    pub(super) fn flush_page_cache(&self) -> Result<()> {
        let Some(page_cache) = &self.page_cache else {
            return Ok(());
        };

        page_cache.lock().evict_range(0..self.size())?;
        Ok(())
    }

    fn get_or_open_page_cache_fid(&self) -> Result<u32> {
        let mut fid_slot = self.page_cache_fid.lock();
        if let Some(fid) = *fid_slot {
            return Ok(fid);
        }

        let fs = self.fs_ref();
        let open_fid = fs
            .fid_mgr
            .clone_fid(self.fid)
            .map_err(|_| Error::with_message(Errno::EIO, "9p clone fid for page cache failed"))?;

        fs.fid_mgr
            .device()
            .p9_lopen(open_fid, O_RDWR)
            .map_err(|_| {
                fs.fid_mgr.clunk(open_fid);
                Error::with_message(Errno::EIO, "9p lopen for page cache failed")
            })?;

        *fid_slot = Some(open_fid);
        Ok(open_fid)
    }

    fn release_page_cache_fid(&self) {
        let Some(fs) = self.fs.upgrade() else {
            return;
        };

        if let Some(fid) = self.page_cache_fid.lock().take() {
            fs.fid_mgr.clunk(fid);
        }
    }

    pub(super) fn cached_read_at(&self, offset: usize, writer: &mut VmWriter) -> Result<usize> {
        self.revalidate_attr()?;

        let Some(page_cache) = &self.page_cache else {
            return self.direct_read_at(offset, writer);
        };

        let file_size = self.size();
        let start = file_size.min(offset);
        let end = file_size.min(offset + writer.avail());
        let read_len = end - start;
        page_cache.lock().pages().read(start, writer)?;
        Ok(read_len)
    }

    pub(super) fn direct_read_at(&self, offset: usize, writer: &mut VmWriter) -> Result<usize> {
        self.revalidate_attr()?;

        let file_size = self.size();
        let start = file_size.min(offset);
        let end = file_size.min(offset + writer.avail());
        let read_len = end - start;
        if read_len == 0 {
            return Ok(0);
        }

        let fs = self.fs_ref();
        let read_fid = fs.fid_mgr.clone_fid(self.fid).map_err(Error::from)?;

        let result = (|| -> Result<usize> {
            fs.fid_mgr
                .device()
                .p9_lopen(read_fid, O_RDONLY)
                .map_err(|_| Error::with_message(Errno::EIO, "9p lopen for read failed"))?;

            let max_len = read_len.min(u32::MAX as usize) as u32;
            let data = fs
                .fid_mgr
                .device()
                .p9_read(read_fid, start as u64, max_len)
                .map_err(|_| Error::with_message(Errno::EIO, "9p read failed"))?;

            let mut reader = VmReader::from(data.as_slice());
            writer.write_fallible(&mut reader)?;
            Ok(read_len.min(data.len()))
        })();

        fs.fid_mgr.clunk(read_fid);
        result
    }

    pub(super) fn cached_write_at(&self, offset: usize, reader: &mut VmReader) -> Result<usize> {
        let Some(page_cache) = &self.page_cache else {
            return self.direct_write_at(offset, reader);
        };
        let page_cache = page_cache.lock();

        let write_len = reader.remain();
        let new_size = offset + write_len;
        if new_size > page_cache.pages().size() {
            page_cache.resize(new_size)?;
        }
        {
            let mut metadata = self.metadata.write();
            metadata.size = metadata.size.max(new_size);
            metadata.blocks = metadata.size.div_ceil(metadata.blk_size.max(1));
        }
        page_cache.pages().write(offset, reader)?;

        Ok(write_len)
    }

    pub(super) fn direct_write_at(&self, offset: usize, reader: &mut VmReader) -> Result<usize> {
        let write_len = reader.remain().min(u32::MAX as usize);
        let mut data = vec![0u8; write_len];
        reader.read_fallible(&mut VmWriter::from(data.as_mut_slice()))?;

        let fs = self.fs_ref();
        let write_fid = fs.fid_mgr.clone_fid(self.fid).map_err(Error::from)?;

        let result = (|| -> Result<usize> {
            fs.fid_mgr
                .device()
                .p9_lopen(write_fid, O_WRONLY)
                .map_err(|_| Error::with_message(Errno::EIO, "9p lopen for write failed"))?;

            let written = fs
                .fid_mgr
                .device()
                .p9_write(write_fid, offset as u64, &data)
                .map_err(|_| Error::with_message(Errno::EIO, "9p write failed"))? as usize;

            let new_size = offset + written;
            {
                let mut metadata = self.metadata.write();
                metadata.size = metadata.size.max(new_size);
                metadata.blocks = metadata.size.div_ceil(metadata.blk_size.max(1));
            }

            self.invalidate_page_cache(self.size())?;
            Ok(written)
        })();

        fs.fid_mgr.clunk(write_fid);
        result
    }

    fn open_handle(&self, access_mode: AccessMode) -> Result<Virtio9PHandle> {
        let flags = match access_mode {
            AccessMode::O_RDONLY => O_RDONLY,
            AccessMode::O_WRONLY => O_WRONLY,
            AccessMode::O_RDWR => O_RDWR,
        };

        let fs = self.fs_ref();
        let open_fid = fs.fid_mgr.clone_fid(self.fid).map_err(Error::from)?;

        match fs.fid_mgr.device().p9_lopen(open_fid, flags) {
            Ok(_) => {}
            Err(_) => {
                fs.fid_mgr.clunk(open_fid);
                return_errno_with_message!(Errno::EIO, "9p lopen failed");
            }
        }

        let Some(inode) = self.this.upgrade() else {
            fs.fid_mgr.clunk(open_fid);
            return_errno_with_message!(Errno::EIO, "9p inode is unavailable");
        };

        let cache_enabled = self.page_cache.is_some();
        Ok(Virtio9PHandle::new(inode, open_fid, flags, cache_enabled))
    }

    fn apply_setattr(&self, valid: u32, mode: u32, uid: u32, gid: u32, size: u64,
                      atime_sec: u64, atime_nsec: u64, mtime_sec: u64, mtime_nsec: u64) -> Result<()> {
        let fs = self.fs_ref();
        fs.fid_mgr
            .device()
            .p9_setattr(
                self.fid, valid, mode, uid, gid, size,
                atime_sec, atime_nsec, mtime_sec, mtime_nsec,
            )
            .map_err(Error::from)?;

        // Re-fetch attributes after setattr.
        self.revalidate_attr()?;
        Ok(())
    }
}

impl Drop for Virtio9PInode {
    fn drop(&mut self) {
        self.release_page_cache_fid();
        // Don't clunk the root fid — it's managed by the filesystem.
        let Some(fs) = self.fs.upgrade() else {
            return;
        };
        if self.fid != fs.root_fid {
            fs.fid_mgr.clunk(self.fid);
        }
    }
}

impl PageCacheBackend for Virtio9PInode {
    fn read_page_async(&self, idx: usize, frame: &CachePage) -> Result<BioWaiter> {
        let offset = idx * PAGE_SIZE;
        if offset >= self.size() {
            return_errno_with_message!(Errno::EINVAL, "9p page read beyond EOF");
        }

        frame.writer().fill_zeros(frame.size());
        let size = (self.size() - offset).min(PAGE_SIZE).min(u32::MAX as usize) as u32;
        let fs = self.fs_ref();
        let fh = self.get_or_open_page_cache_fid()?;
        let data = fs
            .fid_mgr
            .device()
            .p9_read(fh, offset as u64, size)
            .map_err(|_| Error::with_message(Errno::EIO, "9p page read failed"))?;
        let mut frame_writer = frame.writer();
        frame_writer.write_fallible(&mut VmReader::from(data.as_slice()).to_fallible())?;
        Ok(BioWaiter::new())
    }

    fn write_page_async(&self, idx: usize, frame: &CachePage) -> Result<BioWaiter> {
        let offset = idx * PAGE_SIZE;
        let file_size = self.size();
        if offset >= file_size {
            return Ok(BioWaiter::new());
        }

        let write_len = (file_size - offset).min(PAGE_SIZE);
        let mut data = vec![0u8; write_len];
        let mut writer = VmWriter::from(data.as_mut_slice());
        writer.write_fallible(&mut frame.reader().to_fallible())?;

        let fs = self.fs_ref();
        let fh = self.get_or_open_page_cache_fid()?;
        fs.fid_mgr
            .device()
            .p9_write(fh, offset as u64, &data)
            .map_err(|_| Error::with_message(Errno::EIO, "9p page write failed"))?;
        Ok(BioWaiter::new())
    }

    fn npages(&self) -> usize {
        self.size().div_ceil(PAGE_SIZE)
    }
}

impl InodeIo for Virtio9PInode {
    fn read_at(
        &self,
        offset: usize,
        writer: &mut VmWriter,
        _status_flags: StatusFlags,
    ) -> Result<usize> {
        let metadata = self.metadata();
        if metadata.type_ != InodeType::File {
            return_errno_with_message!(Errno::EISDIR, "inode is not a regular file");
        }

        self.cached_read_at(offset, writer)
    }

    fn write_at(
        &self,
        offset: usize,
        reader: &mut VmReader,
        status_flags: StatusFlags,
    ) -> Result<usize> {
        let metadata = self.metadata();
        if metadata.type_ != InodeType::File {
            return_errno_with_message!(Errno::EISDIR, "inode is not a regular file");
        }

        let offset = if status_flags.contains(StatusFlags::O_APPEND) {
            self.revalidate_attr()?;
            self.size()
        } else {
            offset
        };

        self.cached_write_at(offset, reader)
    }
}

impl Inode for Virtio9PInode {
    fn size(&self) -> usize {
        self.metadata.read().size
    }

    fn resize(&self, new_size: usize) -> Result<()> {
        if self.metadata.read().type_ != InodeType::File {
            return_errno_with_message!(Errno::EISDIR, "resize on non-regular file");
        }

        let size = u64::try_from(new_size)
            .map_err(|_| Error::with_message(Errno::EFBIG, "9p resize size too large"))?;

        self.apply_setattr(P9_SETATTR_SIZE, 0, 0, 0, size, 0, 0, 0, 0)
    }

    fn metadata(&self) -> Metadata {
        *self.metadata.read()
    }

    fn ino(&self) -> u64 {
        self.qid.read().path
    }

    fn type_(&self) -> InodeType {
        self.metadata.read().type_
    }

    fn mode(&self) -> Result<InodeMode> {
        Ok(self.metadata.read().mode)
    }

    fn set_mode(&self, mode: InodeMode) -> Result<()> {
        let mode_bits = (self.type_() as u32) | u32::from(mode.bits());
        self.apply_setattr(P9_SETATTR_MODE, mode_bits, 0, 0, 0, 0, 0, 0, 0)
    }

    fn owner(&self) -> Result<Uid> {
        Ok(self.metadata.read().uid)
    }

    fn set_owner(&self, uid: Uid) -> Result<()> {
        self.apply_setattr(P9_SETATTR_UID, 0, uid.into(), 0, 0, 0, 0, 0, 0)
    }

    fn group(&self) -> Result<Gid> {
        Ok(self.metadata.read().gid)
    }

    fn set_group(&self, gid: Gid) -> Result<()> {
        self.apply_setattr(P9_SETATTR_GID, 0, 0, gid.into(), 0, 0, 0, 0, 0)
    }

    fn atime(&self) -> Duration {
        self.metadata.read().atime
    }

    fn set_atime(&self, time: Duration) {
        if let Err(err) = self.apply_setattr(
            P9_SETATTR_ATIME | P9_SETATTR_ATIME_SET,
            0, 0, 0, 0,
            time.as_secs(), time.subsec_nanos() as u64, 0, 0,
        ) {
            warn!("9p set_atime failed for fid {}: {:?}", self.fid, err);
        }
    }

    fn mtime(&self) -> Duration {
        self.metadata.read().mtime
    }

    fn set_mtime(&self, time: Duration) {
        if let Err(err) = self.apply_setattr(
            P9_SETATTR_MTIME | P9_SETATTR_MTIME_SET,
            0, 0, 0, 0,
            0, 0, time.as_secs(), time.subsec_nanos() as u64,
        ) {
            warn!("9p set_mtime failed for fid {}: {:?}", self.fid, err);
        }
    }

    fn ctime(&self) -> Duration {
        self.metadata.read().ctime
    }

    fn set_ctime(&self, _time: Duration) {
        // ctime is automatically updated by the server; ignore client requests.
    }

    fn page_cache(&self) -> Option<Arc<Vmo>> {
        self.page_cache
            .as_ref()
            .map(|page_cache| page_cache.lock().pages().clone())
    }

    fn open(
        &self,
        access_mode: AccessMode,
        _status_flags: StatusFlags,
    ) -> Option<Result<Box<dyn FileIo>>> {
        if self.metadata.read().type_ != InodeType::File {
            return None;
        }
        Some(
            self.open_handle(access_mode)
                .map(|handle| Box::new(handle) as Box<dyn FileIo>),
        )
    }

    fn lookup(&self, name: &str) -> Result<Arc<dyn Inode>> {
        let metadata = self.metadata();
        if metadata.type_ != InodeType::Dir {
            return_errno_with_message!(Errno::ENOTDIR, "lookup on non-directory");
        }

        let fs = self.fs_ref();
        let (newfid, qids) = fs.fid_mgr.walk(self.fid, &[name]).map_err(Error::from)?;

        if qids.is_empty() {
            fs.fid_mgr.clunk(newfid);
            return_errno_with_message!(Errno::ENOENT, "9p walk returned no qids");
        }

        let qid = qids[0];
        let attr = fs
            .fid_mgr
            .device()
            .p9_getattr(newfid, P9_GETATTR_ALL)
            .map_err(|e| {
                fs.fid_mgr.clunk(newfid);
                Error::from(e)
            })?;

        let child_metadata = p9_attr_to_metadata(&attr);
        let inode = Virtio9PInode::new(newfid, qid, child_metadata, Arc::downgrade(&fs));

        Ok(inode)
    }

    fn create(&self, name: &str, type_: InodeType, mode: InodeMode) -> Result<Arc<dyn Inode>> {
        let metadata = self.metadata();
        if metadata.type_ != InodeType::Dir {
            return_errno_with_message!(Errno::ENOTDIR, "create on non-directory");
        }

        let fs = self.fs_ref();
        let device = fs.fid_mgr.device();
        let mode_bits = mode.bits() as u32;

        match type_ {
            InodeType::File => {
                // Walk to get a new fid for the parent, then lcreate on it.
                let create_fid = fs.fid_mgr.clone_fid(self.fid).map_err(Error::from)?;
                match device.p9_lcreate(create_fid, name, O_RDWR, S_IFREG | mode_bits, 0) {
                    Ok(_) => {
                        // lcreate changes create_fid to point to the new file.
                        // Clunk it since we'll walk to the file separately.
                        fs.fid_mgr.clunk(create_fid);
                    }
                    Err(e) => {
                        fs.fid_mgr.clunk(create_fid);
                        return Err(Error::from(e));
                    }
                }
            }
            InodeType::Dir => {
                device
                    .p9_mkdir(self.fid, name, S_IFDIR | mode_bits, 0)
                    .map_err(Error::from)?;
            }
            InodeType::Socket => {
                device
                    .p9_mknod(self.fid, name, S_IFSOCK | mode_bits, 0, 0, 0)
                    .map_err(Error::from)?;
            }
            _ => {
                return_errno_with_message!(
                    Errno::EOPNOTSUPP,
                    "9p create supports file/dir/socket only"
                );
            }
        }

        // Walk to the newly created entry to get a fresh fid.
        self.lookup(name)
    }

    fn mknod(&self, name: &str, mode: InodeMode, type_: MknodType) -> Result<Arc<dyn Inode>> {
        let metadata = self.metadata();
        if metadata.type_ != InodeType::Dir {
            return_errno_with_message!(Errno::ENOTDIR, "mknod on non-directory");
        }

        let fs = self.fs_ref();
        let device = fs.fid_mgr.device();
        let mode_bits = mode.bits() as u32;

        let (raw_mode, major, minor) = match type_ {
            MknodType::CharDevice(dev_id) => {
                let major = ((dev_id >> 8) & 0xfff) as u32;
                let minor = ((dev_id & 0xff) | ((dev_id >> 12) & 0xfff00)) as u32;
                (S_IFCHR | mode_bits, major, minor)
            }
            MknodType::BlockDevice(dev_id) => {
                let major = ((dev_id >> 8) & 0xfff) as u32;
                let minor = ((dev_id & 0xff) | ((dev_id >> 12) & 0xfff00)) as u32;
                (S_IFBLK | mode_bits, major, minor)
            }
            MknodType::NamedPipe => (S_IFIFO | mode_bits, 0, 0),
        };

        device
            .p9_mknod(self.fid, name, raw_mode, major, minor, 0)
            .map_err(Error::from)?;

        self.lookup(name)
    }

    fn link(&self, old: &Arc<dyn Inode>, name: &str) -> Result<()> {
        let metadata = self.metadata();
        if metadata.type_ != InodeType::Dir {
            return_errno_with_message!(Errno::ENOTDIR, "link on non-directory");
        }

        let old = old
            .downcast_ref::<Virtio9PInode>()
            .ok_or_else(|| Error::with_message(Errno::EXDEV, "not same fs"))?;

        let fs = self.fs_ref();
        fs.fid_mgr
            .device()
            .p9_link(self.fid, old.fid, name)
            .map_err(Error::from)?;
        Ok(())
    }

    fn unlink(&self, name: &str) -> Result<()> {
        let metadata = self.metadata();
        if metadata.type_ != InodeType::Dir {
            return_errno_with_message!(Errno::ENOTDIR, "unlink on non-directory");
        }

        let fs = self.fs_ref();
        fs.fid_mgr
            .device()
            .p9_unlinkat(self.fid, name, 0)
            .map_err(Error::from)
    }

    fn rmdir(&self, name: &str) -> Result<()> {
        let metadata = self.metadata();
        if metadata.type_ != InodeType::Dir {
            return_errno_with_message!(Errno::ENOTDIR, "rmdir on non-directory");
        }

        let fs = self.fs_ref();
        fs.fid_mgr
            .device()
            .p9_unlinkat(self.fid, name, AT_REMOVEDIR)
            .map_err(Error::from)
    }

    fn rename(&self, old_name: &str, target: &Arc<dyn Inode>, new_name: &str) -> Result<()> {
        let target = target
            .downcast_ref::<Virtio9PInode>()
            .ok_or_else(|| Error::with_message(Errno::EXDEV, "not same fs"))?;

        let fs = self.fs_ref();
        fs.fid_mgr
            .device()
            .p9_renameat(self.fid, old_name, target.fid, new_name)
            .map_err(Error::from)
    }

    fn readdir_at(&self, offset: usize, visitor: &mut dyn DirentVisitor) -> Result<usize> {
        let metadata = self.metadata();
        if metadata.type_ != InodeType::Dir {
            return_errno_with_message!(Errno::ENOTDIR, "readdir on non-directory");
        }

        let fs = self.fs_ref();
        // Clone fid, open as directory, read entries, clunk.
        let dir_fid = fs.fid_mgr.clone_fid(self.fid).map_err(Error::from)?;

        let result = (|| -> Result<usize> {
            fs.fid_mgr
                .device()
                .p9_lopen(dir_fid, O_RDONLY)
                .map_err(|_| Error::with_message(Errno::EIO, "9p lopen dir failed"))?;

            let entries = fs
                .fid_mgr
                .device()
                .p9_readdir(dir_fid, offset as u64, P9_READDIR_BUF_SIZE)
                .map_err(|_| Error::with_message(Errno::EIO, "9p readdir failed"))?;

            let mut current_off = offset;
            for entry in &entries {
                current_off = entry.offset as usize;
                visitor.visit(
                    entry.name.as_str(),
                    entry.qid.path,
                    inode_type_from_dirent_type(entry.type_),
                    current_off,
                )?;
            }

            Ok(current_off)
        })();

        fs.fid_mgr.clunk(dir_fid);
        result
    }

    fn read_link(&self) -> Result<SymbolicLink> {
        let metadata = self.metadata();
        if metadata.type_ != InodeType::SymLink {
            return_errno_with_message!(Errno::EINVAL, "read_link on non-symlink");
        }

        let fs = self.fs_ref();
        let target = fs
            .fid_mgr
            .device()
            .p9_readlink(self.fid)
            .map_err(Error::from)?;

        Ok(SymbolicLink::Plain(target))
    }

    fn sync_data(&self) -> Result<()> {
        self.flush_page_cache()?;
        let fs = self.fs_ref();
        let _ = fs.fid_mgr.device().p9_fsync(self.fid, 1);
        Ok(())
    }

    fn sync_all(&self) -> Result<()> {
        self.flush_page_cache()?;
        let fs = self.fs_ref();
        let _ = fs.fid_mgr.device().p9_fsync(self.fid, 0);
        Ok(())
    }

    fn fs(&self) -> Arc<dyn FileSystem> {
        self.fs_ref()
    }

    fn extension(&self) -> &Extension {
        &self.extension
    }
}
