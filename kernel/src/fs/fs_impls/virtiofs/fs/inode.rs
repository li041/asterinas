// SPDX-License-Identifier: MPL-2.0

//! Inode implementation for `virtiofs`.
//!
//! This module defines [`VirtioFsInode`], which owns cached metadata, optional
//! page-cache state, and the inode operations backed by FUSE requests.

use alloc::sync::{Arc, Weak};
use core::time::Duration;

use aster_block::bio::BioWaiter;
use aster_fuse::{
    Attr, DirentType, EntryOut, FuseAttrOut, FuseDirEntry, FuseFileHandle, FuseNodeId,
    FuseOpenFlags, GetattrFlags, LookupCount, ReleaseFlags, ReleaseKind, SetattrIn, SetattrValid,
    WriteFlags,
};
use aster_virtio::device::filesystem::device::AttrVersion;
use ostd::{
    mm::{HasSize, VmReader, VmWriter, io::util::HasVmReaderWriter},
    warn,
};

use super::{
    super::metadata_from_attr,
    FUSE_READDIR_BUF_SIZE, VirtioFs,
    dir::VirtioFsDir,
    file::{CachePolicy, VirtioFsFile},
    open_handle::{OpenHandles, VirtioFsOpenHandle},
    valid_until,
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
    time::clocks::{MonotonicCoarseClock, RealTimeCoarseClock},
};

/// An inode backed by a FUSE server over virtio-fs.
pub(super) struct VirtioFsInode {
    nodeid: FuseNodeId,
    type_: InodeType,
    lookup_count: LookupCount,
    inner: RwMutex<InodeInner>,
    // TODO: Move the entry timeout state to [`Dentry`] once the VFS can carry
    // filesystem-specific per-dentry data.
    entry_valid_until: Mutex<Duration>,
    page_cache: Option<PageCache>,
    /// Real open handles that can be used by page-cache backend I/O.
    //
    /// [`PageCacheBackend`] callbacks do not carry the [`VirtioFsOpenHandle`] that
    /// caused a cache miss or writeback. Keeping weak references to real
    /// server-issued handles here lets page-cache I/O use an existing open handle.
    open_handles: OpenHandles,
    fs: Weak<VirtioFs>,
    extension: Extension,
    weak_self: Weak<Self>,
}

struct InodeInner {
    metadata: Metadata,
    attr_valid_until: Duration,
    attr_version: AttrVersion,
}

#[derive(Clone, Copy)]
enum MetadataChange {
    Setattr(SetattrValid),
    Link,
}

impl InodeInner {
    fn is_attr_valid(&self, now: Duration) -> bool {
        now < self.attr_valid_until
    }

    fn accepts_attr_version(&self, incoming: AttrVersion) -> bool {
        incoming >= self.attr_version
    }
}

impl VirtioFsInode {
    /// Creates a new `VirtioFsInode`.
    ///
    /// Regular files automatically get a page cache;
    /// Directories and other types do not.
    pub(super) fn new(
        nodeid: FuseNodeId,
        metadata: Metadata,
        fs: Weak<VirtioFs>,
        entry_valid_until: Duration,
        attr_valid_until: Duration,
    ) -> Arc<Self> {
        Arc::new_cyclic(|weak_self| Self {
            nodeid,
            type_: metadata.type_,
            lookup_count: LookupCount::new(),
            page_cache: metadata
                .type_
                .is_regular_file()
                .then(|| PageCache::with_capacity(metadata.size, weak_self.clone() as _).unwrap()),
            inner: RwMutex::new(InodeInner {
                metadata,
                attr_valid_until,
                attr_version: AttrVersion::INITIAL,
            }),
            entry_valid_until: Mutex::new(entry_valid_until),
            open_handles: OpenHandles::new(),
            fs,
            extension: Extension::new(),
            weak_self: weak_self.clone(),
        })
    }

    /// Returns a strong reference to the parent `VirtioFs`.
    pub(super) fn fs_ref(&self) -> Arc<VirtioFs> {
        self.fs.upgrade().unwrap()
    }

    /// Returns the FUSE node ID of this inode.
    pub(super) fn nodeid(&self) -> FuseNodeId {
        self.nodeid
    }

