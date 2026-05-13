// SPDX-License-Identifier: MPL-2.0

//! Page cache backend implementation for `VirtioFsInode`.

use alloc::{boxed::Box, sync::Arc};
use core::ops::Deref;

use aster_fuse::{FuseCompleteFn, ReadIn, WriteFlags, WriteIn};
use aster_util::mem_obj_slice::Slice;
use aster_virtio::device::filesystem::{
    device::{FuseSession, FuseWaiter},
    pool::FsDmaStorage,
};
use io_util::{IoBatch, IoCompletion, IoError};
use ostd::{
    mm::{io::util::HasVmReaderWriter, Segment, PAGE_SIZE},
    sync::{LocalIrqDisabled, SpinLock},
};

use super::VirtioFsInode;
use crate::{
    prelude::*,
    vm::page_cache::{
        cache_page::{self, CachePage, LockedCachePage},
        CachePageExt, PageCacheBackend,
    },
};

impl PageCacheBackend for VirtioFsInode {
    fn read_page_async(
        &self,
        idx: usize,
        locked_page: LockedCachePage,
        io_batch: &mut IoBatch,
    ) -> Result<()> {
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

        if let Some(open_handle) = self.open_handles.find_readable_handle() {
            let fs = self.fs_ref();
            let session = fs.session.clone();
            let read_in = ReadIn::new(
                open_handle.fh(),
                offset as u64,
                size,
                open_handle.file_flags(),
            );
            let waiter = session.read_async(self.nodeid(), read_in, data_buf, None)?;
            io_batch.push(Arc::new(FusePageReadCompletion::new(
                session,
                waiter,
                locked_page,
            )));
            return Ok(());
        }

        let complete_fn: FuseCompleteFn = Box::new(|_| {});
        let session = self.fs_ref().session.clone();
        let waiter = self.read_buf_with_transient_handle(offset, size, data_buf, complete_fn)?;
        io_batch.push(Arc::new(FusePageReadCompletion::new(
            session,
            waiter,
            locked_page,
        )));
        Ok(())
    }

    fn write_page_async(
        &self,
        idx: usize,
        locked_page: LockedCachePage,
        io_batch: &mut IoBatch,
    ) -> Result<()> {
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
            return Ok(());
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
            let session = fs.session.clone();
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
                None,
            ) {
                Ok(waiter) => {
                    io_batch.push(Arc::new(FusePageWriteCompletion::new(
                        session, waiter, page, idx,
                    )));
                    return Ok(());
                }
                Err(err) => {
                    let locked_page = page.lock();
                    locked_page.set_dirty();
                    cache_page::clear_writing_back(&locked_page);
                    return Err(err.into());
                }
            }
        }

        let complete_fn: FuseCompleteFn = Box::new(|_| {});
        let session = self.fs_ref().session.clone();
        match self.write_buf_with_transient_handle(
            offset,
            size,
            WriteFlags::empty(),
            data_buf,
            complete_fn,
        ) {
            Ok(waiter) => {
                io_batch.push(Arc::new(FusePageWriteCompletion::new(
                    session, waiter, page, idx,
                )));
                Ok(())
            }
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

struct FusePageReadCompletion {
    session: Arc<FuseSession>,
    waiter: Arc<FuseWaiter>,
    locked_page: SpinLock<Option<LockedCachePage>, LocalIrqDisabled>,
}

impl FusePageReadCompletion {
    fn new(
        session: Arc<FuseSession>,
        waiter: Arc<FuseWaiter>,
        locked_page: LockedCachePage,
    ) -> Self {
        Self {
            session,
            waiter,
            locked_page: SpinLock::new(Some(locked_page)),
        }
    }
}

impl IoCompletion for FusePageReadCompletion {
    fn wait(&self) -> core::result::Result<(), IoError> {
        let result = self
            .session
            .wait_for_reply_header(&self.waiter)
            .map_err(|_| IoError::Failed);

        if let Some(locked_page) = self.locked_page.lock().take() {
            if result.is_ok() {
                locked_page.set_up_to_date();
            }
        }

        result
    }
}

struct FusePageWriteCompletion {
    session: Arc<FuseSession>,
    waiter: Arc<FuseWaiter>,
    page: CachePage,
    page_idx: usize,
}

impl FusePageWriteCompletion {
    fn new(
        session: Arc<FuseSession>,
        waiter: Arc<FuseWaiter>,
        page: CachePage,
        page_idx: usize,
    ) -> Self {
        Self {
            session,
            waiter,
            page,
            page_idx,
        }
    }
}

impl IoCompletion for FusePageWriteCompletion {
    fn wait(&self) -> core::result::Result<(), IoError> {
        let result = self
            .session
            .wait_for_reply_header(&self.waiter)
            .map_err(|_| IoError::Failed);
        cache_page::clear_writing_back(&self.page);

        if result.is_err() {
            ostd::error!(
                "virtiofs writeback failed for page index {}; data may be lost",
                self.page_idx
            );
        }

        result
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
