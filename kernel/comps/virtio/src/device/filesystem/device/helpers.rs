// SPDX-License-Identifier: MPL-2.0

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
        in_header: InHeader,
        body_segments: &[&[u8]],
    ) -> Result<FsInBuf, VirtioDeviceError> {
        let body_len = body_segments
            .iter()
            .try_fold(0usize, |len, segment| len.checked_add(segment.len()))
            .ok_or(VirtioDeviceError::QueueUnknownError)?;
        let total_len = size_of::<InHeader>()
            .checked_add(body_len)
            .ok_or(VirtioDeviceError::QueueUnknownError)?;
        let in_buf = self.alloc_to_device_buf(total_len)?;

        in_buf.write_val(0, &in_header).unwrap();

        let mut offset = size_of::<InHeader>();
        for segment in body_segments {
            in_buf.write_bytes(offset, segment).unwrap();
            offset += segment.len();
        }

        in_buf
            .mem_obj()
            .sync_to_device(in_buf.offset().clone())
            .unwrap();

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

    pub(super) fn prepare_fuse_request(
        &self,
        opcode: u32,
        nodeid: u64,
        body_segments: &[&[u8]],
        out_payload_size: Option<usize>,
    ) -> Result<FuseRequest, VirtioDeviceError> {
        let unique = self.alloc_unique();
        let body_len = body_segments
            .iter()
            .try_fold(0usize, |len, segment| len.checked_add(segment.len()))
            .ok_or(VirtioDeviceError::QueueUnknownError)?;
        let in_len = size_of::<InHeader>()
            .checked_add(body_len)
            .ok_or(VirtioDeviceError::QueueUnknownError)?;
        let in_header = InHeader::new(in_len as u32, opcode, unique, nodeid);
        let in_buf = self.prepare_in_buf(in_header, body_segments)?;
        let out_buf = out_payload_size
            .map(|payload_size| self.prepare_out_buf(payload_size))
            .transpose()?;

        Ok(FuseRequest::new(unique, nodeid, in_buf, out_buf))
    }

    fn alloc_to_device_buf(&self, len: usize) -> Result<FsInBuf, VirtioDeviceError> {
        self.to_device_pool.alloc(len)
    }

    fn alloc_from_device_buf(&self, len: usize) -> Result<FsOutBuf, VirtioDeviceError> {
        self.from_device_pool.alloc(len)
    }
}
