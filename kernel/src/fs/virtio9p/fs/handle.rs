// SPDX-License-Identifier: MPL-2.0

use super::inode::Virtio9PInode;
use crate::{
    events::IoEvents,
    fs::{
        inode_handle::FileIo,
        utils::{Inode, InodeIo, SeekFrom, StatusFlags},
    },
    prelude::*,
    process::signal::{PollHandle, Pollable},
};

pub(super) struct Virtio9PHandle {
    inode: Arc<Virtio9PInode>,
    open_fid: u32,
    #[allow(unused)]
    flags: u32,
    cache_enabled: bool,
}

impl Virtio9PHandle {
    pub(super) fn new(
        inode: Arc<Virtio9PInode>,
        open_fid: u32,
        flags: u32,
        cache_enabled: bool,
    ) -> Self {
        Self {
            inode,
            open_fid,
            flags,
            cache_enabled,
        }
    }
}

impl Drop for Virtio9PHandle {
    fn drop(&mut self) {
        if self.cache_enabled {
            let _ = self.inode.flush_page_cache();
        }
        if let Some(fs) = self.inode.try_fs_ref() {
            fs.fid_mgr.clunk(self.open_fid);
        }
    }
}

impl Pollable for Virtio9PHandle {
    fn poll(&self, mask: IoEvents, _poller: Option<&mut PollHandle>) -> IoEvents {
        let events = IoEvents::IN | IoEvents::OUT;
        events & mask
    }
}

impl InodeIo for Virtio9PHandle {
    fn read_at(
        &self,
        offset: usize,
        writer: &mut VmWriter,
        _status_flags: StatusFlags,
    ) -> Result<usize> {
        if self.cache_enabled {
            self.inode.cached_read_at(offset, writer)
        } else {
            self.inode.direct_read_at(offset, writer)
        }
    }

    fn write_at(
        &self,
        offset: usize,
        reader: &mut VmReader,
        status_flags: StatusFlags,
    ) -> Result<usize> {
        let offset = if status_flags.contains(StatusFlags::O_APPEND) {
            self.inode.revalidate_attr()?;
            self.inode.size()
        } else {
            offset
        };

        if self.cache_enabled {
            self.inode.cached_write_at(offset, reader)
        } else {
            self.inode.direct_write_at(offset, reader)
        }
    }
}

impl FileIo for Virtio9PHandle {
    fn check_seekable(&self) -> Result<()> {
        Ok(())
    }

    fn is_offset_aware(&self) -> bool {
        true
    }

    fn seek(&self, offset: &Mutex<usize>, pos: SeekFrom) -> Result<usize> {
        let new_offset = match pos {
            SeekFrom::Start(off) => off as usize,
            SeekFrom::Current(diff) => {
                let cur = *offset.lock();
                if diff >= 0 {
                    cur.checked_add(diff as usize)
                        .ok_or_else(|| Error::with_message(Errno::EOVERFLOW, "seek overflow"))?
                } else {
                    cur.checked_sub((-diff) as usize)
                        .ok_or_else(|| Error::with_message(Errno::EINVAL, "seek before start"))?
                }
            }
            SeekFrom::End(diff) => {
                let file_size = self.inode.size();
                if diff >= 0 {
                    file_size
                        .checked_add(diff as usize)
                        .ok_or_else(|| Error::with_message(Errno::EOVERFLOW, "seek overflow"))?
                } else {
                    file_size
                        .checked_sub((-diff) as usize)
                        .ok_or_else(|| Error::with_message(Errno::EINVAL, "seek before start"))?
                }
            }
        };

        *offset.lock() = new_offset;
        Ok(new_offset)
    }
}
