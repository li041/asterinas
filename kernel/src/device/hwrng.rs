// SPDX-License-Identifier: MPL-2.0

use alloc::{boxed::Box, string::String, sync::Arc};

use aster_virtio::device::entropy::{all_devices, device::EntropyDevice};
use device_id::{DeviceId, MajorId, MinorId};
use ostd::mm::{VmReader, VmWriter};

use crate::{
    device::registry::char,
    events::IoEvents,
    fs::{
        device::{Device, DeviceType},
        inode_handle::FileIo,
        utils::{InodeIo, StatusFlags},
    },
    prelude::*,
    process::signal::{PollHandle, Pollable},
};

#[derive(Clone)]
struct HwRngHandle {
    rng: Arc<EntropyDevice>,
}

impl HwRngHandle {
    pub fn new(rng: Arc<EntropyDevice>) -> Self {
        Self { rng }
    }
}

struct HwRngDevice;

impl Device for HwRngDevice {
    fn type_(&self) -> DeviceType {
        DeviceType::Char
    }

    fn id(&self) -> DeviceId {
        // Same Value with Linux: major 10, minor 183
        device_id::DeviceId::new(MajorId::new(10), MinorId::new(183))
    }

    fn devtmpfs_path(&self) -> Option<String> {
        Some("hwrng".into())
    }

    fn open(&self) -> Result<Box<dyn FileIo>> {
        let device = all_devices()
            .first()
            .cloned()
            .ok_or_else(|| Error::with_message(Errno::ENODEV, "No hardware RNG device found"))?;

        Ok(Box::new(HwRngHandle::new(device)))
    }
}

impl Pollable for HwRngHandle {
    fn poll(&self, mask: IoEvents, _poller: Option<&mut PollHandle>) -> IoEvents {
        let events = IoEvents::IN | IoEvents::OUT;
        events & mask
    }
}

impl InodeIo for HwRngHandle {
    fn read_at(
        &self,
        _offset: usize,
        writer: &mut VmWriter,
        _status_flags: StatusFlags,
    ) -> Result<usize> {
        let mut buf = vec![0u8; writer.avail().min(PAGE_SIZE)];
        let mut written_bytes = 0;

        while writer.has_avail() {
            self.rng.getrandom(&mut buf);
            match writer.write_fallible(&mut VmReader::from(buf.as_slice())) {
                Ok(len) => written_bytes += len,
                Err((err, 0)) if written_bytes == 0 => return Err(err.into()),
                Err((_, len)) => return Ok(written_bytes + len),
            }
        }

        Ok(written_bytes)
    }

    fn write_at(
        &self,
        _offset: usize,
        reader: &mut VmReader,
        _status_flags: StatusFlags,
    ) -> Result<usize> {
        let len = reader.remain();
        reader.skip(len);
        Ok(len)
    }
}

impl FileIo for HwRngHandle {
    fn check_seekable(&self) -> Result<()> {
        Ok(())
    }

    fn is_offset_aware(&self) -> bool {
        false
    }
}

pub(super) fn init_in_first_process() -> Result<()> {
    char::register(Arc::new(HwRngDevice))?;

    Ok(())
}
