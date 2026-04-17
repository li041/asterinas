// SPDX-License-Identifier: MPL-2.0

//! Opened-file handles for `virtiofs`.
//!
//! This module defines per-open file and directory handle types that translate
//! VFS file operations into FUSE requests against [`VirtioFsInode`].

use alloc::sync::Arc;

use aster_fuse::FuseFileHandle;
use aster_virtio::device::filesystem::device::ReleaseKind;
use ostd::{
    mm::{VmReader, VmWriter},
    warn,
};

use super::inode::VirtioFsInode;
use crate::{
    events::IoEvents,
    fs::{
        file::{AccessMode, FileIo, SeekFrom, StatusFlags},
        utils::DirentVisitor,
        vfs::inode::InodeIo,
    },
    prelude::*,
    process::signal::{PollHandle, Pollable},
    thread::work_queue::{self, WorkPriority},
};

/// Represents one opened VFS file on top of a FUSE file handle.
///
/// Here, FUSE `fh` (file handle) and VFS `fd` (file descriptor) are different:
/// - `fh` is an opaque server-side handle returned by `FUSE_OPEN`; I/O and
///   release requests carry it so the backend can access opened state.
/// - `fd` is a per-process VFS object with userspace-visible access rights.
///   Those rights are validated by the VFS/open path before operations reach
///   this handle.
pub(super) struct VirtioFsHandle {
    inode: Arc<VirtioFsInode>,
    fh: FuseFileHandle,
    flags: AccessMode,
    cache_mode: CacheMode,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CacheMode {
    Cached,
    Direct,
}

impl VirtioFsHandle {
    pub(super) fn new(
        inode: Arc<VirtioFsInode>,
        fh: FuseFileHandle,
        flags: AccessMode,
        cache_mode: CacheMode,
    ) -> Self {
        Self {
            inode,
            fh,
            flags,
            cache_mode,
        }
    }
}

pub(super) struct VirtioFsDirHandle {
    inode: Arc<VirtioFsInode>,
    fh: FuseFileHandle,
}

impl VirtioFsDirHandle {
    pub(super) fn new(inode: Arc<VirtioFsInode>, fh: FuseFileHandle) -> Self {
        Self { inode, fh }
    }
}

impl Drop for VirtioFsHandle {
    fn drop(&mut self) {
        let inode = self.inode.clone();
        let fh = self.fh;
        let flags = self.flags;
        let cache_mode = self.cache_mode;

        work_queue::submit_work_func(
            move || {
                if cache_mode == CacheMode::Cached
                    && let Err(err) = inode.flush_page_cache()
                {
                    warn!(
                        "virtiofs flush before release failed for inode {}: {:?}",
                        inode.nodeid(),
                        err
                    );
                }

                if let Some(fs) = inode.try_fs_ref() {
                    fs.conn
                        .fuse_release(inode.nodeid(), fh, flags as u32, ReleaseKind::File);
                }
            },
            WorkPriority::Normal,
        );
    }
}

impl Pollable for VirtioFsHandle {
    fn poll(&self, mask: IoEvents, _poller: Option<&mut PollHandle>) -> IoEvents {
        let events = IoEvents::IN | IoEvents::OUT;
        events & mask
    }
}

impl Drop for VirtioFsDirHandle {
    fn drop(&mut self) {
        let inode = self.inode.clone();
        let fh = self.fh;

        work_queue::submit_work_func(
            move || {
                if let Some(fs) = inode.try_fs_ref() {
                    fs.conn
                        .fuse_release(inode.nodeid(), fh, 0, ReleaseKind::Directory);
                }
            },
            WorkPriority::Normal,
        );
    }
}

impl Pollable for VirtioFsDirHandle {
    fn poll(&self, mask: IoEvents, _poller: Option<&mut PollHandle>) -> IoEvents {
        let events = IoEvents::IN | IoEvents::OUT;
        events & mask
    }
}

impl InodeIo for VirtioFsHandle {
    fn read_at(
        &self,
        offset: usize,
        writer: &mut VmWriter,
        _status_flags: StatusFlags,
    ) -> Result<usize> {
        if self.cache_mode == CacheMode::Cached {
            self.inode.cached_read_at(offset, writer, self.fh)
        } else {
            self.inode.direct_read_at(offset, writer, self.fh)
        }
    }

    fn write_at(
        &self,
        offset: usize,
        reader: &mut VmReader,
        status_flags: StatusFlags,
    ) -> Result<usize> {
        let offset = if status_flags.contains(StatusFlags::O_APPEND) {
            self.inode.revalidate_attr(self.fh)?;
            self.inode.size()
        } else {
            offset
        };

        if self.cache_mode == CacheMode::Cached {
            self.inode.cached_write_at(offset, reader, self.fh)
        } else {
            self.inode.direct_write_at(offset, reader, self.fh)
        }
    }
}

impl FileIo for VirtioFsHandle {
    fn check_seekable(&self) -> Result<()> {
        Ok(())
    }

    fn is_offset_aware(&self) -> bool {
        true
    }

    fn seek(&self, offset: &Mutex<usize>, pos: SeekFrom, _end: Option<usize>) -> Result<usize> {
        let end = match pos {
            SeekFrom::End(_) => {
                // The cached inode size may be stale. Refreshing attributes here
                // keeps `SEEK_END` consistent with the latest file size on the server.
                self.inode.revalidate_attr(self.fh)?;
                Some(self.inode.size())
            }
            _ => None,
        };

        let mut offset = offset.lock();
        let new_offset = match pos {
            SeekFrom::Start(off) => off,
            SeekFrom::Current(diff) => offset.wrapping_add_signed(diff),
            SeekFrom::End(diff) => end.unwrap().wrapping_add_signed(diff),
        };

        if new_offset.cast_signed() < 0 {
            return_errno_with_message!(Errno::EINVAL, "the file offset cannot be negative");
        }

        *offset = new_offset;
        Ok(new_offset)
    }
}

impl InodeIo for VirtioFsDirHandle {
    fn read_at(
        &self,
        _offset: usize,
        _writer: &mut VmWriter,
        _status_flags: StatusFlags,
    ) -> Result<usize> {
        return_errno_with_message!(Errno::EISDIR, "the inode is a directory");
    }

    fn write_at(
        &self,
        _offset: usize,
        _reader: &mut VmReader,
        _status_flags: StatusFlags,
    ) -> Result<usize> {
        return_errno_with_message!(Errno::EISDIR, "the inode is a directory");
    }
}

impl FileIo for VirtioFsDirHandle {
    fn check_seekable(&self) -> Result<()> {
        Ok(())
    }

    fn is_offset_aware(&self) -> bool {
        true
    }

    fn readdir(&self, offset: &Mutex<usize>, visitor: &mut dyn DirentVisitor) -> Result<usize> {
        let mut offset = offset.lock();
        let read_cnt = self.inode.readdir(self.fh, *offset, visitor)?;
        *offset = offset.checked_add(read_cnt).ok_or_else(|| {
            Error::with_message(Errno::EOVERFLOW, "virtiofs directory offset overflow")
        })?;
        Ok(read_cnt)
    }
}
