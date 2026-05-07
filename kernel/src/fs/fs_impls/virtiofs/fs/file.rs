// SPDX-License-Identifier: MPL-2.0

//! Open regular-file handles for `virtiofs`.

use alloc::sync::Arc;

use aster_fuse::FuseOpenFlags;
use ostd::{
    mm::{VmReader, VmWriter},
    warn,
};

use super::{inode::VirtioFsInode, open_handle::VirtioFsOpenHandle};
use crate::{
    events::IoEvents,
    fs::{
        file::{FileIo, SeekFrom, StatusFlags},
        vfs::inode::InodeIo,
    },
    prelude::*,
    process::signal::{PollHandle, Pollable},
    thread::work_queue::{self, WorkPriority},
};

/// A per-open file object backed by a FUSE file handle.
///
/// Each instance owns one server-issued `fh` returned by `FUSE_OPEN`. Read,
/// write, seek, and release requests carry this handle, while access rights
/// are inherited from the VFS open path that created the object.
///
/// The handle also records whether I/O should use the page cache or bypass it,
/// according to the flags returned by the server.
pub(super) struct VirtioFsFile {
    inode: Arc<VirtioFsInode>,
    open_handle: Arc<VirtioFsOpenHandle>,
    cache_policy: CachePolicy,
}

impl VirtioFsFile {
    pub(super) fn new(
        inode: Arc<VirtioFsInode>,
        open_handle: Arc<VirtioFsOpenHandle>,
        cache_policy: CachePolicy,
    ) -> Self {
        Self {
            inode,
            open_handle,
            cache_policy,
        }
    }
}

impl Drop for VirtioFsFile {
    fn drop(&mut self) {
        let inode = self.inode.clone();
        let cache_policy = self.cache_policy;
        let open_handle = self.open_handle.clone();

        work_queue::submit_work_func(
            move || {
                if cache_policy == CachePolicy::Cached
                    && let Err(err) = inode.flush_page_cache()
                {
                    warn!(
                        "virtiofs flush before release failed for inode {:?}: {:?}",
                        inode.nodeid(),
                        err
                    );
                }

                open_handle.release();
            },
            WorkPriority::Normal,
        );
    }
}

impl Pollable for VirtioFsFile {
    fn poll(&self, mask: IoEvents, _poller: Option<&mut PollHandle>) -> IoEvents {
        let events = IoEvents::IN | IoEvents::OUT;
        events & mask
    }
}

impl InodeIo for VirtioFsFile {
    fn read_at(
        &self,
        offset: usize,
        writer: &mut VmWriter,
        _status_flags: StatusFlags,
    ) -> Result<usize> {
        if self.cache_policy == CachePolicy::Cached {
            self.inode.cached_read_at(
                offset,
                writer,
                self.open_handle.fh(),
                self.open_handle.file_flags(),
            )
        } else {
            self.inode.direct_read_at(
                offset,
                writer,
                self.open_handle.fh(),
                self.open_handle.file_flags(),
            )
        }
    }

    fn write_at(
        &self,
        offset: usize,
        reader: &mut VmReader,
        status_flags: StatusFlags,
    ) -> Result<usize> {
        let offset = if status_flags.contains(StatusFlags::O_APPEND) {
            self.inode.revalidate_attr(self.open_handle.fh())?;
            self.inode.size()
        } else {
            offset
        };

        if self.cache_policy == CachePolicy::Cached {
            self.inode.cached_write_at(
                offset,
                reader,
                self.open_handle.fh(),
                self.open_handle.file_flags(),
            )
        } else {
            self.inode.direct_write_at(
                offset,
                reader,
                self.open_handle.fh(),
                self.open_handle.file_flags(),
            )
        }
    }
}

impl FileIo for VirtioFsFile {
    fn check_seekable(&self) -> Result<()> {
        if self
            .open_handle
            .open_flags()
            .intersects(FuseOpenFlags::FOPEN_STREAM | FuseOpenFlags::FOPEN_NONSEEKABLE)
        {
            return_errno_with_message!(Errno::ESPIPE, "the file is not seekable");
        }
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
                self.inode.revalidate_attr(self.open_handle.fh())?;
                Some(self.inode.size())
            }
            _ => None,
        };

        let mut offset = offset.lock();

        // `SEEK_SET`, `SEEK_CUR`, and `SEEK_END` are handled locally.
        // `FUSE_LSEEK` is only need for sparse-file seeking modes.
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

/// The virtio-fs file I/O caching policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CachePolicy {
    /// I/O goes through the page cache.
    Cached,
    /// I/O bypasses the page cache and hits the FUSE server directly.
    Direct,
}
