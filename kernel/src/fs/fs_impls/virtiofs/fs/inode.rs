// SPDX-License-Identifier: MPL-2.0

//! Inode implementation for `virtiofs`.
//!
//! This module defines [`VirtioFsInode`], which owns cached metadata, optional
//! page-cache state, and the inode operations backed by FUSE requests.

use alloc::{
    sync::{Arc, Weak},
    vec::Vec,
};
use core::{
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use aster_block::bio::BioWaiter;
use aster_fuse::{
    DirentType, EntryOut, FuseAttrOut, FuseFileHandle, OpenFlags, SetattrIn, SetattrValid,
};
use aster_virtio::device::filesystem::device::{ReleaseKind, VirtioFsDirEntry};
use ostd::{
    mm::{HasSize, VmReader, VmWriter, io::util::HasVmReaderWriter},
    sync::RwLock,
    warn,
};

use super::{
    super::metadata_from_attr,
    FUSE_READDIR_BUF_SIZE, VirtioFs,
    handle::{CacheMode, VirtioFsDirHandle, VirtioFsHandle},
    valid_duration,
};
use crate::{
    fs::{
        file::{AccessMode, FileIo, InodeMode, InodeType, StatusFlags},
        utils::DirentVisitor,
        vfs::{
            file_system::FileSystem,
            inode::{Extension, Inode, InodeIo, Metadata, SymbolicLink},
            page_cache::{CachePage, PageCache, PageCacheBackend},
            path::Dentry,
        },
    },
    prelude::*,
    process::{Gid, Uid},
    thread::work_queue::{self, WorkPriority},
    time::clocks::MonotonicCoarseClock,
    vm::vmo::Vmo,
};

pub(super) struct VirtioFsInode {
    this: Weak<VirtioFsInode>,
    nodeid: AtomicU64,
    lookup_count: AtomicU64,
    metadata: RwLock<Metadata>,
    // TODO: Move the entry timeout state to `Dentry` once the VFS can carry
    // filesystem-specific per-dentry data. This timeout belongs to the cached
    // name-to-inode association, not to the inode object itself.
    // Reference: https://codebrowser.dev/linux/linux/fs/fuse/dir.c.html#98
    // Reference: https://codebrowser.dev/linux/linux/fs/fuse/dir.c.html#275
    entry_valid_until: RwLock<Option<Duration>>,
    attr_valid_until: RwLock<Duration>,
    page_cache: Option<Mutex<PageCache>>,
    // Dedicated FUSE file handle for page-cache backend I/O.
    //
    // This handle is independent from VFS file descriptors and `VirtioFsHandle`:
    // 1. VFS enforces fd access rights at syscall/open boundaries.
    // 2. PageCacheBackend may issue both read and write requests during cache
    //   lifecycle operations (fill, writeback, eviction), so it keeps one
    //   inode-scoped `O_RDWR` fh for correctness and simplicity.
    // 3. Per-open/per-syscall fhs are still opened with their own access modes.
    page_cache_fh: Mutex<Option<FuseFileHandle>>,
    fs: Weak<VirtioFs>,
    extension: Extension,
}

impl VirtioFsInode {
    pub(super) fn new(
        nodeid: u64,
        metadata: Metadata,
        fs: Weak<VirtioFs>,
        entry_valid_until: Option<Duration>,
        attr_valid_until: Duration,
    ) -> Arc<Self> {
        Arc::new_cyclic(|weak_self| Self {
            this: weak_self.clone(),
            nodeid: AtomicU64::new(nodeid),
            lookup_count: AtomicU64::new(0),
            metadata: RwLock::new(metadata),
            entry_valid_until: RwLock::new(entry_valid_until),
            attr_valid_until: RwLock::new(attr_valid_until),
            page_cache: metadata.type_.is_regular_file().then(|| {
                Mutex::new(PageCache::with_capacity(metadata.size, weak_self.clone() as _).unwrap())
            }),
            page_cache_fh: Mutex::new(None),
            fs,
            extension: Extension::new(),
        })
    }

    pub(super) fn fs_ref(&self) -> Arc<VirtioFs> {
        self.fs.upgrade().unwrap()
    }

    pub(super) fn try_fs_ref(&self) -> Option<Arc<VirtioFs>> {
        self.fs.upgrade()
    }

    pub(super) fn nodeid(&self) -> u64 {
        self.nodeid.load(Ordering::Relaxed)
    }

    pub(super) fn size(&self) -> usize {
        self.metadata.read().size
    }

    fn increase_lookup_count(&self) {
        self.lookup_count.fetch_add(1, Ordering::Relaxed);
    }

    fn release_lookup_count(&self, nlookup: u64) {
        if nlookup == 0 {
            return;
        }

        self.forget_async(nlookup);
    }

    fn forget_async(&self, nlookup: u64) {
        let nodeid = self.nodeid();

        if let Some(fs) = self.fs.upgrade() {
            work_queue::submit_work_func(
                move || {
                    fs.conn.fuse_forget(nodeid, nlookup);
                },
                WorkPriority::Normal,
            );
        }
    }

    fn get_or_open_page_cache_fh(&self) -> Result<FuseFileHandle> {
        if let Some(fh) = *self.page_cache_fh.lock() {
            return Ok(fh);
        }

        let fs = self.fs_ref();
        // Keep the page-cache fh as `O_RDWR` unconditionally. Permission checks
        // are enforced by the VFS/open path, while this backend handle serves
        // cache-internal reads and writeback for the same inode.
        let opened_fh = fs
            .conn
            .fuse_open(self.nodeid(), AccessMode::O_RDWR as u32)?
            .fh;

        let mut fh_slot = self.page_cache_fh.lock();
        if let Some(existing_fh) = *fh_slot {
            fs.conn.fuse_release(
                self.nodeid(),
                opened_fh,
                AccessMode::O_RDWR as u32,
                ReleaseKind::File,
            );
            return Ok(existing_fh);
        }

        *fh_slot = Some(opened_fh);
        Ok(opened_fh)
    }

    fn release_page_cache_fh(&self) {
        let Some(fs) = self.fs.upgrade() else {
            return;
        };

        if let Some(fh) = self.page_cache_fh.lock().take() {
            fs.conn.fuse_release(
                self.nodeid(),
                fh,
                AccessMode::O_RDWR as u32,
                ReleaseKind::File,
            );
        }
    }

    fn revalidate_lookup(&self, parent_nodeid: u64, name: &str) -> Result<()> {
        let now = MonotonicCoarseClock::get().read_time();
        if self
            .entry_valid_until
            .read()
            .is_none_or(|valid_until| now < valid_until)
        {
            return Ok(());
        }

        let old_nodeid = self.nodeid();
        let fs = self.fs_ref();
        let entry_out = fs.conn.fuse_lookup(parent_nodeid, name)?;

        if entry_out.nodeid != old_nodeid {
            // The returned entry refers to a different inode. Drop the lookup
            // reference immediately so we don't leak nlookup on that node.
            fs.conn.fuse_forget(entry_out.nodeid, 1);
            return_errno_with_message!(Errno::ESTALE, "virtiofs stale dentry after revalidate");
        }

        // Count only lookups that still point to this inode.
        self.increase_lookup_count();

        let metadata = metadata_from_attr(entry_out.attr, fs.sb().container_dev_id);
        self.set_metadata(metadata)?;

        let now = MonotonicCoarseClock::get().read_time();
        *self.entry_valid_until.write() = Some(now.saturating_add(valid_duration(
            entry_out.entry_valid,
            entry_out.entry_valid_nsec,
        )));
        *self.attr_valid_until.write() = now.saturating_add(valid_duration(
            entry_out.attr_valid,
            entry_out.attr_valid_nsec,
        ));

        Ok(())
    }

    pub(super) fn revalidate_attr(&self, fh: FuseFileHandle) -> Result<()> {
        let now = MonotonicCoarseClock::get().read_time();
        if now < *self.attr_valid_until.read() {
            return Ok(());
        }

        let fs = self.fs_ref();
        let attr_out = fs.conn.fuse_getattr(self.nodeid(), fh)?;

        let metadata = metadata_from_attr(attr_out.attr, fs.sb().container_dev_id);
        self.set_metadata(metadata)?;

        let now = MonotonicCoarseClock::get().read_time();
        *self.attr_valid_until.write() = now.saturating_add(valid_duration(
            attr_out.attr_valid,
            attr_out.attr_valid_nsec,
        ));

        Ok(())
    }

    fn invalidate_page_cache(&self, new_size: usize) -> Result<()> {
        let Some(page_cache) = &self.page_cache else {
            return Ok(());
        };

        let page_cache = &mut page_cache.lock();

        let cached_size = page_cache.pages().size();
        if cached_size > 0 {
            // Dirty cache pages are laundered before they are removed from the page cache,
            // instead of being silently dropped.
            // Reference: https://codebrowser.dev/linux/linux/fs/fuse/file.c.html#292
            // Reference: https://codebrowser.dev/linux/linux/mm/truncate.c.html#633
            page_cache.evict_range(0..cached_size)?;
            page_cache.resize(0)?;
        }
        page_cache.resize(new_size)?;

        Ok(())
    }

    fn set_metadata(&self, metadata: Metadata) -> Result<()> {
        let old_metadata = self.metadata();
        if self.page_cache.is_some()
            && (old_metadata.size != metadata.size
                || old_metadata.last_modify_at != metadata.last_modify_at)
        {
            self.invalidate_page_cache(metadata.size)?;
        }
        *self.metadata.write() = metadata;
        Ok(())
    }

    pub(super) fn flush_page_cache(&self) -> Result<()> {
        let Some(page_cache) = &self.page_cache else {
            return Ok(());
        };

        page_cache.lock().evict_range(0..self.size())?;
        Ok(())
    }

    pub(super) fn cached_read_at(
        &self,
        offset: usize,
        writer: &mut VmWriter,
        fh: FuseFileHandle,
    ) -> Result<usize> {
        self.revalidate_attr(fh)?;

        let Some(page_cache) = &self.page_cache else {
            return self.direct_read_at(offset, writer, fh);
        };

        let file_size = self.size();
        let start = file_size.min(offset);
        let end = file_size.min(offset.saturating_add(writer.avail()));
        let read_len = end - start;
        page_cache.lock().pages().read(start, writer)?;
        Ok(read_len)
    }

    pub(super) fn direct_read_at(
        &self,
        offset: usize,
        writer: &mut VmWriter,
        fh: FuseFileHandle,
    ) -> Result<usize> {
        self.revalidate_attr(fh)?;

        let file_size = self.size();
        let start = file_size.min(offset);
        let end = file_size.min(offset.saturating_add(writer.avail()));
        let read_len = end - start;
        let max_len = read_len.min(u32::MAX as usize) as u32;
        let data = self
            .fs_ref()
            .conn
            .fuse_read(self.nodeid(), fh, start as u64, max_len)?;
        let mut reader = VmReader::from(data.as_slice());
        writer.write_fallible(&mut reader)?;
        Ok(read_len.min(data.len()))
    }

    pub(super) fn cached_write_at(
        &self,
        offset: usize,
        reader: &mut VmReader,
        fh: FuseFileHandle,
    ) -> Result<usize> {
        let Some(page_cache) = &self.page_cache else {
            return self.direct_write_at(offset, reader, fh);
        };

        // TODO: Support `FUSE_WRITEBACK_CACHE`. `FuseConnection::init_flags`
        // does not currently request the flag, so writeback caching is not
        // negotiated and the kernel must not buffer writes as dirty
        // page-cache pages. Until writeback is implemented we use a
        // write-through path here: every `write(2)` is sent to the daemon
        // synchronously (mirroring Linux's non-writeback `fuse_perform_write`),
        // and cached pages overlapping the written range are then discarded
        // so subsequent reads refetch the authoritative bytes from the
        // daemon. When writeback is added, this branch should instead
        // populate the cache in-place and mark pages `UpToDate` (not
        // `Dirty`) to avoid the refetch.
        //
        // Reference: https://codebrowser.dev/linux/linux/fs/fuse/file.c.html#fuse_perform_write
        let write_len = reader.remain().min(u32::MAX as usize);
        let mut data = vec![0u8; write_len];
        reader.read_fallible(&mut VmWriter::from(data.as_mut_slice()))?;

        let written = self
            .fs_ref()
            .conn
            .fuse_write(self.nodeid(), fh, offset as u64, &data)?;

        let new_size = offset
            .checked_add(written)
            .ok_or_else(|| Error::with_message(Errno::EOVERFLOW, "virtiofs write size overflow"))?;
        {
            let mut metadata = self.metadata.write();
            metadata.size = metadata.size.max(new_size);
            metadata.nr_sectors_allocated = metadata.size.div_ceil(512);
        }

        let page_cache = page_cache.lock();
        if new_size > page_cache.pages().size() {
            page_cache.resize(new_size)?;
        }
        page_cache.discard_range(offset..new_size);

        Ok(written)
    }

    pub(super) fn direct_write_at(
        &self,
        offset: usize,
        reader: &mut VmReader,
        fh: FuseFileHandle,
    ) -> Result<usize> {
        let write_len = reader.remain().min(u32::MAX as usize);
        let mut data = vec![0u8; write_len];
        reader.read_fallible(&mut VmWriter::from(data.as_mut_slice()))?;

        let written = self
            .fs_ref()
            .conn
            .fuse_write(self.nodeid(), fh, offset as u64, &data)?;

        let new_size = offset
            .checked_add(written)
            .ok_or_else(|| Error::with_message(Errno::EOVERFLOW, "virtiofs write size overflow"))?;
        {
            let mut metadata = self.metadata.write();
            metadata.size = metadata.size.max(new_size);
            metadata.nr_sectors_allocated = metadata.size.div_ceil(512);
        }

        self.invalidate_page_cache(self.size())?;

        Ok(written)
    }

    fn open_handle(&self, access_mode: AccessMode) -> Result<VirtioFsHandle> {
        let fs = self.fs_ref();
        let open_out = fs.conn.fuse_open(self.nodeid(), access_mode as u32)?;
        let cache_mode = if self.page_cache.is_some()
            && !open_out.open_flags.contains(OpenFlags::FOPEN_DIRECT_IO)
        {
            CacheMode::Cached
        } else {
            CacheMode::Direct
        };

        if !open_out.open_flags.contains(OpenFlags::FOPEN_KEEP_CACHE) {
            self.invalidate_page_cache(self.size())?;
        }

        let Some(inode) = self.this.upgrade() else {
            fs.conn.fuse_release(
                self.nodeid(),
                open_out.fh,
                access_mode as u32,
                ReleaseKind::File,
            );
            return_errno_with_message!(Errno::EIO, "virtiofs inode is unavailable");
        };

        Ok(VirtioFsHandle::new(
            inode,
            open_out.fh,
            access_mode,
            cache_mode,
        ))
    }

    fn open_dir_handle(&self) -> Result<VirtioFsDirHandle> {
        let fs = self.fs_ref();
        let fh = fs.conn.fuse_opendir(self.nodeid())?;

        let Some(inode) = self.this.upgrade() else {
            fs.conn
                .fuse_release(self.nodeid(), fh, 0, ReleaseKind::Directory);
            return_errno_with_message!(Errno::EIO, "virtiofs inode is unavailable");
        };

        Ok(VirtioFsDirHandle::new(inode, fh))
    }

    fn build_child_inode(
        &self,
        fs: &Arc<VirtioFs>,
        entry_out: EntryOut,
        attr_out: FuseAttrOut,
    ) -> Arc<VirtioFsInode> {
        let now = MonotonicCoarseClock::get().read_time();
        let entry_valid_until = Some(now.saturating_add(valid_duration(
            entry_out.entry_valid,
            entry_out.entry_valid_nsec,
        )));
        let attr_valid_until = now.saturating_add(valid_duration(
            attr_out.attr_valid,
            attr_out.attr_valid_nsec,
        ));

        let inode = VirtioFsInode::new(
            entry_out.nodeid,
            metadata_from_attr(attr_out.attr, fs.sb().container_dev_id),
            Arc::downgrade(fs),
            entry_valid_until,
            attr_valid_until,
        );
        inode.increase_lookup_count();
        inode
    }

    pub(super) fn readdir(
        &self,
        fh: FuseFileHandle,
        offset: usize,
        visitor: &mut dyn DirentVisitor,
    ) -> Result<usize> {
        let fs = self.fs_ref();
        let entries: Vec<VirtioFsDirEntry> =
            fs.conn
                .fuse_readdir(self.nodeid(), fh, offset as u64, FUSE_READDIR_BUF_SIZE)?;

        let offset_read = {
            let try_readdir_fn = |offset: &mut usize,
                                  visitor: &mut dyn DirentVisitor|
             -> Result<()> {
                for entry in &entries {
                    let next_offset = entry.offset() as usize;
                    visitor.visit(entry.name(), entry.ino(), entry.type_().into(), next_offset)?;
                    *offset = next_offset;
                }

                Ok(())
            };

            let mut iterate_offset = offset;
            match try_readdir_fn(&mut iterate_offset, visitor) {
                Err(e) if iterate_offset == offset => Err(e),
                _ => Ok(iterate_offset - offset),
            }?
        };

        self.set_atime(crate::time::clocks::RealTimeCoarseClock::get().read_time());

        Ok(offset_read)
    }

    fn setattr(&self, setattr_in: SetattrIn) -> Result<()> {
        let fs = self.fs_ref();
        let attr_out = fs.conn.fuse_setattr(self.nodeid(), setattr_in)?;

        let metadata = metadata_from_attr(attr_out.attr, fs.sb().container_dev_id);
        self.set_metadata(metadata)?;

        let now = MonotonicCoarseClock::get().read_time();
        *self.attr_valid_until.write() = now.saturating_add(valid_duration(
            attr_out.attr_valid,
            attr_out.attr_valid_nsec,
        ));

        Ok(())
    }
}

impl Drop for VirtioFsInode {
    fn drop(&mut self) {
        self.release_page_cache_fh();
        let nlookup = self.lookup_count.load(Ordering::Relaxed);
        self.release_lookup_count(nlookup);
    }
}

impl PageCacheBackend for VirtioFsInode {
    fn read_page_async(&self, idx: usize, frame: &CachePage) -> Result<BioWaiter> {
        let offset = idx.checked_mul(PAGE_SIZE).ok_or_else(|| {
            Error::with_message(Errno::EOVERFLOW, "virtiofs page offset overflow")
        })?;
        if offset >= self.size() {
            return_errno_with_message!(Errno::EINVAL, "virtiofs page read beyond EOF");
        }

        frame.writer().fill_zeros(frame.size());
        let size = (self.size() - offset).min(PAGE_SIZE).min(u32::MAX as usize) as u32;
        let fs = self.fs_ref();
        let fh = self.get_or_open_page_cache_fh()?;
        let data = fs.conn.fuse_read(self.nodeid(), fh, offset as u64, size)?;
        let mut frame_writer = frame.writer();
        frame_writer.write_fallible(&mut VmReader::from(data.as_slice()).to_fallible())?;
        Ok(BioWaiter::new())
    }

    fn write_page_async(&self, idx: usize, frame: &CachePage) -> Result<BioWaiter> {
        let offset = idx.checked_mul(PAGE_SIZE).ok_or_else(|| {
            Error::with_message(Errno::EOVERFLOW, "virtiofs page offset overflow")
        })?;
        let file_size = self.size();
        if offset >= file_size {
            return Ok(BioWaiter::new());
        }

        let write_len = (file_size - offset).min(PAGE_SIZE);
        let mut data = vec![0u8; write_len];
        let mut writer = VmWriter::from(data.as_mut_slice());
        writer.write_fallible(&mut frame.reader().to_fallible())?;

        let fs = self.fs_ref();
        let fh = self.get_or_open_page_cache_fh()?;
        fs.conn
            .fuse_write(self.nodeid(), fh, offset as u64, &data)?;
        Ok(BioWaiter::new())
    }

    fn npages(&self) -> usize {
        self.size().div_ceil(PAGE_SIZE)
    }
}

// Most regular file and directory I/O goes through `open()`, which returns
// `VirtioFsHandle` or `VirtioFsDirHandle` and lets `InodeHandle` dispatch via
// `FileIo`.
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

        let fs = self.fs_ref();
        let fh = fs
            .conn
            .fuse_open(self.nodeid(), AccessMode::O_RDONLY as u32)?
            .fh;
        let result = self.cached_read_at(offset, writer, fh);
        fs.conn.fuse_release(
            self.nodeid(),
            fh,
            AccessMode::O_RDONLY as u32,
            ReleaseKind::File,
        );
        result
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

        let fs = self.fs_ref();
        let fh = fs
            .conn
            .fuse_open(self.nodeid(), AccessMode::O_WRONLY as u32)?
            .fh;

        let offset = if status_flags.contains(StatusFlags::O_APPEND) {
            self.revalidate_attr(fh)?;
            let size = self.size();
            size
        } else {
            offset
        };

        let result = self.cached_write_at(offset, reader, fh);
        fs.conn.fuse_release(
            self.nodeid(),
            fh,
            AccessMode::O_WRONLY as u32,
            ReleaseKind::File,
        );
        result
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

        let setattr_in = SetattrIn {
            valid: SetattrValid::FATTR_SIZE,
            size,
            ..SetattrIn::default()
        };
        self.setattr(setattr_in)
    }

    fn metadata(&self) -> Metadata {
        // TODO: Make stat-like queries revalidate attributes before returning
        // this cached metadata. This likely needs a separate fallible metadata
        // query path so internal callers can keep using the local snapshot
        // without revalidation.
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
        let mode_bits = u32::from(self.type_()) | u32::from(mode.bits());
        let setattr_in = SetattrIn {
            valid: SetattrValid::FATTR_MODE,
            mode: mode_bits,
            ..SetattrIn::default()
        };
        self.setattr(setattr_in)
    }

    fn owner(&self) -> Result<Uid> {
        Ok(self.metadata.read().uid)
    }

    fn set_owner(&self, uid: Uid) -> Result<()> {
        let setattr_in = SetattrIn {
            valid: SetattrValid::FATTR_UID,
            uid: uid.into(),
            ..SetattrIn::default()
        };
        self.setattr(setattr_in)
    }

    fn group(&self) -> Result<Gid> {
        Ok(self.metadata.read().gid)
    }

    fn set_group(&self, gid: Gid) -> Result<()> {
        let setattr_in = SetattrIn {
            valid: SetattrValid::FATTR_GID,
            gid: gid.into(),
            ..SetattrIn::default()
        };
        self.setattr(setattr_in)
    }

    fn atime(&self) -> Duration {
        self.metadata.read().last_access_at
    }

    fn set_atime(&self, time: Duration) {
        let setattr_in = SetattrIn {
            valid: SetattrValid::FATTR_ATIME,
            atime: time.as_secs(),
            atimensec: time.subsec_nanos(),
            ..SetattrIn::default()
        };
        if let Err(err) = self.setattr(setattr_in) {
            warn!(
                "virtiofs set_atime failed for inode {}: {:?}",
                self.nodeid(),
                err
            );
        }
    }

    fn mtime(&self) -> Duration {
        self.metadata.read().last_modify_at
    }

    fn set_mtime(&self, time: Duration) {
        let setattr_in = SetattrIn {
            valid: SetattrValid::FATTR_MTIME,
            mtime: time.as_secs(),
            mtimensec: time.subsec_nanos(),
            ..SetattrIn::default()
        };
        if let Err(err) = self.setattr(setattr_in) {
            warn!(
                "virtiofs set_mtime failed for inode {}: {:?}",
                self.nodeid(),
                err
            );
        }
    }

    fn ctime(&self) -> Duration {
        self.metadata.read().last_meta_change_at
    }

    fn set_ctime(&self, time: Duration) {
        let setattr_in = SetattrIn {
            valid: SetattrValid::FATTR_CTIME,
            ctime: time.as_secs(),
            ctimensec: time.subsec_nanos(),
            ..SetattrIn::default()
        };
        if let Err(err) = self.setattr(setattr_in) {
            warn!(
                "virtiofs set_ctime failed for inode {}: {:?}",
                self.nodeid(),
                err
            );
        }
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
        let inode_type = self.metadata.read().type_;
        match inode_type {
            InodeType::File => Some(
                self.open_handle(access_mode)
                    .map(|handle| Box::new(handle) as Box<dyn FileIo>),
            ),
            InodeType::Dir => Some(
                self.open_dir_handle()
                    .map(|handle| Box::new(handle) as Box<dyn FileIo>),
            ),
            _ => None,
        }
    }

    fn lookup(&self, name: &str) -> Result<Arc<dyn Inode>> {
        let fs = self.fs_ref();
        let parent_nodeid = self.nodeid();
        let entry_out = fs.conn.fuse_lookup(parent_nodeid, name)?;
        let nodeid = entry_out.nodeid;

        let now = MonotonicCoarseClock::get().read_time();

        let entry_valid_until = Some(now.saturating_add(valid_duration(
            entry_out.entry_valid,
            entry_out.entry_valid_nsec,
        )));
        let attr_valid_until = now.saturating_add(valid_duration(
            entry_out.attr_valid,
            entry_out.attr_valid_nsec,
        ));

        let inode = VirtioFsInode::new(
            nodeid,
            metadata_from_attr(entry_out.attr, fs.sb().container_dev_id),
            Arc::downgrade(&fs),
            entry_valid_until,
            attr_valid_until,
        );
        inode.increase_lookup_count();

        Ok(inode)
    }

    fn create(&self, name: &str, type_: InodeType, mode: InodeMode) -> Result<Arc<dyn Inode>> {
        let fs = self.fs_ref();
        let parent_nodeid = self.nodeid();
        let entry_out = match type_ {
            InodeType::File => {
                let (entry_out, open_out) = fs.conn.fuse_create(
                    parent_nodeid,
                    name,
                    u32::from(InodeType::File) | u32::from(mode.bits()),
                )?;
                fs.conn.fuse_release(
                    entry_out.nodeid,
                    open_out.fh,
                    AccessMode::O_RDWR as u32,
                    ReleaseKind::File,
                );
                entry_out
            }
            InodeType::Dir => fs.conn.fuse_mkdir(
                parent_nodeid,
                name,
                u32::from(InodeType::Dir) | u32::from(mode.bits()),
            )?,
            InodeType::Socket => fs.conn.fuse_mknod(
                parent_nodeid,
                name,
                u32::from(InodeType::Socket) | u32::from(mode.bits()),
                0,
            )?,
            _ => {
                return_errno_with_message!(
                    Errno::EOPNOTSUPP,
                    "virtiofs create supports file/dir/socket only"
                )
            }
        };
        let attr_out = FuseAttrOut {
            attr_valid: entry_out.attr_valid,
            attr_valid_nsec: entry_out.attr_valid_nsec,
            dummy: 0,
            attr: entry_out.attr,
        };

        Ok(self.build_child_inode(&fs, entry_out, attr_out))
    }

    fn link(&self, old: &Arc<dyn Inode>, name: &str) -> Result<()> {
        let old = old
            .downcast_ref::<VirtioFsInode>()
            .ok_or_else(|| Error::with_message(Errno::EXDEV, "not same fs"))?;

        let fs = self.fs_ref();
        let entry_out = fs.conn.fuse_link(old.nodeid(), self.nodeid(), name)?;
        old.increase_lookup_count();

        let metadata = metadata_from_attr(entry_out.attr, fs.sb().container_dev_id);
        old.set_metadata(metadata)?;

        let now = MonotonicCoarseClock::get().read_time();
        *old.attr_valid_until.write() = now.saturating_add(valid_duration(
            entry_out.attr_valid,
            entry_out.attr_valid_nsec,
        ));

        Ok(())
    }

    fn unlink(&self, name: &str) -> Result<()> {
        let fs = self.fs_ref();
        fs.conn.fuse_unlink(self.nodeid(), name)?;
        Ok(())
    }

    fn rmdir(&self, name: &str) -> Result<()> {
        let fs = self.fs_ref();
        fs.conn.fuse_rmdir(self.nodeid(), name)?;
        Ok(())
    }

    fn readdir_at(&self, offset: usize, visitor: &mut dyn DirentVisitor) -> Result<usize> {
        let fs = self.fs_ref();
        let fh = fs.conn.fuse_opendir(self.nodeid())?;
        let result = self.readdir(fh, offset, visitor);
        fs.conn
            .fuse_release(self.nodeid(), fh, 0, ReleaseKind::Directory);
        result
    }

    fn sync_data(&self) -> Result<()> {
        self.flush_page_cache()
    }

    fn fs(&self) -> Arc<dyn FileSystem> {
        self.fs_ref()
    }

    fn read_link(&self) -> Result<SymbolicLink> {
        if self.metadata().type_ != InodeType::SymLink {
            return_errno_with_message!(Errno::EINVAL, "read_link on non-symlink")
        }

        let fs = self.fs_ref();
        let target = fs.conn.fuse_readlink(self.nodeid())?;

        Ok(SymbolicLink::Plain(target))
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
impl From<DirentType> for InodeType {
    fn from(type_: DirentType) -> Self {
        match type_ {
            DirentType::DT_DIR => InodeType::Dir,
            DirentType::DT_REG => InodeType::File,
            DirentType::DT_LNK => InodeType::SymLink,
            DirentType::DT_CHR => InodeType::CharDevice,
            DirentType::DT_BLK => InodeType::BlockDevice,
            DirentType::DT_FIFO => InodeType::NamedPipe,
            DirentType::DT_SOCK => InodeType::Socket,
            DirentType::DT_UNKNOWN => InodeType::Unknown,
        }
    }
}
