// SPDX-License-Identifier: MPL-2.0

//! Page cache backend implementation for `VirtioFsInode`.

use core::ops::Deref;

use aster_fuse::{FuseCompletion, ReadReq, WriteFlags, WriteReq};
use aster_virtio::device::filesystem::pool::FuseReplyBuf;
use io_util::batch::IoBatch;
use ostd::mm::{Segment, io::util::HasVmReaderWriter};

use super::{super::open_handle::VirtioFsOpenHandle, VirtioFsInode};
use crate::{
    fs::file::AccessMode,
    prelude::*,
    vm::page_cache::{CachePageExt, LockedCachePage, PageCacheBackend},
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
        let data_buf = FuseReplyBuf::new_map(Segment::from(cache_page.clone()).into())?;
        // FIXME: Page-cache I/O should use the current `InodeHandle` status
        // flags instead of the flags captured in the cached FUSE handle. The
        // page-cache backend currently receives only the inode, so it cannot
        // observe per-open status flag changes.
        let read_req = ReadReq::new(
            handle.fh(),
            page_offset(idx)? as u64,
            PAGE_SIZE as u32,
            handle.file_flags(),
        );

        let complete_fn = move |status| {
            let FuseCompletion::Complete(payload_len) = status else {
                return;
            };

            if payload_len > PAGE_SIZE {
                ostd::error!(
                    "virtiofs read failed for page index {}; payload length {} exceeds page size",
                    idx,
                    payload_len
                );
            } else {
                let mut writer = cache_page.writer();
                writer.skip(payload_len);
                writer.fill_zeros(PAGE_SIZE - payload_len);
                locked_page.set_up_to_date();
            }
            // Keep the handle alive until the request completes
            // or until the completion closure is dropped on submission failure.
            drop(handle);
        };

        match session.read_async(nodeid, read_req, data_buf, Some(Box::new(complete_fn))) {
            Ok(waiter) => {
                io_batch.push(waiter);
                Ok(())
            }
            Err(err) => Err(err.into()),
        }
    }

    fn read_pages_async(
        &self,
        pages: Vec<(usize, LockedCachePage)>,
        io_batch: &mut IoBatch,
    ) -> Result<()> {
        if pages.is_empty() {
            return_errno_with_message!(Errno::EINVAL, "empty virtiofs page read");
        }
        if pages.len() == 1 {
            let (idx, locked_page) = pages
                .into_iter()
                .next()
                .ok_or_else(|| Error::with_message(Errno::EINVAL, "empty virtiofs page read"))?;
            return self.read_page_async(idx, locked_page, io_batch);
        }

        debug_assert!(!pages.is_empty());
        debug_assert!(
            pages
                .windows(2)
                .all(|pair| pair[0].0.checked_add(1) == Some(pair[1].0))
        );

        let max_pages = self.fs_ref().session().max_pages();
        let mut pages = pages.into_iter();
        loop {
            let chunk = pages.by_ref().take(max_pages).collect::<Vec<_>>();
            if chunk.is_empty() {
                break;
            }
            self.submit_read_pages(chunk, io_batch)?;
        }

        Ok(())
    }

    fn write_page_async(
        &self,
        idx: usize,
        locked_page: LockedCachePage,
        io_batch: &mut IoBatch,
    ) -> Result<()> {
        locked_page.wait_until_finish_writing_back();

        let page_start = page_offset(idx)?;
        let file_size = self.size();
        if page_start >= file_size {
            return_errno_with_message!(Errno::EINVAL, "virtiofs writeback page is beyond EOF");
        }
        let writeback_len = PAGE_SIZE.min(file_size - page_start);

        let fs = self.fs_ref();
        let data_buf = fs.session().alloc_write_buf(writeback_len)?;
        let mut page_reader = locked_page.reader();
        page_reader.limit(writeback_len);

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
                locked_page.clear_writing_back();
                return Err(err);
            }
        };

        let nodeid = self.nodeid();
        let session = fs.session().clone();
        let complete_page = page.clone();
        // FIXME: Page-cache I/O should use the current `InodeHandle` status
        // flags instead of the flags captured in the cached FUSE handle. The
        // page-cache backend currently receives only the inode, so it cannot
        // observe per-open status flag changes.
        let write_req = WriteReq::new(
            handle.fh(),
            page_start as u64,
            writeback_len as u32,
            handle.file_flags(),
            WriteFlags::empty(),
        );

        let complete_fn = move |status| {
            complete_page.clear_writing_back();
            if let FuseCompletion::MalformedResponse | FuseCompletion::RemoteError(_) = status {
                ostd::error!(
                    "virtiofs writeback failed for page index {}; data may be lost",
                    idx
                );
            }
            // Keep the handle alive until the request completes
            // or until the completion closure is dropped on submission failure.
            drop(handle);
        };

        match session.write_async(nodeid, write_req, data_buf, Some(Box::new(complete_fn))) {
            Ok(waiter) => {
                io_batch.push(waiter);
                Ok(())
            }
            Err(err) => {
                let locked_page = page.lock();
                locked_page.set_dirty();
                locked_page.clear_writing_back();
                Err(err.into())
            }
        }
    }
}

