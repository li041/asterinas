// SPDX-License-Identifier: MPL-2.0

use alloc::{
    string::{String, ToString},
    sync::{Arc, Weak},
    vec::Vec,
};
use core::{
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use aster_block::bio::BioWaiter;
use aster_virtio::device::filesystem::{
    device::{FileSystemDevice, VirtioFsDirEntry, get_device_by_tag},
    protocol::{Attr, FOPEN_DIRECT_IO, FOPEN_KEEP_CACHE, FUSE_ROOT_ID, FuseAttrOut},
};
use ostd::{
    mm::{HasSize, VmReader, VmWriter, io_util::HasVmReaderWriter},
    sync::RwLock,
};

use crate::{
    events::IoEvents,
    fs::{
        inode_handle::FileIo,
        path::Dentry,
        registry::{FsProperties, FsType},
        utils::{
            AccessMode, CachePage, DirentVisitor, Extension, FileSystem, FsEventSubscriberStats,
            FsFlags, Inode, InodeIo, InodeMode, InodeType, Metadata, PageCache, PageCacheBackend,
            StatusFlags, SuperBlock,
        },
    },
    prelude::*,
    process::{
        Gid, Uid,
        signal::{PollHandle, Pollable},
    },
    time::clocks::MonotonicCoarseClock,
    vm::vmo::Vmo,
};

const VIRTIOFS_MAGIC: u64 = 0x6573_5546;
const BLOCK_SIZE: usize = 4096;
const NAME_MAX: usize = 255;
const S_IFREG: u32 = 0o100000;
const S_IFDIR: u32 = 0o040000;
const O_RDONLY: u32 = 0;
const O_WRONLY: u32 = 1;
const O_RDWR: u32 = 2;

pub(super) struct VirtioFsType;

impl FsType for VirtioFsType {
    fn name(&self) -> &'static str {
        "virtiofs"
    }

    fn properties(&self) -> FsProperties {
        FsProperties::empty()
    }

    fn create(
        &self,
        _flags: FsFlags,
        args: Option<CString>,
        _disk: Option<Arc<dyn aster_block::BlockDevice>>,
    ) -> Result<Arc<dyn FileSystem>> {
        let tag = args
            .as_ref()
            .ok_or_else(|| Error::with_message(Errno::EINVAL, "virtiofs source(tag) is required"))?
            .to_str()
            .map_err(|_| Error::with_message(Errno::EINVAL, "invalid virtiofs tag"))?
            .to_string();

        let device = get_device_by_tag(&tag)
            .ok_or_else(|| Error::with_message(Errno::ENODEV, "virtiofs device tag not found"))?;

        Ok(VirtioFs::new(device, tag)? as _)
    }

    fn sysnode(&self) -> Option<Arc<dyn aster_systree::SysNode>> {
        None
    }
}

pub struct VirtioFs {
    sb: SuperBlock,
    root: Arc<VirtioFsInode>,
    tag: String,
    device: Arc<FileSystemDevice>,
    fs_event_subscriber_stats: FsEventSubscriberStats,
}

impl VirtioFs {
    fn new(device: Arc<FileSystemDevice>, tag: String) -> Result<Arc<Self>> {
        let root_attr = device.fuse_getattr(FUSE_ROOT_ID).map_err(Error::from)?.attr;
        let root_metadata = metadata_from_attr(root_attr);

        Ok(Arc::new_cyclic(|weak_fs| {
            let root = VirtioFsInode::new(FUSE_ROOT_ID, root_metadata, weak_fs.clone(), None);

            Self {
                sb: SuperBlock::new(VIRTIOFS_MAGIC, BLOCK_SIZE, NAME_MAX),
                root,
                tag,
                device,
                fs_event_subscriber_stats: FsEventSubscriberStats::new(),
            }
        }))
    }
}

