// SPDX-License-Identifier: MPL-2.0

//! Open directory handles for `virtiofs`.

use alloc::sync::Arc;

use aster_fuse::FuseOpenFlags;
use ostd::mm::{VmReader, VmWriter};

use super::{inode::VirtioFsInode, open_handle::VirtioFsOpenHandle};
use crate::{
    events::IoEvents,
    fs::{
        file::{FileIo, StatusFlags},
        utils::DirentVisitor,
        vfs::inode::InodeIo,
    },
    prelude::*,
    process::signal::{PollHandle, Pollable},
    thread::work_queue::{self, WorkPriority},
};

/// A per-open directory object backed by a FUSE open handle.
///
/// Readdir and release requests carry this handle.
pub(super) struct VirtioFsDir {
    inode: Arc<VirtioFsInode>,
    open_handle: Arc<VirtioFsOpenHandle>,
}

impl VirtioFsDir {
    pub(super) fn new(inode: Arc<VirtioFsInode>, open_handle: Arc<VirtioFsOpenHandle>) -> Self {
        Self { inode, open_handle }
    }
}

impl Drop for VirtioFsDir {
    fn drop(&mut self) {
        let open_handle = self.open_handle.clone();

        work_queue::submit_work_func(move || open_handle.release(), WorkPriority::Normal);
    }
}

impl Pollable for VirtioFsDir {
    fn poll(&self, mask: IoEvents, _poller: Option<&mut PollHandle>) -> IoEvents {
        let events = IoEvents::IN | IoEvents::OUT;
        events & mask
    }
}

impl InodeIo for VirtioFsDir {
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

impl FileIo for VirtioFsDir {
    fn check_seekable(&self) -> Result<()> {
        if self
            .open_handle
            .open_flags()
            .intersects(FuseOpenFlags::FOPEN_STREAM | FuseOpenFlags::FOPEN_NONSEEKABLE)
        {
            return_errno_with_message!(Errno::ESPIPE, "the directory is not seekable");
        }

        Ok(())
    }

    fn is_offset_aware(&self) -> bool {
        true
    }

    fn readdir(&self, offset: &Mutex<usize>, visitor: &mut dyn DirentVisitor) -> Result<usize> {
        let mut offset = offset.lock();
        let read_cnt = self.inode.readdir(
            self.open_handle.fh(),
            *offset,
            self.open_handle.file_flags(),
            visitor,
        )?;
        *offset = offset.checked_add(read_cnt).ok_or_else(|| {
            Error::with_message(Errno::EOVERFLOW, "virtiofs directory offset overflow")
        })?;
        Ok(read_cnt)
    }
}
