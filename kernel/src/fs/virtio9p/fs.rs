// SPDX-License-Identifier: MPL-2.0

mod handle;
mod inode;

use aster_virtio::device::transport9p::{
    device::{Transport9PDevice, get_device_by_tag},
    protocol::{P9Attr, P9Qid, P9_GETATTR_ALL, P9_NOFID},
};

use self::inode::Virtio9PInode;
use super::fid::FidManager;
use crate::{
    fs::{
        registry::{FsProperties, FsType},
        utils::{FileSystem, FsEventSubscriberStats, FsFlags, Inode, Metadata, SuperBlock},
    },
    prelude::*,
};

const P9_MAGIC: u64 = 0x01021997; // 9P2000
const BLOCK_SIZE: usize = 4096;
const NAME_MAX: usize = 255;

pub(super) const S_IFREG: u32 = 0o100000;
pub(super) const S_IFDIR: u32 = 0o040000;
pub(super) const S_IFLNK: u32 = 0o120000;
pub(super) const S_IFSOCK: u32 = 0o140000;
pub(super) const S_IFBLK: u32 = 0o060000;
pub(super) const S_IFCHR: u32 = 0o020000;
pub(super) const S_IFIFO: u32 = 0o010000;
pub(super) const O_RDONLY: u32 = 0;
pub(super) const O_WRONLY: u32 = 1;
pub(super) const O_RDWR: u32 = 2;
pub(super) const P9_READDIR_BUF_SIZE: u32 = 4096;

pub(super) struct Virtio9PFsType;

impl FsType for Virtio9PFsType {
    fn name(&self) -> &'static str {
        "9p"
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
            .ok_or_else(|| Error::with_message(Errno::EINVAL, "9p source(tag) is required"))?
            .to_str()
            .map_err(|_| Error::with_message(Errno::EINVAL, "invalid 9p tag"))?
            .to_string();

        let device = get_device_by_tag(&tag)
            .ok_or_else(|| Error::with_message(Errno::ENODEV, "9p device tag not found"))?;

        Ok(Virtio9P::new(device, tag)? as _)
    }

    fn sysnode(&self) -> Option<Arc<dyn aster_systree::SysNode>> {
        None
    }
}

pub(super) struct Virtio9P {
    sb: SuperBlock,
    root: Arc<Virtio9PInode>,
    tag: String,
    pub(super) fid_mgr: Arc<FidManager>,
    pub(super) root_fid: u32,
    fs_event_subscriber_stats: FsEventSubscriberStats,
}

impl Virtio9P {
    fn new(device: Arc<Transport9PDevice>, tag: String) -> Result<Arc<Self>> {
        let fid_mgr = Arc::new(FidManager::new(device.clone()));

        // Allocate a FID for the root and attach.
        let root_fid = fid_mgr.alloc_fid();
        let root_qid = device
            .p9_attach(root_fid, P9_NOFID, "", "", 0)
            .map_err(|_| Error::with_message(Errno::EIO, "9p attach failed"))?;

        // Get attributes of root.
        let root_attr = device
            .p9_getattr(root_fid, P9_GETATTR_ALL)
            .map_err(|_| Error::with_message(Errno::EIO, "9p getattr root failed"))?;

        let root_metadata = p9_attr_to_metadata(&root_attr);

        // Get filesystem statistics for the superblock.
        let sb = match device.p9_statfs(root_fid) {
            Ok(statfs) => SuperBlock {
                magic: P9_MAGIC,
                bsize: statfs.bsize as usize,
                blocks: statfs.blocks as usize,
                bfree: statfs.bfree as usize,
                bavail: statfs.bavail as usize,
                files: statfs.files as usize,
                ffree: statfs.ffree as usize,
                fsid: statfs.fsid,
                namelen: statfs.namelen as usize,
                frsize: statfs.bsize as usize,
                flags: 0,
            },
            Err(_) => SuperBlock::new(P9_MAGIC, BLOCK_SIZE, NAME_MAX),
        };

        Ok(Arc::new_cyclic(|weak_fs| {
            let root = Virtio9PInode::new(
                root_fid,
                root_qid,
                root_metadata,
                weak_fs.clone(),
            );

            Self {
                sb,
                root,
                tag,
                fid_mgr,
                root_fid,
                fs_event_subscriber_stats: FsEventSubscriberStats::new(),
            }
        }))
    }
}

impl FileSystem for Virtio9P {
    fn name(&self) -> &'static str {
        "9p"
    }

    fn source(&self) -> Option<&str> {
        Some(&self.tag)
    }

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

/// Convert P9Attr to kernel Metadata.
pub(super) fn p9_attr_to_metadata(attr: &P9Attr) -> Metadata {
    use core::time::Duration;
    use crate::fs::utils::{InodeMode, InodeType};
    use crate::process::{Gid, Uid};

    Metadata {
        dev: 0,
        ino: attr.qid.path,
        size: attr.size as usize,
        blk_size: if attr.blksize > 0 { attr.blksize as usize } else { 4096 },
        blocks: attr.blocks as usize,
        atime: Duration::new(attr.atime_sec, attr.atime_nsec as u32),
        mtime: Duration::new(attr.mtime_sec, attr.mtime_nsec as u32),
        ctime: Duration::new(attr.ctime_sec, attr.ctime_nsec as u32),
        type_: InodeType::from_raw_mode(attr.mode as u16).unwrap_or(InodeType::Unknown),
        mode: InodeMode::from_bits_truncate(attr.mode as u16),
        nlinks: attr.nlink as usize,
        uid: Uid::new(attr.uid),
        gid: Gid::new(attr.gid),
        rdev: attr.rdev,
    }
}

/// Convert dirent type byte to InodeType.
pub(super) fn inode_type_from_dirent_type(type_: u8) -> crate::fs::utils::InodeType {
    use crate::fs::utils::InodeType;
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