impl FileSystem for VirtioFs {
    fn name(&self) -> &'static str {
        "virtiofs"
    }

    fn source(&self) -> Option<&str> {
        Some(&self.tag)
    }

    // lxh TODO: implement sync by issuing fsync to all open files and sync to the device if supported
    fn sync(&self) -> Result<()> {
        Ok(())
    }

    fn root_inode(&self) -> Arc<dyn Inode> {
        self.root.clone()
    }

    fn sb(&self) -> SuperBlock {
        self.sb.clone()
    }

    fn fs_event_subscriber_stats(&self) -> &FsEventSubscriberStats {
        &self.fs_event_subscriber_stats
    }
}

struct VirtioFsInode {
    this: Weak<VirtioFsInode>,
    nodeid: AtomicU64,
    lookup_count: AtomicU64,
    metadata: RwLock<Metadata>,
    entry_valid_until: RwLock<Option<Duration>>,
    attr_valid_until: RwLock<Option<Duration>>,
    page_cache: Option<PageCache>,
    cache_lock: Mutex<()>,
    fs: Weak<VirtioFs>,
    extension: Extension,
}

struct VirtioFsHandle {
    inode: Arc<VirtioFsInode>,
    fh: u64,
    release_flags: u32,
    cache_enabled: bool,
}

impl VirtioFsInode {
    fn new(
        nodeid: u64,
        metadata: Metadata,
        fs: Weak<VirtioFs>,
        entry_valid_until: Option<Duration>,
    ) -> Arc<Self> {
        Arc::new_cyclic(|weak_self| Self {
            this: weak_self.clone(),
            nodeid: AtomicU64::new(nodeid),
            lookup_count: AtomicU64::new(0),
            metadata: RwLock::new(metadata),
            entry_valid_until: RwLock::new(entry_valid_until),
            attr_valid_until: RwLock::new(None),
            page_cache: metadata
                .type_
                .is_regular_file()
                .then(|| PageCache::with_capacity(metadata.size, weak_self.clone() as _).unwrap()),
            cache_lock: Mutex::new(()),
            fs,
            extension: Extension::new(),
        })
    }

    fn fs_ref(&self) -> Arc<VirtioFs> {
        self.fs.upgrade().unwrap()
    }

    fn nodeid(&self) -> u64 {
        self.nodeid.load(Ordering::Relaxed)
    }

    fn increase_lookup_count(&self, count: u64) {
        if count == 0 {
            return;
        }

        self.lookup_count.fetch_add(count, Ordering::Relaxed);
    }

    fn release_lookup_count(&self) {
        let nlookup = self.lookup_count.swap(0, Ordering::Relaxed);
        if nlookup == 0 {
            return;
        }

        let nodeid = self.nodeid();
        if nodeid == FUSE_ROOT_ID {
            return;
        }

        let Some(fs) = self.fs.upgrade() else {
            return;
        };

        let _ = fs.device.fuse_forget(nodeid, nlookup);
    }

    fn revalidate_lookup(&self, parent_nodeid: u64, name: &str) -> Result<()> {
        let now = MonotonicCoarseClock::get().read_time();
        if self
            .entry_valid_until
            .read()
            .is_some_and(|valid_until| now < valid_until)
        {
            return Ok(());
        }

        let old_nodeid = self.nodeid();
        let fs = self.fs_ref();
        let entry_out = fs
            .device
            .fuse_lookup(parent_nodeid, name)
            .map_err(Error::from)?;
        self.increase_lookup_count(1);

        if entry_out.nodeid != old_nodeid {
            return_errno_with_message!(Errno::ENOENT, "virtiofs stale dentry after revalidate");
        }

        *self.metadata.write() = metadata_from_attr(entry_out.attr);

        let now = MonotonicCoarseClock::get().read_time();
        *self.entry_valid_until.write() = Some(now.saturating_add(valid_duration(
            entry_out.entry_valid,
            entry_out.entry_valid_nsec,
        )));
        *self.attr_valid_until.write() = Some(now.saturating_add(valid_duration(
            entry_out.attr_valid,
            entry_out.attr_valid_nsec,
        )));

        Ok(())
    }