impl VirtioFsInode {
    fn submit_read_pages(
        &self,
        pages: Vec<(usize, LockedCachePage)>,
        io_batch: &mut IoBatch,
    ) -> Result<()> {
        let first_page_idx = pages
            .first()
            .map(|(idx, _)| *idx)
            .ok_or_else(|| Error::with_message(Errno::EINVAL, "empty virtiofs page read"))?;
        let request_len = pages
            .len()
            .checked_mul(PAGE_SIZE)
            .ok_or_else(|| Error::with_message(Errno::EOVERFLOW, "virtiofs read size overflow"))?;
        let request_len = u32::try_from(request_len)
            .map_err(|_| Error::with_message(Errno::EOVERFLOW, "virtiofs read size overflow"))?;

        let handle = self.readable_page_handle()?;
        let nodeid = self.nodeid();
        let session = self.fs_ref().session().clone();
        let data_buf = session.alloc_read_buf(request_len as usize)?;
        let complete_data_buf = data_buf.clone();
        // FIXME: Page-cache I/O should use the current `InodeHandle` status
        // flags instead of the flags captured in the cached FUSE handle. The
        // page-cache backend currently receives only the inode, so it cannot
        // observe per-open status flag changes.
        let read_req = ReadReq::new(
            handle.fh(),
            page_offset(first_page_idx)? as u64,
            request_len,
            handle.file_flags(),
        );

        let complete_fn = move |status| {
            let FuseCompletion::Complete(payload_len) = status else {
                return;
            };

            if payload_len > request_len as usize {
                ostd::error!(
                    "virtiofs read failed for page index {}; payload length {} exceeds request size {}",
                    first_page_idx,
                    payload_len,
                    request_len
                );
                return;
            }

            let mut data_reader = complete_data_buf.reader().unwrap();
            data_reader.limit(payload_len);
            let mut remaining = payload_len;
            for (_, locked_page) in pages {
                let page_data_len = remaining.min(PAGE_SIZE);
                let mut page_writer = locked_page.writer();
                page_writer.limit(PAGE_SIZE);
                let copied_len = page_writer.write(&mut data_reader);
                debug_assert_eq!(copied_len, page_data_len);
                page_writer.fill_zeros(PAGE_SIZE - page_data_len);
                locked_page.set_up_to_date();
                remaining -= page_data_len;
            }
            // Keep the handle alive until the request completes
            // or until the completion closure is dropped on submission failure.
            drop(handle);
        };

        match session.read_async(nodeid, read_req, data_buf, Some(Box::new(complete_fn))) {
            Ok(waiter) => {
                io_batch.push(waiter);
                Ok(())
            }
            Err(err) => Err(err.into()),
        }
    }
    fn readable_page_handle(&self) -> Result<Arc<VirtioFsOpenHandle>> {
        if let Some(open_handle) = self.open_handles.find_readable_handle() {
            return Ok(open_handle);
        }

        self.open_transient_handle(AccessMode::O_RDONLY)
    }

    fn writable_page_handle(&self) -> Result<Arc<VirtioFsOpenHandle>> {
        if let Some(open_handle) = self.open_handles.find_writable_handle() {
            return Ok(open_handle);
        }

        self.open_transient_handle(AccessMode::O_RDWR)
    }

    pub(in crate::fs::fs_impls::virtiofs) fn invalidate_whole_page_cache(&self) -> Result<()> {
        self.inner.write().invalidate_page_cache()
    }
}

fn page_offset(idx: usize) -> Result<usize> {
    idx.checked_mul(PAGE_SIZE)
        .ok_or_else(|| Error::with_message(Errno::EOVERFLOW, "virtiofs page offset overflow"))
}
