// SPDX-License-Identifier: MPL-2.0

//! Page cache backend implementation for `VirtioFsInode`.

use core::ops::Deref;

use aster_fuse::{FuseCompletion, FuseFileHandle, ReadIn, WriteFlags, WriteIn};
use io_util::batch::IoBatch;
use ostd::mm::{Segment, io::util::HasVmReaderWriter};

use super::{
    super::open_handle::{TransientHandle, VirtioFsOpenHandle},
    VirtioFsInode,
};
use crate::{
    fs::file::AccessMode,
    prelude::*,
    vm::page_cache::{
        CachePageExt, PageCacheBackend,
        cache_page::{self, LockedCachePage},
    },
};

impl PageCacheBackend for VirtioFsInode {
    fn read_page_async(
        &self,
        idx: usize,
        locked_page: LockedCachePage,
        io_batch: &mut IoBatch,
    ) -> Result<()> {
        let handle = self.readable_page_handle()?;
        let nodeid = self.nodeid();
        let session = self.fs_ref().session().clone();
        let cache_page = locked_page.deref().clone();
        let data_buf = session.new_map_read_buf(Segment::from(cache_page.clone()).into())?;
        let read_in = ReadIn::new(
            handle.fh(),
            page_offset(idx)? as u64,
            PAGE_SIZE as u32,
            handle.file_flags(),
        );

        let complete_fn = move |status| {
            if let FuseCompletion::Complete(payload_len) = status {
                if payload_len > PAGE_SIZE {
                    ostd::error!(
                        "virtiofs read failed for page index {}; payload length {} exceeds page size",
                        idx,
                        payload_len
                    );
                } else if payload_len < PAGE_SIZE {
                    let mut writer = cache_page.writer();
                    writer.skip(payload_len);
                    writer.fill_zeros(PAGE_SIZE - payload_len);
                    locked_page.set_up_to_date();
                } else {
                    locked_page.set_up_to_date();
                }
            }
            // Keep the handle alive until the request completes
            // or until the completion closure is dropped on submission failure.
            drop(handle);
        };

        match session.read_async(nodeid, read_in, data_buf, Some(Box::new(complete_fn))) {
            Ok(waiter) => {
                io_batch.push(waiter);
                Ok(())
            }
            Err(err) => Err(err.into()),
        }
    }

    fn write_page_async(
        &self,
        idx: usize,
        locked_page: LockedCachePage,
        io_batch: &mut IoBatch,
    ) -> Result<()> {
        locked_page.wait_until_finish_writing_back();

        let fs = self.fs_ref();
        let data_buf = fs.session().alloc_write_buf(PAGE_SIZE)?;
        let page_reader = locked_page.reader();

        data_buf
            .writer()
            .unwrap()
            .write_fallible(&mut page_reader.to_fallible())?;

        locked_page.set_writing_back();
        locked_page.set_up_to_date();

        let page = locked_page.unlock();

        let handle = match self.writable_page_handle() {
            Ok(handle) => handle,
            Err(err) => {
                let locked_page = page.lock();
                locked_page.set_dirty();
                cache_page::clear_writing_back(&locked_page);
                return Err(err);
            }
        };

        let nodeid = self.nodeid();
        let session = fs.session().clone();
        let complete_page = page.clone();
        let write_in = WriteIn::new(
            handle.fh(),
            page_offset(idx)? as u64,
            PAGE_SIZE as u32,
            handle.file_flags(),
            WriteFlags::empty(),
        );

        let complete_fn = move |status| {
            cache_page::clear_writing_back(&complete_page);
            if !matches!(status, FuseCompletion::Complete(_)) {
                ostd::error!(
                    "virtiofs writeback failed for page index {}; data may be lost",
                    idx
                );
            }
            // Keep the handle alive until the request completes
            // or until the completion closure is dropped on submission failure.
            drop(handle);
        };

        match session.write_async(nodeid, write_in, data_buf, Some(Box::new(complete_fn))) {
            Ok(waiter) => {
                io_batch.push(waiter);
                Ok(())
            }
            Err(err) => {
                let locked_page = page.lock();
                locked_page.set_dirty();
                cache_page::clear_writing_back(&locked_page);
                Err(err.into())
            }
        }
    }
}

impl VirtioFsInode {
    fn readable_page_handle(&self) -> Result<PageIoHandle> {
        if let Some(open_handle) = self.open_handles.find_readable_handle() {
            return Ok(PageIoHandle::Cached(open_handle));
        }

        Ok(PageIoHandle::Transient(TransientHandle::new(
            self.open_transient_handle(AccessMode::O_RDONLY)?,
        )))
    }

    fn writable_page_handle(&self) -> Result<PageIoHandle> {
        if let Some(open_handle) = self.open_handles.find_writable_handle() {
            return Ok(PageIoHandle::Cached(open_handle));
        }

        Ok(PageIoHandle::Transient(TransientHandle::new(
            self.open_transient_handle(AccessMode::O_RDWR)?,
        )))
    }

    pub(in super::super) fn invalidate_whole_page_cache(&self) -> Result<()> {
        let _size_guard = self.size_lock.write();

        self.invalidate_whole_page_cache_locked()
    }

    /// Invalidates the whole page cache, if any.
    ///
    /// The caller must hold `size_lock` before calling this function.
    pub(super) fn invalidate_whole_page_cache_locked(&self) -> Result<()> {
        let Some(page_cache) = &self.page_cache else {
            return Ok(());
        };

        let cached_size = page_cache.size();
        if cached_size > 0 {
            page_cache.invalidate_range(0..cached_size)?;
        }

        Ok(())
    }
}

fn page_offset(idx: usize) -> Result<usize> {
    idx.checked_mul(PAGE_SIZE)
        .ok_or_else(|| Error::with_message(Errno::EOVERFLOW, "virtiofs page offset overflow"))
}

enum PageIoHandle {
    Cached(Arc<VirtioFsOpenHandle>),
    Transient(TransientHandle),
}

impl PageIoHandle {
    fn open_handle(&self) -> &VirtioFsOpenHandle {
        match self {
            Self::Cached(handle) => handle.as_ref(),
            Self::Transient(handle) => handle.deref(),
        }
    }

    fn fh(&self) -> FuseFileHandle {
        self.open_handle().fh()
    }

    fn file_flags(&self) -> u32 {
        self.open_handle().file_flags()
    }
}