    fn refresh_attr_if_needed(&self) -> Result<()> {
        let now = MonotonicCoarseClock::get().read_time();
        let should_refresh = match *self.attr_valid_until.read() {
            Some(valid_until) => now >= valid_until,
            None => true,
        };

        if !should_refresh {
            return Ok(());
        }

        let _guard = self.cache_lock.lock();
        let now = MonotonicCoarseClock::get().read_time();
        if self
            .attr_valid_until
            .read()
            .is_some_and(|valid_until| now < valid_until)
        {
            return Ok(());
        }

        let old_metadata = self.metadata();
        let fs = self.fs_ref();
        let attr_out = fs.device.fuse_getattr(self.nodeid()).map_err(Error::from)?;

        let new_metadata = metadata_from_attr(attr_out.attr);
        if old_metadata.mtime != new_metadata.mtime {
            self.invalidate_page_cache_locked(new_metadata.size)?;
        } else if let Some(page_cache) = &self.page_cache
            && page_cache.pages().size() != new_metadata.size
        {
            page_cache.resize(new_metadata.size)?;
        }

        *self.metadata.write() = new_metadata;
        let now = MonotonicCoarseClock::get().read_time();
        *self.attr_valid_until.write() = Some(now.saturating_add(valid_duration(
            attr_out.attr_valid,
            attr_out.attr_valid_nsec,
        )));
        Ok(())
    }

    fn invalidate_page_cache_locked(&self, new_size: usize) -> Result<()> {
        let Some(page_cache) = &self.page_cache else {
            return Ok(());
        };

        let cached_size = page_cache.pages().size();
        if cached_size > 0 {
            page_cache.discard_range(0..cached_size);
            page_cache.resize(0)?;
        }
        page_cache.resize(new_size)?;
        Ok(())
    }

    fn invalidate_page_cache(&self, new_size: usize) -> Result<()> {
        let _guard = self.cache_lock.lock();
        self.invalidate_page_cache_locked(new_size)
    }

    fn flush_page_cache(&self) -> Result<()> {
        let Some(page_cache) = &self.page_cache else {
            return Ok(());
        };

        let _guard = self.cache_lock.lock();
        page_cache.evict_range(0..self.size())?;
        Ok(())
    }

    fn mark_attr_stale(&self) {
        *self.attr_valid_until.write() = None;
    }

    fn cached_read_at(&self, offset: usize, writer: &mut VmWriter) -> Result<usize> {
        self.refresh_attr_if_needed()?;

        let Some(page_cache) = &self.page_cache else {
            return self.direct_read_at(offset, writer);
        };

        let file_size = self.size();
        let start = file_size.min(offset);
        let end = file_size.min(offset + writer.avail());
        let read_len = end - start;
        page_cache.pages().read(start, writer)?;
        Ok(read_len)
    }

    fn direct_read_at(&self, offset: usize, writer: &mut VmWriter) -> Result<usize> {
        self.refresh_attr_if_needed()?;

        let file_size = self.size();
        let start = file_size.min(offset);
        let end = file_size.min(offset + writer.avail());
        let read_len = end - start;
        let max_len = read_len.min(u32::MAX as usize) as u32;
        let fs = self.fs_ref();
        let data = fs
            .device
            .fuse_open(self.nodeid(), O_RDONLY)
            .and_then(|fh_out| {
                let fh = fh_out.fh;
                let result = fs
                    .device
                    .fuse_read(self.nodeid(), fh, start as u64, max_len as u32);
                let _ = fs.device.fuse_release(self.nodeid(), fh, O_RDONLY);
                result
            })
            .map_err(|_| Error::with_message(Errno::EIO, "virtiofs read failed"))?;
        let mut reader = VmReader::from(data.as_slice());
        writer.write_fallible(&mut reader)?;
        Ok(read_len.min(data.len()))
    }