    /// Returns the cached file size.
    pub(super) fn size(&self) -> usize {
        self.inner.read().metadata.size
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

    fn revalidate_lookup(&self, parent_nodeid: FuseNodeId, name: &str) -> Result<()> {
        let mut entry_valid_until = self.entry_valid_until.lock();

        let now = MonotonicCoarseClock::get().read_time();
        if now < *entry_valid_until {
            return Ok(());
        }

        let old_nodeid = self.nodeid();
        let fs = self.fs_ref();
        let attr_version = fs.conn.current_attr_version();
        let entry_out = fs.conn.fuse_lookup(parent_nodeid, name)?;

        if entry_out.nodeid != old_nodeid {
            // The returned entry refers to a different inode. Drop the lookup
            // reference immediately so we don't leak nlookup on that node.
            fs.conn.fuse_forget(entry_out.nodeid, 1);
            return_errno_with_message!(Errno::ESTALE, "virtiofs stale dentry after revalidate");
        }

        // Count only lookups that still point to this inode.
        self.lookup_count.increase();

        self.update_metadata_if_fresh(
            entry_out.attr,
            entry_out.attr_valid,
            entry_out.attr_valid_nsec,
            attr_version,
            &fs,
        )?;

        let now = MonotonicCoarseClock::get().read_time();
        *entry_valid_until = valid_until(now, entry_out.entry_valid, entry_out.entry_valid_nsec);

        Ok(())
    }

    /// Refreshes cached attributes from the server if the current cache has expired.
    pub(super) fn revalidate_attr(&self, fh: FuseFileHandle) -> Result<()> {
        let now = MonotonicCoarseClock::get().read_time();
        if self.inner.read().is_attr_valid(now) {
            return Ok(());
        }

        let fs = self.fs_ref();
        let attr_version = fs.conn.current_attr_version();
        let attr_out = fs
            .conn
            .fuse_getattr(self.nodeid(), GetattrFlags::GETATTR_FH, fh)?;

        self.update_metadata_if_fresh(
            attr_out.attr,
            attr_out.attr_valid,
            attr_out.attr_valid_nsec,
            attr_version,
            &fs,
        )?;

        Ok(())
    }

    fn invalidate_page_cache(&self, new_size: usize) -> Result<()> {
        let Some(page_cache) = &self.page_cache else {
            return Ok(());
        };

        let cached_size = page_cache.pages().size();
        if cached_size > 0 {
            // Dirty cache pages are laundered before they are removed from the page cache,
            // instead of being silently dropped.
            page_cache.evict_range(0..cached_size)?;
            page_cache.resize(0)?;
        }
        page_cache.resize(new_size)?;

        Ok(())
    }

    fn update_metadata_if_fresh(
        &self,
        attr: Attr,
        attr_valid: u64,
        attr_valid_nsec: u32,
        attr_version: AttrVersion,
        fs: &VirtioFs,
    ) -> Result<()> {
        if !self.inner.read().accepts_attr_version(attr_version) {
            return Ok(());
        }

        let metadata = metadata_from_attr(attr, fs.sb().container_dev_id);
        let now = MonotonicCoarseClock::get().read_time();

        // Commit the metadata under the write lock, then invalidate the page
        // cache after releasing it. Page-cache eviction can call back into
        // this inode to read the current size.
        let should_invalidate = {
            let mut inner = self.inner.write();
            if !inner.accepts_attr_version(attr_version) {
                return Ok(());
            }
            self.commit_metadata_locked(
                &mut inner,
                metadata,
                valid_until(now, attr_valid, attr_valid_nsec),
                fs,
            )
        };

        if should_invalidate {
            self.invalidate_page_cache(self.size())?;
        }

        Ok(())
    }

    fn update_metadata_after_change(
        &self,
        attr: Attr,
        attr_valid: u64,
        attr_valid_nsec: u32,
        attr_version: AttrVersion,
        change: MetadataChange,
        fs: &VirtioFs,
    ) -> Result<()> {
        let metadata = metadata_from_attr(attr, fs.sb().container_dev_id);
        let now = MonotonicCoarseClock::get().read_time();

        let should_invalidate = {
            let mut inner = self.inner.write();
            // A successful metadata-changing request must be reflected locally.
            // If another update committed while the request was in flight, do
            // not refresh unrelated cached fields from this reply.
            if inner.accepts_attr_version(attr_version) {
                self.commit_metadata_locked(
                    &mut inner,
                    metadata,
                    valid_until(now, attr_valid, attr_valid_nsec),
                    fs,
                )
            } else {
                self.commit_metadata_change_locked(&mut inner, metadata, now, change, fs)
            }
        };

        if should_invalidate {
            self.invalidate_page_cache(self.size())?;
        }

        Ok(())
    }

    fn commit_metadata_locked(
        &self,
        inner: &mut InodeInner,
        metadata: Metadata,
        attr_valid_until: Duration,
        fs: &VirtioFs,
    ) -> bool {
        let old_metadata = inner.metadata;
        let should_invalidate = self.page_cache.is_some()
            && (old_metadata.size != metadata.size
                || old_metadata.last_modify_at != metadata.last_modify_at);
        inner.metadata = metadata;
        inner.attr_valid_until = attr_valid_until;
        inner.attr_version = fs.conn.bump_attr_version();
        should_invalidate
    }

    fn commit_metadata_change_locked(
        &self,
        inner: &mut InodeInner,
        metadata: Metadata,
        attr_valid_until: Duration,
        change: MetadataChange,
        fs: &VirtioFs,
    ) -> bool {
        let old_metadata = inner.metadata;

        match change {
            MetadataChange::Setattr(valid) => {
                if valid.contains(SetattrValid::FATTR_MODE) {
                    inner.metadata.type_ = metadata.type_;
                    inner.metadata.mode = metadata.mode;
                }
                if valid.contains(SetattrValid::FATTR_UID) {
                    inner.metadata.uid = metadata.uid;
                }
                if valid.contains(SetattrValid::FATTR_GID) {
                    inner.metadata.gid = metadata.gid;
                }
                if valid.contains(SetattrValid::FATTR_SIZE) {
                    inner.metadata.size = metadata.size;
                    inner.metadata.nr_sectors_allocated = metadata.nr_sectors_allocated;
                }
                if valid.intersects(SetattrValid::FATTR_ATIME | SetattrValid::FATTR_ATIME_NOW) {
                    inner.metadata.last_access_at = metadata.last_access_at;
                }
                if valid.intersects(SetattrValid::FATTR_MTIME | SetattrValid::FATTR_MTIME_NOW) {
                    inner.metadata.last_modify_at = metadata.last_modify_at;
                }
                if valid.intersects(
                    SetattrValid::FATTR_MODE
                        | SetattrValid::FATTR_UID
                        | SetattrValid::FATTR_GID
                        | SetattrValid::FATTR_SIZE
                        | SetattrValid::FATTR_ATIME
                        | SetattrValid::FATTR_MTIME
                        | SetattrValid::FATTR_ATIME_NOW
                        | SetattrValid::FATTR_MTIME_NOW
                        | SetattrValid::FATTR_CTIME,
                ) {
                    inner.metadata.last_meta_change_at = metadata.last_meta_change_at;
                }
            }
            MetadataChange::Link => {
                inner.metadata.nr_hard_links = metadata.nr_hard_links;
                inner.metadata.last_meta_change_at = metadata.last_meta_change_at;
            }
        }

        inner.attr_valid_until = attr_valid_until;
        inner.attr_version = fs.conn.bump_attr_version();

        self.page_cache.is_some()
            && (old_metadata.size != inner.metadata.size
                || old_metadata.last_modify_at != inner.metadata.last_modify_at)
    }

    /// Writes back all dirty pages in the page cache (if any) to the FUSE server.
    pub(super) fn flush_page_cache(&self) -> Result<()> {
        let Some(page_cache) = &self.page_cache else {
            return Ok(());
        };

        page_cache.evict_range(0..self.size())?;
        Ok(())
    }

    /// Reads from the inode through the page cache, falling back to direct I/O
    /// when no page cache is present.
    pub(super) fn cached_read_at(
        &self,
        offset: usize,
        writer: &mut VmWriter,
        fh: FuseFileHandle,
        flags: u32,
    ) -> Result<usize> {
        self.revalidate_attr(fh)?;

        let Some(page_cache) = &self.page_cache else {
            return self.direct_read_at(offset, writer, fh, flags);
        };

        let file_size = self.size();
        let start = file_size.min(offset);
        let end = file_size.min(offset.saturating_add(writer.avail()));
        let read_len = end - start;
        page_cache.pages().read(start, writer)?;
        Ok(read_len)
    }

    /// Reads from the inode by issuing a `FUSE_READ` request directly to the server.
    pub(super) fn direct_read_at(
        &self,
        offset: usize,
        writer: &mut VmWriter,
        fh: FuseFileHandle,
        flags: u32,
    ) -> Result<usize> {
        self.revalidate_attr(fh)?;

        let file_size = self.size();
        let start = file_size.min(offset);
        let end = file_size.min(offset.saturating_add(writer.avail()));
        let read_len = end - start;
        let max_len = read_len.min(u32::MAX as usize) as u32;
        let copied = self.fs_ref().conn.fuse_read(
            self.nodeid(),
            fh,
            start as u64,
            max_len,
            flags,
            writer,
        )?;
        Ok(copied)
    }

    /// Writes to the inode, sending data to the server and discarding overlapping
    /// cached pages so subsequent reads refetch authoritative data.
    pub(super) fn cached_write_at(
        &self,
        offset: usize,
        reader: &mut VmReader,
        fh: FuseFileHandle,
        flags: u32,
    ) -> Result<usize> {
        let Some(page_cache) = &self.page_cache else {
            return self.direct_write_at(offset, reader, fh, flags);
        };

        // TODO: Support `FUSE_WRITEBACK_CACHE`.
        //
        // `FuseConnection::init_flags` does not currently request the flag,
        // so writeback caching is not negotiated.
        //
        // Until writeback is implemented, every write is sent to the daemon
        // synchronously. Cached pages overlapping the written range are then
        // discarded so subsequent reads refetch the authoritative bytes from
        // the daemon.
        // Reference: <https://elixir.bootlin.com/linux/v6.18/source/fs/fuse/file.c#L1261>.
        let written = self.fs_ref().conn.fuse_write(
            self.nodeid(),
            fh,
            offset as u64,
            flags,
            WriteFlags::empty(),
            reader,
        )?;

        let new_size = offset
            .checked_add(written)
            .ok_or_else(|| Error::with_message(Errno::EOVERFLOW, "virtiofs write size overflow"))?;
        self.commit_local_write(new_size);

        if new_size > page_cache.pages().size() {
            page_cache.resize(new_size)?;
        }
        page_cache.discard_range(offset..new_size);

        Ok(written)
    }

    /// Writes to the inode by issuing a `FUSE_WRITE` request directly to the server,
    /// then invalidates the entire page cache.
    pub(super) fn direct_write_at(
        &self,
        offset: usize,
        reader: &mut VmReader,
        fh: FuseFileHandle,
        flags: u32,
    ) -> Result<usize> {
        let written = self.fs_ref().conn.fuse_write(
            self.nodeid(),
            fh,
            offset as u64,
            flags,
            WriteFlags::empty(),
            reader,
        )?;

        let new_size = offset
            .checked_add(written)
            .ok_or_else(|| Error::with_message(Errno::EOVERFLOW, "virtiofs write size overflow"))?;
        self.commit_local_write(new_size);

        self.invalidate_page_cache(self.size())?;

        Ok(written)
    }

    fn commit_local_write(&self, new_size: usize) {
        let fs = self.fs_ref();
        let now = RealTimeCoarseClock::get().read_time();

        let mut inner = self.inner.write();
        inner.metadata.size = inner.metadata.size.max(new_size);
        inner.metadata.nr_sectors_allocated = inner.metadata.size.div_ceil(512);
        inner.metadata.last_modify_at = now;
        inner.metadata.last_meta_change_at = now;
        // A local write has already changed the server-visible attributes.
        // Expire the cache and publish a newer version so in-flight attribute
        // refreshes cannot overwrite the local size or timestamps.
        inner.attr_valid_until = MonotonicCoarseClock::get().read_time();
        inner.attr_version = fs.conn.bump_attr_version();
    }

    fn do_open(
        &self,
        access_mode: AccessMode,
        status_flags: StatusFlags,
    ) -> Result<Box<dyn FileIo>> {
        let inode = self
            .weak_self
            .upgrade()
            .ok_or_else(|| Error::with_message(Errno::EIO, "virtiofs inode is unavailable"))?;
        let fs = self.fs_ref();
        match self.type_ {
            InodeType::File => {
                let open_out = fs.conn.fuse_open(self.nodeid(), access_mode as u32)?;
                let cache_policy = if self.page_cache.is_some()
                    && !open_out.open_flags.contains(FuseOpenFlags::FOPEN_DIRECT_IO)
                {
                    CachePolicy::Cached
                } else {
                    CachePolicy::Direct
                };
                if !open_out
                    .open_flags
                    .contains(FuseOpenFlags::FOPEN_KEEP_CACHE)
                {
                    self.invalidate_page_cache(self.size())?;
                }
                let open_handle = VirtioFsOpenHandle::new(
                    open_out.fh,
                    self.nodeid(),
                    access_mode,
                    status_flags,
                    open_out.open_flags,
                    self.fs.clone(),
                    ReleaseKind::File,
                );
                if cache_policy == CachePolicy::Cached {
                    self.open_handles.insert(&open_handle);
                }
                Ok(Box::new(VirtioFsFile::new(
                    inode,
                    open_handle,
                    cache_policy,
                )))
            }
            InodeType::Dir => {
                let open_out = fs.conn.fuse_opendir(self.nodeid())?;
                let open_handle = VirtioFsOpenHandle::new(
                    open_out.fh,
                    self.nodeid(),
                    access_mode,
                    status_flags,
                    open_out.open_flags,
                    self.fs.clone(),
                    ReleaseKind::Directory,
                );
                Ok(Box::new(VirtioFsDir::new(inode, open_handle)))
            }
            _ => unreachable!("do_open called with non-file/dir inode type"),
        }
    }

    fn build_child_inode(
        &self,
        fs: &Arc<VirtioFs>,
        entry_out: EntryOut,
        attr_out: FuseAttrOut,
    ) -> Arc<VirtioFsInode> {
        let now = MonotonicCoarseClock::get().read_time();
        let entry_valid_until = valid_until(now, entry_out.entry_valid, entry_out.entry_valid_nsec);
        let attr_valid_until = valid_until(now, attr_out.attr_valid, attr_out.attr_valid_nsec);

        VirtioFsInode::new(
            entry_out.nodeid,
            metadata_from_attr(attr_out.attr, fs.sb().container_dev_id),
            Arc::downgrade(fs),
            entry_valid_until,
            attr_valid_until,
        )
    }

    pub(super) fn readdir(
        &self,
        fh: FuseFileHandle,
        offset: usize,
        flags: u32,
        visitor: &mut dyn DirentVisitor,
    ) -> Result<usize> {
        let fs = self.fs_ref();
        let entries: Vec<FuseDirEntry> = fs.conn.fuse_readdir(
            self.nodeid(),
            fh,
            offset as u64,
            FUSE_READDIR_BUF_SIZE,
            flags,
        )?;

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

        self.set_atime(RealTimeCoarseClock::get().read_time());

        Ok(offset_read)
    }

    fn setattr(&self, setattr_in: SetattrIn) -> Result<()> {
        let fs = self.fs_ref();
        let attr_version = fs.conn.current_attr_version();
        let valid = setattr_in.valid;
        let attr_out = fs.conn.fuse_setattr(self.nodeid(), setattr_in)?;

        self.update_metadata_after_change(
            attr_out.attr,
            attr_out.attr_valid,
            attr_out.attr_valid_nsec,
            attr_version,
            MetadataChange::Setattr(valid),
            &fs,
        )?;

        Ok(())
    }
}

impl Drop for VirtioFsInode {
    fn drop(&mut self) {
        let nlookup = self.lookup_count.get();
        if nlookup > 0 {
            self.forget_async(nlookup);
        }
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
        let mut frame_writer = frame.writer().to_fallible();
        let fs = self.fs_ref();
        self.open_handles.with_readable_handle(|open_handle| {
            fs.conn.fuse_read(
                self.nodeid(),
                open_handle.fh(),
                offset as u64,
                size,
                open_handle.file_flags(),
                &mut frame_writer,
            )?;
            Ok(())
        })?;
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

        let mut reader = frame.reader().to_fallible();
        reader.limit(file_size - offset);

        let fs = self.fs_ref();
        self.open_handles.with_writable_handle(|open_handle| {
            fs.conn.fuse_write(
                self.nodeid(),
                open_handle.fh(),
                offset as u64,
                open_handle.file_flags(),
                WriteFlags::WRITE_CACHE,
                &mut reader,
            )?;
            Ok(())
        })?;
        Ok(BioWaiter::new())
    }

