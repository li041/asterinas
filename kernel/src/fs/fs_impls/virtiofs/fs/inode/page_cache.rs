// SPDX-License-Identifier: MPL-2.0

//! Page cache backend implementation for `VirtioFsInode`.

use aster_block::bio::{BioCompleteFn, BioSegment, BioStatus, BioWaiter};
use aster_fuse::{WriteFlags, WriteIn};
use ostd::mm::PAGE_SIZE;

use super::VirtioFsInode;
use crate::{prelude::*, vm::page_cache::PageCacheBackend};

impl PageCacheBackend for VirtioFsInode {
    fn submit_read_bio(
        &self,
        idx: usize,
        bio_segment: BioSegment,
        complete_fn: Option<BioCompleteFn>,
    ) -> Result<BioWaiter> {
        let offset = idx.checked_mul(PAGE_SIZE).ok_or_else(|| {
            Error::with_message(Errno::EOVERFLOW, "virtiofs page offset overflow")
        })?;
        if offset >= self.size() {
            return_errno_with_message!(Errno::EINVAL, "virtiofs page read beyond EOF");
        }

        let size = (self.size() - offset).min(PAGE_SIZE).min(u32::MAX as usize) as u32;

        let ret: Result<()> = (|| {
            if let Some(open_handle) = self.open_handles.find_readable_handle() {
                let fs = self.fs_ref();
                fs.session.read(
                    self.nodeid(),
                    open_handle.fh(),
                    offset as u64,
                    size,
                    open_handle.file_flags(),
                    bio_segment,
                )?;
            } else {
                self.read_bio_with_transient_handle(offset, size, bio_segment)?;
            }
            Ok(())
        })();

        if let Some(complete_fn) = complete_fn {
            let bio_status = match ret {
                Ok(()) => BioStatus::Complete,
                Err(_) => BioStatus::IoError,
            };
            complete_fn(bio_status);
        }

        Ok(BioWaiter::new())
    }

    fn submit_write_bio(
        &self,
        idx: usize,
        bio_segment: BioSegment,
        complete_fn: Option<BioCompleteFn>,
    ) -> Result<BioWaiter> {
        let offset = idx.checked_mul(PAGE_SIZE).ok_or_else(|| {
            Error::with_message(Errno::EOVERFLOW, "virtiofs page offset overflow")
        })?;
        let file_size = self.size();
        if offset >= file_size {
            return Ok(BioWaiter::new());
        }

        let size = (file_size - offset).min(PAGE_SIZE);

        let ret: Result<()> = (|| {
            if let Some(open_handle) = self.open_handles.find_writable_handle() {
                let fs = self.fs_ref();
                fs.session.write(
                    self.nodeid(),
                    WriteIn::new(
                        open_handle.fh(),
                        offset as u64,
                        size as u32,
                        open_handle.file_flags(),
                        WriteFlags::empty(),
                    ),
                    bio_segment,
                )?;
            } else {
                self.write_bio_with_transient_handle(
                    offset,
                    size,
                    WriteFlags::empty(),
                    bio_segment,
                )?;
            }
            Ok(())
        })();

        if let Some(complete_fn) = complete_fn {
            let bio_status = match ret {
                Ok(()) => BioStatus::Complete,
                Err(_) => BioStatus::IoError,
            };
            complete_fn(bio_status);
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

    pub(in super::super) fn invalidate_whole_page_cache(&self) -> Result<()> {
        let Some(page_cache) = &self.page_cache else {
            return Ok(());
        };

        let cached_size = page_cache.size();
        if cached_size > 0 {
            page_cache.evict_range(0..cached_size)?;
        }

        Ok(())
    }
}