    fn cached_write_at(&self, offset: usize, reader: &mut VmReader) -> Result<usize> {
        let Some(page_cache) = &self.page_cache else {
            return self.direct_write_at(offset, reader);
        };

        let write_len = reader.remain();
        let new_size = offset + write_len;
        {
            let _guard = self.cache_lock.lock();
            if new_size > page_cache.pages().size() {
                page_cache.resize(new_size)?;
            }
            {
                let mut metadata = self.metadata.write();
                metadata.size = metadata.size.max(new_size);
                metadata.blocks = metadata.size.div_ceil(metadata.blk_size.max(1));
            }
            page_cache.pages().write(offset, reader)?;
        }

        self.mark_attr_stale();
        Ok(write_len)
    }

    fn direct_write_at(&self, offset: usize, reader: &mut VmReader) -> Result<usize> {
        let write_len = reader.remain().min(u32::MAX as usize);
        let mut data = vec![0u8; write_len];
        reader.read_fallible(&mut VmWriter::from(data.as_mut_slice()))?;

        let fs = self.fs_ref();
        let written = fs
            .device
            .fuse_open(self.nodeid(), O_WRONLY)
            .and_then(|fh_out| {
                let fh = fh_out.fh;
                let result = fs
                    .device
                    .fuse_write(self.nodeid(), fh, offset as u64, &data);
                let _ = fs.device.fuse_release(self.nodeid(), fh, O_WRONLY);
                result
            })
            .map_err(|_| Error::with_message(Errno::EIO, "virtiofs write failed"))?;

        let new_size = offset + written;
        {
            let mut metadata = self.metadata.write();
            metadata.size = metadata.size.max(new_size);
            metadata.blocks = metadata.size.div_ceil(metadata.blk_size.max(1));
        }

        if self.page_cache.is_some() {
            self.invalidate_page_cache(self.size())?;
        }
        self.mark_attr_stale();
        Ok(written)
    }

    fn open_handle(&self, access_mode: AccessMode) -> Result<VirtioFsHandle> {
        let open_flags = match access_mode {
            AccessMode::O_RDONLY => O_RDONLY,
            AccessMode::O_WRONLY => O_WRONLY,
            AccessMode::O_RDWR => O_RDWR,
        };

        let fs = self.fs_ref();
        let open_out = fs
            .device
            .fuse_open(self.nodeid(), open_flags)
            .map_err(|_| Error::with_message(Errno::EIO, "virtiofs open failed"))?;
        let cache_enabled =
            self.page_cache.is_some() && (open_out.open_flags & FOPEN_DIRECT_IO == 0);

        if open_out.open_flags & FOPEN_KEEP_CACHE == 0 {
            self.invalidate_page_cache(self.size())?;
        }

        let Some(inode) = self.this.upgrade() else {
            let _ = fs
                .device
                .fuse_release(self.nodeid(), open_out.fh, open_flags);
            return_errno_with_message!(Errno::EIO, "virtiofs inode is unavailable");
        };

        Ok(VirtioFsHandle {
            inode,
            fh: open_out.fh,
            release_flags: open_flags,
            cache_enabled,
        })
    }
}

impl Drop for VirtioFsInode {
    fn drop(&mut self) {
        self.release_lookup_count();
    }
}

impl PageCacheBackend for VirtioFsInode {
    fn read_page_async(&self, idx: usize, frame: &CachePage) -> Result<BioWaiter> {
        let offset = idx * PAGE_SIZE;
        if offset >= self.size() {
            return_errno_with_message!(Errno::EINVAL, "virtiofs page read beyond EOF");
        }

        frame.writer().fill_zeros(frame.size());
        let size = (self.size() - offset).min(PAGE_SIZE).min(u32::MAX as usize) as u32;
        let fs = self.fs_ref();
        let data = fs
            .device
            .fuse_open(self.nodeid(), O_RDONLY)
            .and_then(|fh_out| {
                let fh = fh_out.fh;
                let result = fs.device.fuse_read(self.nodeid(), fh, offset as u64, size);
                let _ = fs.device.fuse_release(self.nodeid(), fh, O_RDONLY);
                result
            })
            .map_err(|_| Error::with_message(Errno::EIO, "virtiofs page read failed"))?;
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
        fs.device
            .fuse_open(self.nodeid(), O_WRONLY)
            .and_then(|fh_out| {
                let fh = fh_out.fh;
                let result = fs
                    .device
                    .fuse_write(self.nodeid(), fh, offset as u64, &data);
                let _ = fs.device.fuse_release(self.nodeid(), fh, O_WRONLY);
                result
            })
            .map_err(|_| Error::with_message(Errno::EIO, "virtiofs page write failed"))?;
        Ok(BioWaiter::new())
    }