    fn npages(&self) -> usize {
        self.size().div_ceil(PAGE_SIZE)
    }
}

// Regular file and directory I/O must go through `FileIo`.
impl InodeIo for VirtioFsInode {
    fn read_at(
        &self,
        _offset: usize,
        _writer: &mut VmWriter,
        _status_flags: StatusFlags,
    ) -> Result<usize> {
        return_errno_with_message!(
            Errno::EBADF,
            "virtiofs inode I/O requires an open file handle"
        );
    }

    fn write_at(
        &self,
        _offset: usize,
        _reader: &mut VmReader,
        _status_flags: StatusFlags,
    ) -> Result<usize> {
        return_errno_with_message!(
            Errno::EBADF,
            "virtiofs inode I/O requires an open file handle"
        );
    }
}

impl Inode for VirtioFsInode {
    fn size(&self) -> usize {
        self.inner.read().metadata.size
    }

    fn resize(&self, new_size: usize) -> Result<()> {
        if self.type_ != InodeType::File {
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
        self.inner.read().metadata
    }

    fn ino(&self) -> u64 {
        self.nodeid().as_u64()
    }

    fn type_(&self) -> InodeType {
        self.type_
    }

    fn mode(&self) -> Result<InodeMode> {
        Ok(self.inner.read().metadata.mode)
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
        Ok(self.inner.read().metadata.uid)
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
        Ok(self.inner.read().metadata.gid)
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
        self.inner.read().metadata.last_access_at
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
                self.nodeid().as_u64(),
                err
            );
        }
    }

