// SPDX-License-Identifier: MPL-2.0

//! `Inode` trait implementation for `VirtioFsInode`.

use alloc::sync::Arc;
use core::time::Duration;

use aster_fuse::{FuseAttrOut, FuseOpenFlags, ReleaseFlags, ReleaseKind, SetattrIn, SetattrValid};

use super::{MetadataUpdate, VirtioFsInode};
use crate::{
    fs::{
        file::{AccessMode, FileIo, InodeMode, InodeType, StatusFlags},
        utils::DirentVisitor,
        vfs::{
            file_system::FileSystem,
            inode::{Extension, Inode, InodeIo, Metadata, RevalidationPolicy, SymbolicLink},
        },
    },
    prelude::*,
    process::{Gid, Uid},
    time::clocks::MonotonicCoarseClock,
    vm::vmo::Vmo,
};

impl Inode for VirtioFsInode {
    fn size(&self) -> usize {
        self.inner.read().metadata.size
    }

    fn resize(&self, new_size: usize) -> Result<()> {
        if self.type_() != InodeType::File {
            return_errno_with_message!(Errno::EISDIR, "resize on non-regular file");
        }

        let size = u64::try_from(new_size)
            .map_err(|_| Error::with_message(Errno::EFBIG, "virtiofs resize size too large"))?;

        let setattr_in = SetattrIn::new(SetattrValid::FATTR_SIZE).with_size(size);
        self.setattr(setattr_in)
    }

    fn metadata(&self) -> Metadata {
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
        let setattr_in = SetattrIn::new(SetattrValid::FATTR_MODE).with_mode(mode_bits);
        self.setattr(setattr_in)
    }

    fn owner(&self) -> Result<Uid> {
        Ok(self.inner.read().metadata.uid)
    }

    fn set_owner(&self, uid: Uid) -> Result<()> {
        let setattr_in = SetattrIn::new(SetattrValid::FATTR_UID).with_uid(uid.into());
        self.setattr(setattr_in)
    }

    fn group(&self) -> Result<Gid> {
        Ok(self.inner.read().metadata.gid)
    }

    fn set_group(&self, gid: Gid) -> Result<()> {
        let setattr_in = SetattrIn::new(SetattrValid::FATTR_GID).with_gid(gid.into());
        self.setattr(setattr_in)
    }

    fn atime(&self) -> Duration {
        self.inner.read().metadata.last_access_at
    }

    fn set_atime(&self, time: Duration) {
        self.set_time(SetattrValid::FATTR_ATIME, time);
    }

    fn mtime(&self) -> Duration {
        self.inner.read().metadata.last_modify_at
    }

    fn set_mtime(&self, time: Duration) {
        self.set_time(SetattrValid::FATTR_MTIME, time);
    }

    fn ctime(&self) -> Duration {
        self.inner.read().metadata.last_meta_change_at
    }

    fn set_ctime(&self, time: Duration) {
        self.set_time(SetattrValid::FATTR_CTIME, time);
    }

    fn page_cache(&self) -> Option<Arc<Vmo>> {
        self.page_cache
            .as_ref()
            .map(|page_cache| page_cache.pages().clone())
    }

    fn open(
        &self,
        access_mode: AccessMode,
        status_flags: StatusFlags,
    ) -> Option<Result<Box<dyn FileIo>>> {
        if !matches!(self.type_, InodeType::File | InodeType::Dir) {
            return None;
        }
        Some(self.open(access_mode, status_flags))
    }

    fn lookup(&self, name: &str) -> Result<Arc<dyn Inode>> {
        let fs = self.fs_ref();
        let parent_nodeid = self.nodeid();
        let entry_out = fs.conn.lookup(parent_nodeid, name)?;
        let nodeid = entry_out.nodeid();

        let now = MonotonicCoarseClock::get().read_time();

        let entry_valid_until =
            super::super::valid_until(now, entry_out.entry_valid(), entry_out.entry_valid_nsec());
        let attr_valid_until =
            super::super::valid_until(now, entry_out.attr_valid(), entry_out.attr_valid_nsec());

        let inode = VirtioFsInode::new(
            nodeid,
            super::super::super::metadata_from_attr(entry_out.attr(), fs.sb().container_dev_id),
            Arc::downgrade(&fs),
            entry_valid_until,
            attr_valid_until,
            fs.conn.bump_attr_version(),
        );

        Ok(inode)
    }