    fn npages(&self) -> usize {
        self.size().div_ceil(PAGE_SIZE)
    }
}

impl Drop for VirtioFsHandle {
    fn drop(&mut self) {
        if self.cache_enabled {
            let _ = self.inode.flush_page_cache();
        }
        if let Some(fs) = self.inode.fs.upgrade() {
            let _ = fs
                .device
                .fuse_release(self.inode.nodeid(), self.fh, self.release_flags);
        }
    }
}

impl Pollable for VirtioFsHandle {
    fn poll(&self, mask: IoEvents, _poller: Option<&mut PollHandle>) -> IoEvents {
        let events = IoEvents::IN | IoEvents::OUT;
        events & mask
    }
}

impl InodeIo for VirtioFsHandle {
    fn read_at(
        &self,
        offset: usize,
        writer: &mut VmWriter,
        _status_flags: StatusFlags,
    ) -> Result<usize> {
        if self.cache_enabled {
            self.inode.cached_read_at(offset, writer)
        } else {
            self.inode.direct_read_at(offset, writer)
        }
    }

    fn write_at(
        &self,
        offset: usize,
        reader: &mut VmReader,
        _status_flags: StatusFlags,
    ) -> Result<usize> {
        if self.cache_enabled {
            self.inode.cached_write_at(offset, reader)
        } else {
            self.inode.direct_write_at(offset, reader)
        }
    }
}

impl FileIo for VirtioFsHandle {
    fn check_seekable(&self) -> Result<()> {
        Ok(())
    }

    fn is_offset_aware(&self) -> bool {
        true
    }
}

impl InodeIo for VirtioFsInode {
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
        _status_flags: StatusFlags,
    ) -> Result<usize> {
        let metadata = self.metadata();
        if metadata.type_ != InodeType::File {
            return_errno_with_message!(Errno::EISDIR, "inode is not a regular file");
        }

        self.cached_write_at(offset, reader)
    }
}

impl Inode for VirtioFsInode {
    fn size(&self) -> usize {
        self.metadata.read().size
    }

    fn resize(&self, new_size: usize) -> Result<()> {
        if self.metadata.read().type_ != InodeType::File {
            return_errno_with_message!(Errno::EISDIR, "resize on non-regular file");
        }

        let size = u64::try_from(new_size)
            .map_err(|_| Error::with_message(Errno::EFBIG, "virtiofs resize size too large"))?;

        let fs = self.fs_ref();
        let attr_out = fs
            .device
            .fuse_setattr(self.nodeid(), size)
            .map_err(Error::from)?;

        let new_metadata = metadata_from_attr(attr_out.attr);
        {
            let _guard = self.cache_lock.lock();
            if let Some(page_cache) = &self.page_cache {
                page_cache.resize(new_metadata.size)?;
            }
            *self.metadata.write() = new_metadata;
        }
        self.mark_attr_stale();
        Ok(())
    }

    fn metadata(&self) -> Metadata {
        *self.metadata.read()
    }

    fn ino(&self) -> u64 {
        self.nodeid()
    }

    fn type_(&self) -> InodeType {
        self.metadata.read().type_
    }

    fn mode(&self) -> Result<InodeMode> {
        Ok(self.metadata.read().mode)
    }

    fn set_mode(&self, mode: InodeMode) -> Result<()> {
        self.metadata.write().mode = mode;
        Ok(())
    }

    fn owner(&self) -> Result<Uid> {
        Ok(self.metadata.read().uid)
    }

    fn set_owner(&self, uid: Uid) -> Result<()> {
        self.metadata.write().uid = uid;
        Ok(())
    }
    fn group(&self) -> Result<Gid> {
        Ok(self.metadata.read().gid)
    }

