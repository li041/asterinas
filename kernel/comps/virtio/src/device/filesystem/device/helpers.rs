// SPDX-License-Identifier: MPL-2.0

//! Request construction helpers for [`FileSystemDevice`].
//!
//! This module provides small helpers for parsing device metadata, assigning
//! FUSE request IDs, and preparing DMA buffers for request submission.

use core::mem::size_of;

use ostd::mm::io::util::HasVmReaderWriter;

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
        nodeid: FuseNodeId,
        operation: &mut impl FuseOperation,
        unique: u64,
    ) -> Result<FsInBuf, FuseError> {
        let total_len = size_of::<InHeader>() + operation.body_len();
        let in_buf = self
            .to_device_pool
            .alloc_fs_buf(total_len)
            .map_err(FuseError::ResourceAlloc)?;

        let header = InHeader::new(total_len as u32, operation.opcode() as u32, unique, nodeid);

        let mut writer = in_buf.writer().unwrap();
        writer.write_val(&header).unwrap();
        operation.write_body(&mut writer)?;

        in_buf
            .mem_obj()
            .sync_to_device(in_buf.offset().clone())
            .unwrap();

        Ok(in_buf)
    }

    pub(super) fn prepare_out_buf(&self, payload_size: usize) -> Result<FsOutBuf, FuseError> {
        let total_len = size_of::<OutHeader>() + payload_size;
        self.from_device_pool
            .alloc_fs_buf(total_len)
            .map_err(FuseError::ResourceAlloc)
    }
}