    fn create(&self, name: &str, type_: InodeType, mode: InodeMode) -> Result<Arc<dyn Inode>> {
        let fs = self.fs_ref();
        let parent_nodeid = self.nodeid();
        let entry_out = match type_ {
            InodeType::File => {
                let (entry_out, open_out) = fs.conn.create(
                    parent_nodeid,
                    name,
                    u32::from(InodeType::File) | u32::from(mode.bits()),
                )?;
                fs.conn.release(
                    entry_out.nodeid(),
                    open_out.fh(),
                    AccessMode::O_RDWR as u32,
                    ReleaseFlags::empty(),
                    ReleaseKind::File,
                );
                entry_out
            }
            InodeType::Dir => fs.conn.mkdir(
                parent_nodeid,
                name,
                u32::from(InodeType::Dir) | u32::from(mode.bits()),
            )?,
            InodeType::Socket => fs.conn.mknod(
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
        let attr_out = FuseAttrOut::new(
            entry_out.attr_valid(),
            entry_out.attr_valid_nsec(),
            entry_out.attr(),
        );

        Ok(self.build_child_inode(&fs, entry_out, attr_out))
    }

    fn link(&self, old: &Arc<dyn Inode>, name: &str) -> Result<()> {
        let old = old
            .downcast_ref::<VirtioFsInode>()
            .ok_or_else(|| Error::with_message(Errno::EXDEV, "not same fs"))?;

        let fs = self.fs_ref();
        let request_attr_version = fs.conn.snapshot_attr_version();
        let entry_out = fs.conn.link(old.nodeid(), self.nodeid(), name)?;
        old.lookup_count.increase();

        old.commit_metadata_changing_reply(
            entry_out.attr(),
            entry_out.attr_valid(),
            entry_out.attr_valid_nsec(),
            request_attr_version,
            MetadataUpdate::Link,
            &fs,
        )?;

        Ok(())
    }

    fn unlink(&self, name: &str) -> Result<()> {
        let fs = self.fs_ref();
        fs.conn.unlink(self.nodeid(), name)?;
        Ok(())
    }

    fn rmdir(&self, name: &str) -> Result<()> {
        let fs = self.fs_ref();
        fs.conn.rmdir(self.nodeid(), name)?;
        Ok(())
    }

    fn readdir_at(&self, offset: usize, visitor: &mut dyn DirentVisitor) -> Result<usize> {
        let fs = self.fs_ref();
        let open_out = fs.conn.opendir(self.nodeid())?;
        let mut open_flags = open_out.open_flags();
        open_flags.remove(FuseOpenFlags::FOPEN_DIRECT_IO);
        if !open_flags.contains(FuseOpenFlags::FOPEN_KEEP_CACHE) {
            self.invalidate_page_cache(self.size())?;
        }
        let result = self.readdir(open_out.fh(), offset, AccessMode::O_RDWR as u32, visitor);
        fs.conn.release(
            self.nodeid(),
            open_out.fh(),
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

    fn revalidation_policy(&self) -> RevalidationPolicy {
        match self.type_ {
            InodeType::Dir => {
                RevalidationPolicy::REVALIDATE_EXISTS | RevalidationPolicy::REVALIDATE_ABSENT
            }
            _ => RevalidationPolicy::empty(),
        }
    }

    fn revalidate_exists(&self, name: &str, child: &dyn Inode) -> bool {
        let Some(child) = child.downcast_ref::<VirtioFsInode>() else {
            return false;
        };

        child.revalidate_lookup(self.nodeid(), name).is_ok()
    }

    fn revalidate_absent(&self, _name: &str) -> bool {
        false
    }

    fn read_link(&self) -> Result<SymbolicLink> {
        if self.type_ != InodeType::SymLink {
            return_errno_with_message!(Errno::EINVAL, "read_link on non-symlink")
        }

        let fs = self.fs_ref();
        let target = fs.conn.readlink(self.nodeid())?;

        Ok(SymbolicLink::Plain(target))
    }

    fn extension(&self) -> &Extension {
        &self.extension
    }
}

impl InodeIo for VirtioFsInode {
    fn read_at(
        &self,
        offset: usize,
        writer: &mut VmWriter,
        _status_flags: StatusFlags,
    ) -> Result<usize> {
        if self.type_ != InodeType::File {
            return_errno_with_message!(
                Errno::EBADF,
                "virtiofs inode I/O requires an open file handle"
            );
        }

        self.read_with_transient_handle(offset, writer)
    }

    fn write_at(
        &self,
        offset: usize,
        reader: &mut VmReader,
        _status_flags: StatusFlags,
    ) -> Result<usize> {
        if self.type_ != InodeType::File {
            return_errno_with_message!(
                Errno::EBADF,
                "virtiofs inode I/O requires an open file handle"
            );
        }

        self.write_with_transient_handle(offset, reader)
    }
}
