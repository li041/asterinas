// SPDX-License-Identifier: MPL-2.0

//! Page cache backend implementation for `VirtioFsInode`.

use alloc::{boxed::Box, sync::Arc};
use core::{any::Any, ops::Deref};

use aster_fuse::{FuseCompleteFn, FuseStatus, ReadIn, WriteFlags, WriteIn};
use aster_util::mem_obj_slice::Slice;
use aster_virtio::device::filesystem::{device::FuseWaiter, pool::FsDmaStorage};
use ostd::mm::{PAGE_SIZE, Segment, io::util::HasVmReaderWriter};

use super::VirtioFsInode;
use crate::{
    prelude::*,
    vm::page_cache::{
        CachePageExt, PageCacheBackend, PageCacheIoWaiter,
        cache_page::{self, LockedCachePage},
    },
};

impl PageCacheBackend for VirtioFsInode {
    fn read_page_async(
        &self,
        idx: usize,
        locked_page: LockedCachePage,
    ) -> Result<Box<dyn PageCacheIoWaiter>> {
        let offset = idx.checked_mul(PAGE_SIZE).ok_or_else(|| {
            Error::with_message(Errno::EOVERFLOW, "virtiofs page offset overflow")
        })?;
        if offset >= self.size() {
            return_errno_with_message!(Errno::EINVAL, "virtiofs page read beyond EOF");
        }

        let size = (self.size() - offset).min(PAGE_SIZE).min(u32::MAX as usize) as u32;
        let data_buf = Arc::new(Slice::new(
            FsDmaStorage::new_from_segment(Segment::from(locked_page.deref().clone()).into()),
            0..size as usize,
        ));

        let complete_fn: FuseCompleteFn = Box::new(move |status| {
            if status == FuseStatus::Complete {
                locked_page.set_up_to_date();
            }
            // The page lock is released when `locked_page` is dropped here.
        });

        if let Some(open_handle) = self.open_handles.find_readable_handle() {
            let fs = self.fs_ref();
            let read_in = ReadIn::new(
                open_handle.fh(),
                offset as u64,
                size,
                open_handle.file_flags(),
            );
            let waiter =
                fs.session
                    .read_async(self.nodeid(), read_in, data_buf, Some(complete_fn))?;
            return Ok(Box::new(waiter));
        }

        let waiter = self.read_buf_with_transient_handle(offset, size, data_buf, complete_fn)?;
        Ok(Box::new(waiter))
    }

    fn write_page_async(
        &self,
        idx: usize,
        locked_page: LockedCachePage,
    ) -> Result<Box<dyn PageCacheIoWaiter>> {
        let offset = idx.checked_mul(PAGE_SIZE).ok_or_else(|| {
            Error::with_message(Errno::EOVERFLOW, "virtiofs page offset overflow")
        })?;

        locked_page.wait_until_finish_writing_back();

        let file_size = self.size();
        if offset >= file_size {
            locked_page.set_writing_back();
            locked_page.set_up_to_date();
            let page = locked_page.unlock();
            cache_page::clear_writing_back(&page);
            return Ok(Box::new(FuseWaiter::complete()));
        }

        let size = (file_size - offset).min(PAGE_SIZE);
        let fs = self.fs_ref();
        let data_buf = fs.session.alloc_write_buf(size)?;
        let mut page_reader = locked_page.reader();
        page_reader.limit(size);
        data_buf
            .writer()
            .unwrap()
            .write_fallible(&mut page_reader.to_fallible())?;

        locked_page.set_writing_back();
        locked_page.set_up_to_date();

        let page = locked_page.unlock();

        if let Some(open_handle) = self.open_handles.find_writable_handle() {
            let submit_page = page.clone();
            let complete_fn: FuseCompleteFn = Box::new(move |status| {
                cache_page::clear_writing_back(&submit_page);
                if status != FuseStatus::Complete {
                    ostd::error!(
                        "virtiofs writeback failed for page index {idx} with status {status:?}; data may be lost"
                    );
                }
            });

            match fs.session.write_async(
                self.nodeid(),
                WriteIn::new(
                    open_handle.fh(),
                    offset as u64,
                    size as u32,
                    open_handle.file_flags(),
                    WriteFlags::empty(),
                ),
                data_buf,
                Some(complete_fn),
            ) {
                Ok(waiter) => return Ok(Box::new(waiter)),
                Err(err) => {
                    let locked_page = page.lock();
                    locked_page.set_dirty();
                    cache_page::clear_writing_back(&locked_page);
                    return Err(err.into());
                }
            }
        }

        let submit_page = page.clone();
        let complete_fn: FuseCompleteFn = Box::new(move |status| {
            cache_page::clear_writing_back(&submit_page);
            if status != FuseStatus::Complete {
                ostd::error!(
                    "virtiofs writeback failed for page index {idx} with status {status:?}; data may be lost"
                );
            }
        });
        match self.write_buf_with_transient_handle(
            offset,
            size,
            WriteFlags::empty(),
            data_buf,
            complete_fn,
        ) {
            Ok(waiter) => Ok(Box::new(waiter)),
            Err(err) => {
                let locked_page = page.lock();
                locked_page.set_dirty();
                cache_page::clear_writing_back(&locked_page);
                Err(err)
            }
        }
    }

    fn npages(&self) -> usize {
        self.size().div_ceil(PAGE_SIZE)
    }
}

impl PageCacheIoWaiter for FuseWaiter {
    fn wait(&self) -> Result<()> {
        if FuseWaiter::wait(self) != FuseStatus::Complete {
            return_errno!(Errno::EIO);
        }

        Ok(())
    }

    fn concat(&mut self, other: Box<dyn PageCacheIoWaiter>) -> Result<()> {
        let other: Box<dyn Any + Send + Sync> = other;
        let Ok(other) = other.downcast::<Self>() else {
            return_errno_with_message!(Errno::EINVAL, "cannot concatenate different waiter types");
        };

        FuseWaiter::concat(self, *other);
        Ok(())
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