    fn set_group(&self, gid: Gid) -> Result<()> {
        self.metadata.write().gid = gid;
        Ok(())
    }

    fn atime(&self) -> Duration {
        self.metadata.read().atime
    }

    fn set_atime(&self, time: Duration) {
        self.metadata.write().atime = time;
    }

    fn mtime(&self) -> Duration {
        self.metadata.read().mtime
    }

    fn set_mtime(&self, time: Duration) {
        self.metadata.write().mtime = time;
    }

    fn ctime(&self) -> Duration {
        self.metadata.read().ctime
    }

    fn set_ctime(&self, time: Duration) {
        self.metadata.write().ctime = time;
    }

    fn page_cache(&self) -> Option<Arc<Vmo>> {
        self.page_cache
            .as_ref()
            .map(|page_cache| page_cache.pages().clone())
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
            return_errno_with_message!(Errno::ENOTDIR, "lookup on non-directory")
        }

        let fs = self.fs_ref();
        let parent_nodeid = self.nodeid();
        let entry_out = fs
            .device
            .fuse_lookup(parent_nodeid, name)
            .map_err(Error::from)?;
        let nodeid = entry_out.nodeid;
        let now = MonotonicCoarseClock::get().read_time();
        let entry_valid_until = Some(now.saturating_add(valid_duration(
            entry_out.entry_valid,
            entry_out.entry_valid_nsec,
        )));
        let attr_valid_until = Some(now.saturating_add(valid_duration(
            entry_out.attr_valid,
            entry_out.attr_valid_nsec,
        )));

