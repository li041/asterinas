// SPDX-License-Identifier: MPL-2.0

use super::*;

// (in_header, in_payload, out_header, out_payload)
type FsRequestSlices = (FsInBuf, FsInBuf, FsOutBuf, FsOutBuf);

impl FileSystemDevice {
    pub(super) fn parse_tag(raw_tag: &[u8; 36]) -> String {
        let len = raw_tag
            .iter()
            .position(|&byte| byte == 0)
            .unwrap_or(raw_tag.len());

        match core::str::from_utf8(&raw_tag[..len]) {
            Ok(tag) => tag.to_string(),
            Err(_) => "<invalid-tag>".to_string(),
        }
    }

    pub(super) fn alloc_unique(&self) -> u64 {
        self.next_unique.fetch_add(1, Ordering::Relaxed)
    }

    pub(super) fn prepare_in_header_buf(
        &self,
        in_header: InHeader,
    ) -> Result<FsInBuf, VirtioDeviceError> {
        let in_header_buf = self.alloc_to_device_buf(size_of::<InHeader>())?;
        in_header_buf.write_val(0, &in_header).unwrap();
        in_header_buf
            .mem_obj()
            .sync_to_device(in_header_buf.offset().clone())
            .unwrap();
        Ok(in_header_buf)
    }

    pub(super) fn prepare_in_payload_buf<T: Pod>(
        &self,
        in_payload: T,
    ) -> Result<FsInBuf, VirtioDeviceError> {
        let in_payload_buf = self.alloc_to_device_buf(size_of::<T>())?;
        in_payload_buf.write_val(0, &in_payload).unwrap();
        in_payload_buf
            .mem_obj()
            .sync_to_device(in_payload_buf.offset().clone())
            .unwrap();
        Ok(in_payload_buf)
    }

    pub(super) fn prepare_in_name_buf(&self, name: &str) -> Result<FsInBuf, VirtioDeviceError> {
        let in_name_buf = self.alloc_to_device_buf(name.len() + 1)?;

        {
            let mut writer = in_name_buf.writer().unwrap();
            let mut value_reader = VmReader::from(name.as_bytes());
            let _ = writer.write(&mut value_reader);
            let nul: [u8; 1] = [0u8];
            let mut nul_reader = VmReader::from(&nul[..]);
            let _ = writer.write(&mut nul_reader);
        }

        in_name_buf
            .mem_obj()
            .sync_to_device(in_name_buf.offset().clone())
            .unwrap();

        Ok(in_name_buf)
    }

    pub(super) fn prepare_in_data_buf(&self, data: &[u8]) -> Result<FsInBuf, VirtioDeviceError> {
        let in_data_buf = self.alloc_to_device_buf(data.len())?;
        {
            let mut writer = in_data_buf.writer().unwrap();
            let mut data_reader = VmReader::from(data);
            let _ = writer.write(&mut data_reader);
        }
        in_data_buf
            .mem_obj()
            .sync_to_device(in_data_buf.offset().clone())
            .unwrap();
        Ok(in_data_buf)
    }

    pub(super) fn prepare_out_header_buf(&self) -> Result<FsOutBuf, VirtioDeviceError> {
        self.alloc_from_device_buf(size_of::<OutHeader>())
    }

    pub(super) fn prepare_out_payload_buf(
        &self,
        size: usize,
    ) -> Result<FsOutBuf, VirtioDeviceError> {
        self.alloc_from_device_buf(size)
    }

    pub(super) fn prepare_request_slices<T: Pod>(
        &self,
        in_header: InHeader,
        in_payload: T,
        out_payload_size: usize,
    ) -> Result<FsRequestSlices, VirtioDeviceError> {
        let in_header_slice = self.prepare_in_header_buf(in_header)?;
        let in_payload_slice = self.prepare_in_payload_buf(in_payload)?;
        let out_header_slice = self.prepare_out_header_buf()?;
        let out_payload_slice = self.prepare_out_payload_buf(out_payload_size)?;
        Ok((
            in_header_slice,
            in_payload_slice,
            out_header_slice,
            out_payload_slice,
        ))
    }

    fn alloc_to_device_buf(&self, len: usize) -> Result<FsInBuf, VirtioDeviceError> {
        self.to_device_pool.alloc(len)
    }

    fn alloc_from_device_buf(&self, len: usize) -> Result<FsOutBuf, VirtioDeviceError> {
        self.from_device_pool.alloc(len)
    }
}
