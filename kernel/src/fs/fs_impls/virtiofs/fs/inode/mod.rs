// SPDX-License-Identifier: MPL-2.0

//! Inode implementation for `virtiofs`.

mod metadata;
mod ops;
mod page_cache;
mod vfs;

use alloc::sync::Weak;
use core::time::Duration;

use aster_fuse::{DirentType, FuseNodeId, LookupCount};
use aster_virtio::device::filesystem::device::AttrVersion;

use super::{VirtioFs, open_handle::OpenHandles};
use crate::{
    fs::{
        file::InodeType,
        vfs::{
            inode::{Extension, Metadata},
            page_cache::PageCache,
        },
    },
    prelude::*,
};

pub(super) struct VirtioFsInode {
    nodeid: FuseNodeId,
    type_: InodeType,
    lookup_count: LookupCount,
    inner: RwMutex<InodeInner>,
    entry_valid_until: Mutex<Duration>,
    page_cache: Option<PageCache>,
    open_handles: OpenHandles,
    fs: Weak<VirtioFs>,
    extension: Extension,
    weak_self: Weak<Self>,
}

impl VirtioFsInode {
    pub(in super::super) fn new(
        nodeid: FuseNodeId,
        metadata: Metadata,
        fs: Weak<VirtioFs>,
        entry_valid_until: Duration,
        attr_valid_until: Duration,
        attr_version: AttrVersion,
    ) -> Arc<Self> {
        Arc::new_cyclic(|weak_self| Self {
            nodeid,
            type_: metadata.type_,
            lookup_count: LookupCount::default(),
            page_cache: metadata
                .type_
                .is_regular_file()
                .then(|| PageCache::with_capacity(metadata.size, weak_self.clone() as _).unwrap()),
            inner: RwMutex::new(InodeInner {
                metadata,
                attr_valid_until,
                attr_version,
            }),
            entry_valid_until: Mutex::new(entry_valid_until),
            open_handles: OpenHandles::new(),
            fs,
            extension: Extension::new(),
            weak_self: weak_self.clone(),
        })
    }

    pub(in super::super) fn fs_ref(&self) -> Arc<VirtioFs> {
        self.fs.upgrade().unwrap()
    }

    pub(in super::super) fn nodeid(&self) -> FuseNodeId {
        self.nodeid
    }

    pub(in super::super) fn size(&self) -> usize {
        self.inner.read().metadata.size
    }

    pub(in super::super) fn type_(&self) -> InodeType {
        self.type_
    }
}

struct InodeInner {
    metadata: Metadata,
    attr_valid_until: Duration,
    attr_version: AttrVersion,
}

impl InodeInner {
    fn is_attr_valid(&self, now: Duration) -> bool {
        now < self.attr_valid_until
    }

    fn accepts_attr_version(&self, incoming: AttrVersion) -> bool {
        incoming >= self.attr_version
    }
}

#[derive(Clone, Copy)]
pub(in super::super) enum MetadataUpdate {
    Setattr(aster_fuse::SetattrValid),
    Link,
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