        let inode = VirtioFsInode::new(
            nodeid,
            metadata_from_attr(entry_out.attr),
            Arc::downgrade(&fs),
            entry_valid_until,
        );
        *inode.attr_valid_until.write() = attr_valid_until;
        inode.increase_lookup_count(1);
        Ok(inode)
    }

    fn create(&self, name: &str, type_: InodeType, mode: InodeMode) -> Result<Arc<dyn Inode>> {
        let metadata = self.metadata();
        if metadata.type_ != InodeType::Dir {
            return_errno_with_message!(Errno::ENOTDIR, "create on non-directory")
        }

        let fs = self.fs_ref();
        let parent_nodeid = self.nodeid();
        let (entry_out, opened_open_out) = match type_ {
            InodeType::File => {
                let (entry_out, open_out) = fs
                    .device
                    .fuse_create(parent_nodeid, name, S_IFREG | mode.bits() as u32)
                    .map_err(Error::from)?;
                (entry_out, Some(open_out))
            }
            InodeType::Dir => {
                let entry_out = fs
                    .device
                    .fuse_mkdir(parent_nodeid, name, S_IFDIR | mode.bits() as u32)
                    .map_err(Error::from)?;
                (entry_out, None)
            }
            _ => {
                return_errno_with_message!(
                    Errno::EOPNOTSUPP,
                    "virtiofs create supports file/dir only"
                )
            }
        };
        let attr_out: FuseAttrOut = fs
            .device
            .fuse_getattr(entry_out.nodeid)
            .map_err(|_| Error::with_message(Errno::EIO, "virtiofs getattr after create failed"))?;

        if let Some(open_out) = opened_open_out {
            let _ = fs
                .device
                .fuse_release(entry_out.nodeid, open_out.fh, O_RDWR);
        }

        let now = MonotonicCoarseClock::get().read_time();

        let entry_valid_until = Some(now.saturating_add(valid_duration(
            entry_out.entry_valid,
            entry_out.entry_valid_nsec,
        )));
        let attr_valid_until = Some(now.saturating_add(valid_duration(
            attr_out.attr_valid,
            attr_out.attr_valid_nsec,
        )));

        let inode = VirtioFsInode::new(
            entry_out.nodeid,
            metadata_from_attr(attr_out.attr),
            Arc::downgrade(&fs),
            entry_valid_until,
        );
        *inode.attr_valid_until.write() = attr_valid_until;
        inode.increase_lookup_count(1);

        Ok(inode)
    }

    fn unlink(&self, name: &str) -> Result<()> {
        let metadata = self.metadata();
        if metadata.type_ != InodeType::Dir {
            return_errno_with_message!(Errno::ENOTDIR, "unlink on non-directory")
        }

        let fs = self.fs_ref();
        fs.device
            .fuse_unlink(self.nodeid(), name)
            .map_err(Error::from)
    }

    fn rmdir(&self, name: &str) -> Result<()> {
        let metadata = self.metadata();
        if metadata.type_ != InodeType::Dir {
            return_errno_with_message!(Errno::ENOTDIR, "rmdir on non-directory")
        }

        let fs = self.fs_ref();
        fs.device
            .fuse_rmdir(self.nodeid(), name)
            .map_err(Error::from)
    }

    fn readdir_at(&self, offset: usize, visitor: &mut dyn DirentVisitor) -> Result<usize> {
        // lxh debug
        const FUSE_READDIR_BUF_SIZE: u32 = 4096;
        let metadata = self.metadata();
        if metadata.type_ != InodeType::Dir {
            return_errno_with_message!(Errno::ENOTDIR, "readdir on non-directory")
        }

        let fs = self.fs_ref();
        let entries: Vec<VirtioFsDirEntry> = fs
            .device
            .fuse_opendir(self.nodeid())
            .and_then(|fh| {
                let result =
                    fs.device
                        .fuse_readdir(self.nodeid(), fh, offset as u64, FUSE_READDIR_BUF_SIZE);
                let _ = fs.device.fuse_releasedir(self.nodeid(), fh);
                result
            })
            .map_err(|_| Error::with_message(Errno::EIO, "virtiofs readdir failed"))?;

        let mut current_off = offset;
        for entry in entries.iter() {
            current_off = entry.offset as usize;
            visitor.visit(
                entry.name.as_str(),
                entry.ino,
                inode_type_from_dirent_type(entry.type_),
                current_off,
            )?;
        }

        Ok(current_off)
    }

    fn fs(&self) -> Arc<dyn FileSystem> {
        self.fs_ref()
    }

    fn sync_data(&self) -> Result<()> {
        self.flush_page_cache()
    }

    fn revalidate_child(&self, name: &str, child: &Dentry) -> Result<()> {
        let Some(parent) = child.parent() else {
            return Ok(());
        };

        self.revalidate_lookup(parent.inode().ino(), name)
    }

    fn extension(&self) -> &Extension {
        &self.extension
    }
}

fn inode_type_from_dirent_type(type_: u32) -> InodeType {
    match type_ {
        4 => InodeType::Dir,
        8 => InodeType::File,
        10 => InodeType::SymLink,
        2 => InodeType::CharDevice,
        6 => InodeType::BlockDevice,
        1 => InodeType::NamedPipe,
        12 => InodeType::Socket,
        _ => InodeType::Unknown,
    }
}

fn metadata_from_attr(attr: Attr) -> Metadata {
    Metadata {
        dev: 0,
        ino: attr.ino,
        size: attr.size as usize,
        blk_size: attr.blksize as usize,
        blocks: attr.blocks as usize,
        atime: Duration::new(attr.atime, attr.atimensec),
        mtime: Duration::new(attr.mtime, attr.mtimensec),
        ctime: Duration::new(attr.ctime, attr.ctimensec),
        type_: InodeType::from_raw_mode(attr.mode as u16).unwrap_or(InodeType::Unknown),
        mode: InodeMode::from_bits_truncate(attr.mode as u16),
        nlinks: attr.nlink as usize,
        uid: Uid::new(attr.uid),
        gid: Gid::new(attr.gid),
        rdev: attr.rdev as u64,
    }
}

fn valid_duration(secs: u64, nsecs: u32) -> Duration {
    let extra_secs = (nsecs / 1_000_000_000) as u64;
    let nanos = (nsecs % 1_000_000_000) as u64;
    Duration::from_secs(secs.saturating_add(extra_secs)).saturating_add(Duration::from_nanos(nanos))
}
