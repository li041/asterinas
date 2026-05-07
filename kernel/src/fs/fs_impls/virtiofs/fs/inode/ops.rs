// SPDX-License-Identifier: MPL-2.0

//! Methods and constructors for `VirtioFsInode`.

use alloc::sync::Arc;

use aster_fuse::{
    EntryOut, FuseAttrOut, FuseDirEntry, FuseFileHandle, FuseOpenFlags, GetattrFlags, ReleaseFlags,
    ReleaseKind, SetattrIn, WriteFlags,
};
use ostd::mm::{VmReader, VmWriter};

use super::{
    super::{
        super::metadata_from_attr,
        FUSE_READDIR_BUF_SIZE, VirtioFs,
        dir::VirtioFsDir,
        file::{CachePolicy, VirtioFsFile},
        open_handle::VirtioFsOpenHandle,
        valid_until,
    },
    VirtioFsInode,
};
use crate::{
    fs::{
        file::{AccessMode, FileIo, InodeType, StatusFlags},
        fs_impls::virtiofs::fs::inode::MetadataUpdate,
        utils::DirentVisitor,
        vfs::file_system::FileSystem,
    },
    prelude::*,
    thread::work_queue::{self, WorkPriority},
    time::clocks::MonotonicCoarseClock,
};

impl VirtioFsInode {
    pub(in super::super) fn cached_read_at(
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

    pub(in super::super) fn direct_read_at(
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
        let copied =
            self.fs_ref()
                .conn
                .read(self.nodeid(), fh, start as u64, max_len, flags, writer)?;
        Ok(copied)
    }

    pub(in super::super) fn read_with_transient_handle(
        &self,
        offset: usize,
        writer: &mut VmWriter,
    ) -> Result<usize> {
        let fs = self.fs_ref();
        let flags = AccessMode::O_RDONLY as u32;
        let open_out = fs.conn.open(self.nodeid(), flags)?;
        let ret = self.direct_read_at(offset, writer, open_out.fh(), flags);
        fs.conn.release(
            self.nodeid(),
            open_out.fh(),
            flags,
            ReleaseFlags::empty(),
            ReleaseKind::File,
        );
        ret
    }

    pub(in super::super) fn cached_write_at(
        &self,
        offset: usize,
        reader: &mut VmReader,
        fh: FuseFileHandle,
        flags: u32,
    ) -> Result<usize> {
        let Some(page_cache) = &self.page_cache else {
            return self.direct_write_at(offset, reader, fh, flags);
        };

        let written = self.fs_ref().conn.write(
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

    pub(in super::super) fn direct_write_at(
        &self,
        offset: usize,
        reader: &mut VmReader,
        fh: FuseFileHandle,
        flags: u32,
    ) -> Result<usize> {
        let written = self.fs_ref().conn.write(
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

    pub(in super::super) fn write_with_transient_handle(
        &self,
        offset: usize,
        reader: &mut VmReader,
    ) -> Result<usize> {
        let fs = self.fs_ref();
        let flags = AccessMode::O_RDWR as u32;
        let open_out = fs.conn.open(self.nodeid(), flags)?;
        let ret = self.direct_write_at(offset, reader, open_out.fh(), flags);
        fs.conn.release(
            self.nodeid(),
            open_out.fh(),
            flags,
            ReleaseFlags::empty(),
            ReleaseKind::File,
        );
        ret
    }

    pub(in super::super) fn open(
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
                let open_out = fs.conn.open(self.nodeid(), access_mode as u32)?;
                let cache_policy = if self.page_cache.is_some()
                    && !open_out
                        .open_flags()
                        .contains(FuseOpenFlags::FOPEN_DIRECT_IO)
                {
                    CachePolicy::Cached
                } else {
                    CachePolicy::Direct
                };
                let open_handle = VirtioFsOpenHandle::new(
                    open_out.fh(),
                    self.nodeid(),
                    access_mode,
                    status_flags,
                    open_out.open_flags(),
                    self.fs.clone(),
                    ReleaseKind::File,
                );
                if !open_out
                    .open_flags()
                    .contains(FuseOpenFlags::FOPEN_KEEP_CACHE)
                    && let Err(err) = self.invalidate_page_cache(self.size())
                {
                    open_handle.release();
                    return Err(err);
                }
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
                let open_out = fs.conn.opendir(self.nodeid())?;
                let open_handle = VirtioFsOpenHandle::new(
                    open_out.fh(),
                    self.nodeid(),
                    access_mode,
                    status_flags,
                    open_out.open_flags(),
                    self.fs.clone(),
                    ReleaseKind::Directory,
                );
                Ok(Box::new(VirtioFsDir::new(inode, open_handle)))
            }
            _ => Err(Error::with_message(
                Errno::EOPNOTSUPP,
                "we only supports opening regular files and directories now",
            )),
        }
    }

    pub(in super::super) fn build_child_inode(
        &self,
        fs: &Arc<VirtioFs>,
        entry_out: EntryOut,
        attr_out: FuseAttrOut,
    ) -> Arc<VirtioFsInode> {
        let now = MonotonicCoarseClock::get().read_time();
        let entry_valid_until =
            valid_until(now, entry_out.entry_valid(), entry_out.entry_valid_nsec());
        let attr_valid_until = valid_until(now, attr_out.attr_valid(), attr_out.attr_valid_nsec());

        VirtioFsInode::new(
            entry_out.nodeid(),
            metadata_from_attr(attr_out.attr(), fs.sb().container_dev_id),
            Arc::downgrade(fs),
            entry_valid_until,
            attr_valid_until,
            fs.conn.bump_attr_version(),
        )
    }

    pub(in super::super) fn readdir(
        &self,
        fh: FuseFileHandle,
        offset: usize,
        flags: u32,
        visitor: &mut dyn DirentVisitor,
    ) -> Result<usize> {
        let fs = self.fs_ref();
        let entries: Vec<FuseDirEntry> = fs.conn.readdir(
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

        self.inner.write().attr_valid_until = MonotonicCoarseClock::get().read_time();

        Ok(offset_read)
    }

    pub(in super::super) fn setattr(&self, setattr_in: SetattrIn) -> Result<()> {
        let fs = self.fs_ref();
        let request_attr_version = fs.conn.snapshot_attr_version();
        let valid = setattr_in.valid();
        let attr_out = fs.conn.setattr(self.nodeid(), setattr_in)?;

        self.commit_metadata_changing_reply(
            attr_out.attr(),
            attr_out.attr_valid(),
            attr_out.attr_valid_nsec(),
            request_attr_version,
            MetadataUpdate::Setattr(valid),
            &fs,
        )?;

        Ok(())
    }

    fn forget_async(&self, nlookup: u64) {
        let nodeid = self.nodeid();

        if let Some(fs) = self.fs.upgrade() {
            work_queue::submit_work_func(
                move || {
                    fs.conn.forget(nodeid, nlookup);
                },
                WorkPriority::Normal,
            );
        }
    }

    pub(in super::super) fn revalidate_lookup(
        &self,
        parent_nodeid: aster_fuse::FuseNodeId,
        name: &str,
    ) -> Result<()> {
        let now = MonotonicCoarseClock::get().read_time();
        if now < *self.entry_valid_until.lock() {
            return Ok(());
        }

        let old_nodeid = self.nodeid();
        let fs = self.fs_ref();
        let request_attr_version = fs.conn.snapshot_attr_version();
        let entry_out = fs.conn.lookup(parent_nodeid, name)?;

        if entry_out.nodeid() != old_nodeid {
            fs.conn.forget(entry_out.nodeid(), 1);
            return_errno_with_message!(Errno::ESTALE, "virtiofs stale dentry after revalidate");
        }

        self.lookup_count.increase();

        self.commit_fresh_metadata_reply(
            entry_out.attr(),
            entry_out.attr_valid(),
            entry_out.attr_valid_nsec(),
            request_attr_version,
            &fs,
        )?;

        let now = MonotonicCoarseClock::get().read_time();
        *self.entry_valid_until.lock() =
            valid_until(now, entry_out.entry_valid(), entry_out.entry_valid_nsec());

        Ok(())
    }

    pub(in super::super) fn revalidate_attr(&self, fh: FuseFileHandle) -> Result<()> {
        let now = MonotonicCoarseClock::get().read_time();
        if self.inner.read().is_attr_valid(now) {
            return Ok(());
        }

        let fs = self.fs_ref();
        let request_attr_version = fs.conn.snapshot_attr_version();
        let attr_out = fs
            .conn
            .getattr(self.nodeid(), GetattrFlags::GETATTR_FH, fh)?;

        self.commit_fresh_metadata_reply(
            attr_out.attr(),
            attr_out.attr_valid(),
            attr_out.attr_valid_nsec(),
            request_attr_version,
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
