// SPDX-License-Identifier: MPL-2.0

//! Virtio-fs support on top of the VFS inode interface.
//!
//! This module wires the `virtiofs` filesystem type into the VFS and converts
//! FUSE attributes into kernel [`Metadata`]. The filesystem-specific inode and
//! handle implementations live in the `fs` submodule.

mod fs;

use core::time::Duration;

use aster_fuse::Attr;
use device_id::DeviceId;

use crate::{
    fs::{
        file::{InodeMode, InodeType},
        vfs::inode::Metadata,
    },
    process::{Gid, Uid},
};

pub(super) fn init() {
    crate::fs::vfs::registry::register(&fs::VirtioFsType).unwrap();
}

pub(super) fn metadata_from_attr(attr: Attr, container_dev_id: DeviceId) -> Metadata {
    Metadata {
        ino: attr.ino,
        size: attr.size as usize,
        optimal_block_size: attr.blksize as usize,
        nr_sectors_allocated: attr.blocks as usize,
        last_access_at: Duration::new(attr.atime, attr.atimensec),
        last_modify_at: Duration::new(attr.mtime, attr.mtimensec),
        last_meta_change_at: Duration::new(attr.ctime, attr.ctimensec),
        type_: InodeType::from_raw_mode(attr.mode as u16).unwrap_or(InodeType::Unknown),
        mode: InodeMode::from_bits_truncate(attr.mode as u16),
        nr_hard_links: attr.nlink as usize,
        uid: Uid::new(attr.uid),
        gid: Gid::new(attr.gid),
        container_dev_id,
        self_dev_id: if attr.rdev == 0 {
            None
        } else {
            DeviceId::from_encoded_u64(attr.rdev as u64)
        },
    }
}
