// SPDX-License-Identifier: MPL-2.0

//! Metadata cache and attribute updates for `VirtioFsInode`.

use core::time::Duration;

use aster_fuse::{Attr, SetattrIn, SetattrValid};
use aster_virtio::device::filesystem::device::AttrVersion;

use super::{
    super::{super::metadata_from_attr, VirtioFs, valid_until},
    InodeInner, MetadataUpdate, VirtioFsInode,
};
use crate::{
    fs::vfs::{file_system::FileSystem, inode::Metadata},
    prelude::*,
    time::clocks::{MonotonicCoarseClock, RealTimeCoarseClock},
};

impl VirtioFsInode {
    pub(in super::super) fn commit_metadata_changing_reply(
        &self,
        attr: Attr,
        attr_valid: u64,
        attr_valid_nsec: u32,
        request_attr_version: AttrVersion,
        update: MetadataUpdate,
        fs: &VirtioFs,
    ) -> Result<()> {
        let metadata = metadata_from_attr(attr, fs.sb().container_dev_id);
        let now = MonotonicCoarseClock::get().read_time();

        let should_invalidate = {
            let mut inner = self.inner.write();
            if inner.accepts_attr_version(request_attr_version) {
                self.commit_metadata_locked(
                    &mut inner,
                    metadata,
                    valid_until(now, attr_valid, attr_valid_nsec),
                    fs,
                )
            } else {
                self.commit_metadata_update_locked(&mut inner, metadata, now, update, fs)
            }
        };

        if should_invalidate {
            self.invalidate_page_cache(self.size())?;
        }

        Ok(())
    }

    pub(in super::super) fn commit_fresh_metadata_reply(
        &self,
        attr: Attr,
        attr_valid: u64,
        attr_valid_nsec: u32,
        request_attr_version: AttrVersion,
        fs: &VirtioFs,
    ) -> Result<()> {
        if !self.inner.read().accepts_attr_version(request_attr_version) {
            return Ok(());
        }

        let metadata = metadata_from_attr(attr, fs.sb().container_dev_id);
        let now = MonotonicCoarseClock::get().read_time();

        let should_invalidate = {
            let mut inner = self.inner.write();
            if !inner.accepts_attr_version(request_attr_version) {
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

    pub(in super::super) fn commit_local_write(&self, new_size: usize) {
        let fs = self.fs_ref();
        let now = RealTimeCoarseClock::get().read_time();

        let mut inner = self.inner.write();
        inner.metadata.size = inner.metadata.size.max(new_size);
        inner.metadata.nr_sectors_allocated = inner.metadata.size.div_ceil(512);
        inner.metadata.last_modify_at = now;
        inner.metadata.last_meta_change_at = now;
        inner.attr_valid_until = MonotonicCoarseClock::get().read_time();
        inner.attr_version = fs.conn.bump_attr_version();
    }

    pub(in super::super) fn set_time(&self, valid: SetattrValid, time: Duration) {
        let setattr_in = match valid {
            SetattrValid::FATTR_ATIME => {
                SetattrIn::new(valid).with_atime(time.as_secs(), time.subsec_nanos())
            }
            SetattrValid::FATTR_MTIME => {
                SetattrIn::new(valid).with_mtime(time.as_secs(), time.subsec_nanos())
            }
            SetattrValid::FATTR_CTIME => {
                SetattrIn::new(valid).with_ctime(time.as_secs(), time.subsec_nanos())
            }
            _ => unreachable!(),
        };
        if let Err(err) = self.setattr(setattr_in) {
            warn!(
                "virtiofs set_time failed for inode {}: {:?}",
                self.nodeid().as_u64(),
                err
            );
        }
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

    fn commit_metadata_update_locked(
        &self,
        inner: &mut InodeInner,
        metadata: Metadata,
        attr_valid_until: Duration,
        update: MetadataUpdate,
        fs: &VirtioFs,
    ) -> bool {
        let old_metadata = inner.metadata;

        match update {
            MetadataUpdate::Setattr(valid) => {
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
            MetadataUpdate::Link => {
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
}
