// SPDX-License-Identifier: MPL-2.0

//! Server-issued FUSE open handles for `virtiofs`.

use alloc::{
    sync::{Arc, Weak},
    vec::Vec,
};

use aster_fuse::{FuseFileHandle, FuseNodeId, FuseOpenFlags, ReleaseFlags, ReleaseKind};

use super::VirtioFs;
use crate::{
    fs::file::{AccessMode, StatusFlags},
    prelude::*,
};

/// A server-issued FUSE open handle.
///
/// This object owns `fh` returned by `FUSE_OPEN` or `FUSE_OPENDIR`.
pub(super) struct VirtioFsOpenHandle {
    fh: FuseFileHandle,
    nodeid: FuseNodeId,
    access_mode: AccessMode,
    status_flags: StatusFlags,
    open_flags: FuseOpenFlags,
    fs: Weak<VirtioFs>,
    release_kind: ReleaseKind,
}

impl VirtioFsOpenHandle {
    pub(super) fn new(
        fh: FuseFileHandle,
        nodeid: FuseNodeId,
        access_mode: AccessMode,
        status_flags: StatusFlags,
        open_flags: FuseOpenFlags,
        fs: Weak<VirtioFs>,
        release_kind: ReleaseKind,
    ) -> Arc<Self> {
        Arc::new(Self {
            fh,
            access_mode,
            status_flags,
            open_flags,
            nodeid,
            fs,
            release_kind,
        })
    }

    /// Returns the FUSE file handle (`fh`) issued by the server.
    pub(super) fn fh(&self) -> FuseFileHandle {
        self.fh
    }

    /// Returns the composite file flags (access mode | status flags).
    pub(super) fn file_flags(&self) -> u32 {
        self.access_mode as u32 | self.status_flags.bits()
    }

    /// Returns the `FUSE_OPEN` reply flags.
    pub(super) fn open_flags(&self) -> FuseOpenFlags {
        self.open_flags
    }

    /// Sends `FUSE_RELEASE` (or `FUSE_RELEASEDIR`) to the server for this handle.
    pub(super) fn release(&self) {
        let Some(fs) = self.fs.upgrade() else {
            return;
        };

        fs.conn.fuse_release(
            self.nodeid,
            self.fh,
            self.file_flags(),
            ReleaseFlags::RELEASE_FLUSH,
            self.release_kind,
        );
    }
}

/// Open handles that have been opened on a virtio-fs inode.
pub(super) struct OpenHandles {
    handles: Mutex<Vec<Weak<VirtioFsOpenHandle>>>,
}

impl OpenHandles {
    pub(super) fn new() -> Self {
        Self {
            handles: Mutex::new(Vec::new()),
        }
    }

    /// Registers a handle, pruning dead weak references first.
    pub(super) fn insert(&self, handle: &Arc<VirtioFsOpenHandle>) {
        let mut handles = self.handles.lock();
        handles.retain(|handle| handle.strong_count() > 0);

        handles.push(Arc::downgrade(handle));
    }

    /// Finds a readable handle and calls `io_fn` with it.
    pub(super) fn with_readable_handle<T>(
        &self,
        io_fn: impl FnOnce(Arc<VirtioFsOpenHandle>) -> Result<T>,
    ) -> Result<T> {
        let Some(handle) = self.find_handle(AccessMode::is_readable) else {
            return Err(Error::with_message(
                Errno::EBADF,
                "virtiofs page-cache read requires an open readable handle",
            ));
        };

        io_fn(handle)
    }

    /// Finds a writable handle and calls `io_fn` with it.
    pub(super) fn with_writable_handle<T>(
        &self,
        io_fn: impl FnOnce(Arc<VirtioFsOpenHandle>) -> Result<T>,
    ) -> Result<T> {
        let Some(handle) = self.find_handle(AccessMode::is_writable) else {
            return Err(Error::with_message(
                Errno::EBADF,
                "virtiofs page-cache writeback requires an open writable handle",
            ));
        };

        io_fn(handle)
    }

    fn find_handle(
        &self,
        accepts: impl Fn(&AccessMode) -> bool,
    ) -> Option<Arc<VirtioFsOpenHandle>> {
        let mut handles = self.handles.lock();
        let mut found_handle = None;
        let mut dead_handle_indexes = Vec::new();

        // Iterate in reverse to prefer recently inserted handles,
        // which are more likely to have required properties and be valid.
        for (index, handle) in handles.iter().enumerate().rev() {
            match handle.upgrade() {
                Some(open_handle) => {
                    if accepts(&open_handle.access_mode) {
                        found_handle = Some(open_handle);
                        break;
                    }
                }
                None => dead_handle_indexes.push(index),
            }
        }

        for index in dead_handle_indexes {
            handles.remove(index);
        }

        found_handle
    }
}
