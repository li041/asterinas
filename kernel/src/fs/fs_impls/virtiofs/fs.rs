// SPDX-License-Identifier: MPL-2.0

//! Virtio-fs filesystem wiring.
//!
//! This module defines [`VirtioFsType`] and [`VirtioFs`], which connect the
//! VFS filesystem interface to the virtio-fs device client. The `handle` and
//! `inode` submodules implement opened-file behavior and inode operations.

mod handle;
mod inode;

use alloc::{
    string::{String, ToString},
    sync::Arc,
};
use core::time::Duration;

use aster_fuse::FUSE_ROOT_ID;
use aster_virtio::device::filesystem::device::{self, FuseConnection};

use self::inode::VirtioFsInode;
use crate::{
    fs::{
        pseudofs::AnonDeviceId,
        utils::NAME_MAX,
        vfs::{
            file_system::{FileSystem, FsEventSubscriberStats, FsFlags, SuperBlock},
            inode::Inode,
            registry::{FsProperties, FsType},
        },
    },
    prelude::*,
    time::clocks::MonotonicCoarseClock,
};

const VIRTIOFS_MAGIC: u64 = 0x6573_5546;
const BLOCK_SIZE: usize = 4096;
pub(super) const FUSE_READDIR_BUF_SIZE: u32 = 4096;

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

        let device = device::find_device_by_tag(&tag)
            .ok_or_else(|| Error::with_message(Errno::ENODEV, "virtiofs device tag not found"))?;

        Ok(VirtioFs::new(device, tag)? as Arc<dyn FileSystem>)
    }

    fn sysnode(&self) -> Option<Arc<dyn aster_systree::SysNode>> {
        None
    }
}

pub(super) struct VirtioFs {
    sb: SuperBlock,
    root: Arc<VirtioFsInode>,
    tag: String,
    pub(super) conn: Arc<FuseConnection>,
    fs_event_subscriber_stats: FsEventSubscriberStats,
}

impl VirtioFs {
    fn new(
        device: Arc<aster_virtio::device::filesystem::device::FileSystemDevice>,
        tag: String,
    ) -> Result<Arc<Self>> {
        let conn = FuseConnection::new(device)
            .map_err(|_| Error::with_message(Errno::EIO, "virtiofs FUSE_INIT failed"))?;

        let anon_device_id =
            AnonDeviceId::acquire().expect("no device ID is available for virtiofs");
        let container_dev_id = anon_device_id.id();

        let root_entry = conn.fuse_lookup(FUSE_ROOT_ID, ".")?;
        let root_metadata = super::metadata_from_attr(root_entry.attr, container_dev_id);
        let attr_valid_until = {
            let now = MonotonicCoarseClock::get().read_time();
            now.saturating_add(valid_duration(
                root_entry.attr_valid,
                root_entry.attr_valid_nsec,
            ))
        };

        Ok(Arc::new_cyclic(|weak_fs| {
            let root = VirtioFsInode::new(
                FUSE_ROOT_ID,
                root_metadata,
                weak_fs.clone(),
                None,
                attr_valid_until,
            );

            Self {
                sb: SuperBlock::new(VIRTIOFS_MAGIC, BLOCK_SIZE, NAME_MAX, container_dev_id),
                root,
                tag,
                conn,
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

    // TODO: Implement `sync` by issuing `fsync` to open files and syncing the device if supported.
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

pub(super) fn valid_duration(secs: u64, nsecs: u32) -> Duration {
    let extra_secs = (nsecs / 1_000_000_000) as u64;
    let nanos = (nsecs % 1_000_000_000) as u64;
    Duration::from_secs(secs.saturating_add(extra_secs)).saturating_add(Duration::from_nanos(nanos))
}
