// SPDX-License-Identifier: MPL-2.0

use alloc::vec;

use super::*;

impl Transport9PDevice {
    pub(super) fn parse_tag(raw_tag: &[u8; 36], tag_len: u16) -> String {
        let len = (tag_len as usize).min(raw_tag.len());
        let end = raw_tag[..len]
            .iter()
            .position(|&byte| byte == 0)
            .unwrap_or(len);

        match core::str::from_utf8(&raw_tag[..end]) {
            Ok(tag) => tag.to_string(),
            Err(_) => "<invalid-tag>".to_string(),
        }
    }

    pub(super) fn alloc_tag_id(&self) -> u16 {
        (self.tag_alloc.alloc() as u16) & 0x7FFF
    }

    pub(super) fn alloc_unique(&self) -> u64 {
        self.tag_alloc.alloc() as u64
    }

    fn alloc_to_device_buf(&self, size: usize) -> P9DmaBuf {
        self.dma_pools.alloc_to_device(size)
    }

    fn alloc_from_device_buf(&self, size: usize) -> P9DmaBuf {
        self.dma_pools.alloc_from_device(size)
    }

    /// Prepare a to-device buffer containing raw bytes.
    pub(super) fn prepare_request_buf(&self, data: &[u8]) -> Slice<P9DmaBuf> {
        let buf = self.alloc_to_device_buf(data.len());
        let slice = Slice::new(buf.clone(), 0..data.len());
        {
            let mut writer = buf.writer().unwrap();
            let mut reader = VmReader::from(data);
            let _ = writer.write(&mut reader);
        }
        slice
            .mem_obj()
            .sync_to_device(slice.offset().clone())
            .unwrap();
        slice
    }

    /// Prepare a from-device buffer of given size for response.
    pub(super) fn prepare_response_buf(&self, size: usize) -> Slice<P9DmaBuf> {
        let buf = self.alloc_from_device_buf(size);
        Slice::new(buf.clone(), 0..size)
    }

    /// Read the response bytes from a from-device slice.
    pub(super) fn read_response_bytes(&self, slice: &Slice<P9DmaBuf>) -> Vec<u8> {
        slice
            .mem_obj()
            .sync_from_device(slice.offset().clone())
            .unwrap();

        let mut data = vec![0u8; slice.size()];
        let mut reader = slice.reader().unwrap();
        reader.read(&mut VmWriter::from(data.as_mut_slice()));
        data
    }

    /// Send a 9P request and wait for response. Returns the raw response bytes.
    pub(super) fn send_9p_request(
        &self,
        request_data: &[u8],
        max_response_size: usize,
    ) -> Result<Vec<u8>, VirtioDeviceError> {
        let unique = self.alloc_unique();

        let req_slice = self.prepare_request_buf(request_data);
        let resp_slice = self.prepare_response_buf(max_response_size);

        self.submit_request_and_wait(unique, &[&req_slice], &[&resp_slice])?;

        let response = self.read_response_bytes(&resp_slice);
        Ok(response)
    }

    /// Send a 9P request during early boot (spin-wait).
    pub(super) fn send_9p_request_early(
        &self,
        request_data: &[u8],
        max_response_size: usize,
    ) -> Result<Vec<u8>, VirtioDeviceError> {
        let unique = self.alloc_unique();

        let req_slice = self.prepare_request_buf(request_data);
        let resp_slice = self.prepare_response_buf(max_response_size);

        self.submit_request(unique, &[&req_slice], &[&resp_slice])?;
        self.wait_for_unique_early(unique as usize)?;

        let response = self.read_response_bytes(&resp_slice);
        Ok(response)
    }

    /// Check a 9P response for errors. Returns the body (after header).
    pub(super) fn check_9p_response(
        response: &[u8],
        expected_type: u8,
    ) -> Result<&[u8], VirtioDeviceError> {
        if let Some(errno) = check_rlerror(response) {
            return Err(VirtioDeviceError::FileSystemError(-(errno as i32)));
        }

        let (_size, msg_type, _tag) =
            parse_header(response).ok_or(VirtioDeviceError::QueueUnknownError)?;

        if msg_type != expected_type {
            warn!(
                "9P unexpected response type: got {}, expected {}",
                msg_type, expected_type
            );
            return Err(VirtioDeviceError::QueueUnknownError);
        }

        Ok(&response[P9_HEADER_SIZE..])
    }
}
