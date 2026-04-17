// SPDX-License-Identifier: MPL-2.0

//! DMA buffer preparation helpers for [`super::FileSystemDevice`].
//!
//! These helpers allocate and fill the in-buffer (host → device) and
//! out-buffer (device → host) for a FUSE request without any intermediate
//! heap allocation.

use core::mem::size_of;

use ostd_pod::IntoBytes;

use super::*;

impl FileSystemDevice {
    pub(super) fn parse_tag(raw_tag: &[u8; 36]) -> &str {
        let len = raw_tag
            .iter()
            .position(|&byte| byte == 0)
            .unwrap_or(raw_tag.len());

        match core::str::from_utf8(&raw_tag[..len]) {
            Ok(tag) => tag,
            Err(_) => "<invalid-tag>",
        }
    }

    pub(super) fn alloc_unique(&self) -> u64 {
        self.next_unique.fetch_add(1, Ordering::Relaxed)
    }

    pub(super) fn prepare_in_buf(
        &self,
        operation: &impl FuseOperation,
        unique: u64,
    ) -> Result<FsInBuf, VirtioDeviceError> {
        let body_len = operation.body_len();
        let total_len = size_of::<InHeader>()
            .checked_add(body_len)
            .ok_or(VirtioDeviceError::QueueUnknownError)?;
        let in_buf = self.alloc_to_device_buf(total_len)?;

        // Write header first — total_len is known before any body bytes.
        let len = u32::try_from(total_len).map_err(|_| VirtioDeviceError::QueueUnknownError)?;
        let header = InHeader::new(len, operation.opcode() as u32, unique, operation.nodeid());
        in_buf
            .write_bytes(0, header.as_bytes())
            .map_err(|_| VirtioDeviceError::QueueUnknownError)?;

        // Write body directly into the DMA buffer via a closure — no intermediate allocation.
        let mut pos = size_of::<InHeader>();
        operation.write_body(&mut |body_bytes| {
            let end = pos
                .checked_add(body_bytes.len())
                .ok_or(FuseError::LengthOverflow)?;
            if end > total_len {
                return Err(FuseError::BufferTooSmall);
            }
            in_buf
                .write_bytes(pos, body_bytes)
                .map_err(|_| FuseError::BufferTooSmall)?;
            pos = end;
            Ok(())
        })?;

        in_buf
            .mem_obj()
            .sync_to_device(in_buf.offset().clone())
            .map_err(|_| VirtioDeviceError::TransportUnknownError)?;

        Ok(in_buf)
    }

    pub(super) fn prepare_out_buf(
        &self,
        payload_size: usize,
    ) -> Result<FsOutBuf, VirtioDeviceError> {
        let total_len = size_of::<OutHeader>()
            .checked_add(payload_size)
            .ok_or(VirtioDeviceError::QueueUnknownError)?;
        self.alloc_from_device_buf(total_len)
    }

    fn alloc_to_device_buf(&self, len: usize) -> Result<FsInBuf, VirtioDeviceError> {
        self.to_device_pool.alloc(len)
    }

    fn alloc_from_device_buf(&self, len: usize) -> Result<FsOutBuf, VirtioDeviceError> {
        self.from_device_pool.alloc(len)
    }
}
