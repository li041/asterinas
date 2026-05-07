// SPDX-License-Identifier: MPL-2.0

//! Virtio-fs filesystem support backed by FUSE requests.

mod fs;

use core::time::Duration;

use aster_fuse::{Attr, FuseError};
use device_id::DeviceId;

use crate::{
    fs::{
        file::{InodeMode, InodeType},
        vfs::inode::Metadata,
    },
    prelude::{Errno, Error},
    process::{Gid, Uid},
};

pub(super) fn init() {
    crate::fs::vfs::registry::register(&fs::VirtioFsType).unwrap();
}

impl From<FuseError> for Error {
    fn from(error: FuseError) -> Self {
        match error {
            FuseError::ResourceAlloc(error) => Error::from(error),
            FuseError::Timeout => Error::with_message(Errno::EIO, "virtiofs request timed out"),
            FuseError::MalformedResponse => {
                Error::with_message(Errno::EIO, "malformed virtiofs response")
            }
            FuseError::PageFault => Error::with_message(Errno::EFAULT, "page fault in virtiofs"),
            FuseError::RemoteError(code) => {
                let errno = match code {
                    -1 => Errno::EPERM,
                    -2 => Errno::ENOENT,
                    -4 => Errno::EINTR,
                    -5 => Errno::EIO,
                    -6 => Errno::ENXIO,
                    -9 => Errno::EBADF,
                    -11 => Errno::EAGAIN,
                    -12 => Errno::ENOMEM,
                    -13 => Errno::EACCES,
                    -14 => Errno::EFAULT,
                    -16 => Errno::EBUSY,
                    -17 => Errno::EEXIST,
                    -18 => Errno::EXDEV,
                    -19 => Errno::ENODEV,
                    -20 => Errno::ENOTDIR,
                    -21 => Errno::EISDIR,
                    -22 => Errno::EINVAL,
                    -23 => Errno::ENFILE,
                    -24 => Errno::EMFILE,
                    -26 => Errno::ETXTBSY,
                    -27 => Errno::EFBIG,
                    -28 => Errno::ENOSPC,
                    -29 => Errno::ESPIPE,
                    -30 => Errno::EROFS,
                    -31 => Errno::EMLINK,
                    -32 => Errno::EPIPE,
                    -34 => Errno::ERANGE,
                    -36 => Errno::ENAMETOOLONG,
                    -38 => Errno::ENOSYS,
                    -39 => Errno::ENOTEMPTY,
                    -40 => Errno::ELOOP,
                    -61 => Errno::ENODATA,
                    -71 => Errno::EPROTO,
                    -74 => Errno::EBADMSG,
                    -75 => Errno::EOVERFLOW,
                    -95 => Errno::EOPNOTSUPP,
                    -110 => Errno::ETIMEDOUT,
                    _ => Errno::EIO,
                };
                Error::with_message(errno, "filesystem request failed")
            }
            FuseError::BufferTooSmall | FuseError::LengthOverflow => {
                Error::with_message(Errno::EIO, "FUSE protocol encoding error")
            }
        }
    }
}

/// Converts a FUSE `Attr` into the VFS `Metadata` structure.
pub(super) fn metadata_from_attr(attr: Attr, container_dev_id: DeviceId) -> Metadata {
    Metadata {
        ino: attr.ino(),
        size: attr.size() as usize,
        optimal_block_size: attr.blksize() as usize,
        nr_sectors_allocated: attr.blocks() as usize,
        last_access_at: Duration::new(attr.atime(), attr.atimensec()),
        last_modify_at: Duration::new(attr.mtime(), attr.mtimensec()),
        last_meta_change_at: Duration::new(attr.ctime(), attr.ctimensec()),
        type_: InodeType::from_raw_mode(attr.mode() as u16).unwrap_or(InodeType::Unknown),
        mode: InodeMode::from_bits_truncate(attr.mode() as u16),
        nr_hard_links: attr.nlink() as usize,
        uid: Uid::new(attr.uid()),
        gid: Gid::new(attr.gid()),
        container_dev_id,
        self_dev_id: if attr.rdev() == 0 {
            None
        } else {
            DeviceId::from_encoded_u64(attr.rdev() as u64)
        },
    }
}