    fn mtime(&self) -> Duration {
        self.inner.read().metadata.last_modify_at
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
                self.nodeid().as_u64(),
                err
            );
        }
    }

    fn ctime(&self) -> Duration {
        self.inner.read().metadata.last_meta_change_at
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
                self.nodeid().as_u64(),
                err
            );
        }
    }

    fn open(
        &self,
        access_mode: AccessMode,
        status_flags: StatusFlags,
    ) -> Option<Result<Box<dyn FileIo>>> {
        if !matches!(self.type_, InodeType::File | InodeType::Dir) {
            return None;
        }
        Some(self.do_open(access_mode, status_flags))
    }

    fn lookup(&self, name: &str) -> Result<Arc<dyn Inode>> {
        let fs = self.fs_ref();
        let parent_nodeid = self.nodeid();
        let entry_out = fs.conn.fuse_lookup(parent_nodeid, name)?;
        let nodeid = entry_out.nodeid;

        let now = MonotonicCoarseClock::get().read_time();

        let entry_valid_until = valid_until(now, entry_out.entry_valid, entry_out.entry_valid_nsec);
        let attr_valid_until = valid_until(now, entry_out.attr_valid, entry_out.attr_valid_nsec);

        let inode = VirtioFsInode::new(
            nodeid,
            metadata_from_attr(entry_out.attr, fs.sb().container_dev_id),
            Arc::downgrade(&fs),
            entry_valid_until,
            attr_valid_until,
        );

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
                    ReleaseFlags::empty(),
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
        let attr_version = fs.conn.current_attr_version();
        let entry_out = fs.conn.fuse_link(old.nodeid(), self.nodeid(), name)?;
        old.lookup_count.increase();

        old.update_metadata_after_change(
            entry_out.attr,
            entry_out.attr_valid,
            entry_out.attr_valid_nsec,
            attr_version,
            MetadataChange::Link,
            &fs,
        )?;

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
        let open_out = fs.conn.fuse_opendir(self.nodeid())?;
        let mut open_flags = open_out.open_flags;
        open_flags.remove(FuseOpenFlags::FOPEN_DIRECT_IO);
        if !open_flags.contains(FuseOpenFlags::FOPEN_KEEP_CACHE) {
            self.invalidate_page_cache(self.size())?;
        }
        let result = self.readdir(open_out.fh, offset, AccessMode::O_RDWR as u32, visitor);
        fs.conn.fuse_release(
            self.nodeid(),
            open_out.fh,
            0,
            ReleaseFlags::empty(),
            ReleaseKind::Directory,
        );
        result
    }

    fn sync_data(&self) -> Result<()> {
        self.flush_page_cache()
    }

    fn fs(&self) -> Arc<dyn FileSystem> {
        self.fs_ref()
    }

    fn read_link(&self) -> Result<SymbolicLink> {
        if self.type_ != InodeType::SymLink {
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

        let parent = parent
            .inode()
            .downcast_ref::<VirtioFsInode>()
            .ok_or_else(|| Error::with_message(Errno::EXDEV, "not same fs"))?;

        self.revalidate_lookup(parent.nodeid(), name)
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
