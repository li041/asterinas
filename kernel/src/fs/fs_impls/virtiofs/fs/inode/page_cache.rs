// SPDX-License-Identifier: MPL-2.0

//! Page cache backend implementation for `VirtioFsInode`.

use aster_block::bio::BioWaiter;
use aster_fuse::WriteFlags;
use ostd::mm::{HasSize, PAGE_SIZE, io::util::HasVmReaderWriter};

use super::VirtioFsInode;
use crate::{
    fs::vfs::page_cache::{CachePage, PageCacheBackend},
    prelude::*,
};

impl PageCacheBackend for VirtioFsInode {
    fn read_page_async(&self, idx: usize, frame: &CachePage) -> Result<BioWaiter> {
        let offset = idx.checked_mul(PAGE_SIZE).ok_or_else(|| {
            Error::with_message(Errno::EOVERFLOW, "virtiofs page offset overflow")
        })?;
        if offset >= self.size() {
            return_errno_with_message!(Errno::EINVAL, "virtiofs page read beyond EOF");
        }

        frame.writer().fill_zeros(frame.size());
        let size = (self.size() - offset).min(PAGE_SIZE).min(u32::MAX as usize) as u32;
        let mut frame_writer = frame.writer().to_fallible();
        if let Some(open_handle) = self.open_handles.find_readable_handle() {
            let fs = self.fs_ref();
            fs.conn.read(
                self.nodeid(),
                open_handle.fh(),
                offset as u64,
                size,
                open_handle.file_flags(),
                &mut frame_writer,
            )?;
        } else {
            self.read_with_transient_handle(offset, &mut frame_writer)?;
        }
        Ok(BioWaiter::new())
    }

    fn write_page_async(&self, idx: usize, frame: &CachePage) -> Result<BioWaiter> {
        let offset = idx.checked_mul(PAGE_SIZE).ok_or_else(|| {
            Error::with_message(Errno::EOVERFLOW, "virtiofs page offset overflow")
        })?;
        let file_size = self.size();
        if offset >= file_size {
            return Ok(BioWaiter::new());
        }

        let mut reader = frame.reader().to_fallible();
        reader.limit(file_size - offset);

        if let Some(open_handle) = self.open_handles.find_writable_handle() {
            let fs = self.fs_ref();
            fs.conn.write(
                self.nodeid(),
                open_handle.fh(),
                offset as u64,
                open_handle.file_flags(),
                WriteFlags::WRITE_CACHE,
                &mut reader,
            )?;
        } else {
            self.write_with_transient_handle(offset, &mut reader)?;
        }
        Ok(BioWaiter::new())
    }

    fn npages(&self) -> usize {
        self.size().div_ceil(PAGE_SIZE)
    }
}

impl VirtioFsInode {
    pub(in super::super) fn flush_page_cache(&self) -> Result<()> {
        let Some(page_cache) = &self.page_cache else {
            return Ok(());
        };

        page_cache.evict_range(0..self.size())?;
        Ok(())
    }

    pub(in super::super) fn invalidate_page_cache(&self, new_size: usize) -> Result<()> {
        let Some(page_cache) = &self.page_cache else {
            return Ok(());
        };

        let cached_size = page_cache.pages().size();
        if cached_size > 0 {
            page_cache.evict_range(0..cached_size)?;
            page_cache.resize(0)?;
        }
        page_cache.resize(new_size)?;

        Ok(())
    }
}
