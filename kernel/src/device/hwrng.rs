// SPDX-License-Identifier: MPL-2.0

use alloc::{boxed::Box, string::String, sync::Arc};
use core::sync::atomic::{Ordering, fence};

use aster_bigtcp::time::Duration;
use aster_block::request_queue;
use aster_util::mem_obj_slice::Slice;
use aster_virtio::device::entropy::{
    all_devices, device::EntropyDevice, get_first_device, register_recv_callback,
};
use device_id::{DeviceId, MajorId, MinorId};
use ostd::{
    mm::{VmReader, VmWriter, io_util::HasVmReaderWriter},
    sync::{Waiter, Waker},
};

use crate::{
    device::registry::char,
    events::IoEvents,
    fs::{
        device::{Device, DeviceType},
        inode_handle::FileIo,
        utils::{InodeIo, StatusFlags},
    },
    prelude::*,
    process::signal::{PollHandle, Pollable, Pollee},
};

static HW_RNG_DEVICE: Mutex<Option<Arc<HwRngDevice>>> = Mutex::new(None);

#[derive(Clone)]
struct HwRngDevice {
    rng: Arc<EntropyDevice>,
    pollee: Pollee,
}

impl HwRngDevice {
    pub fn new(rng: Arc<EntropyDevice>) -> Self {
        Self {
            rng,
            pollee: Pollee::new(),
        }
    }

    pub fn check_io_events(&self) -> IoEvents {
        let mut events = IoEvents::empty();

        if self.rng.can_pop() {
            events |= IoEvents::IN;
        }

        events
    }
}

struct HwRngHandle;

impl Device for HwRngHandle {
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
        let mut device_lock = HW_RNG_DEVICE.lock();
        let hwrng_device = match &*device_lock {
            Some(hwrng_device) => hwrng_device.clone(),
            None => {
                let device = get_first_device().ok_or_else(|| {
                    Error::with_message(Errno::ENODEV, "No hardware RNG device found")
                })?;
                let hwrng_handle = Arc::new(HwRngDevice::new(device));
                *device_lock = Some(hwrng_handle.clone());
                hwrng_handle
            }
        };

        Ok(Box::new((*hwrng_device).clone()))
    }
}

impl Pollable for HwRngDevice {
    fn poll(&self, mask: IoEvents, poller: Option<&mut PollHandle>) -> IoEvents {
        self.pollee
            .poll_with(mask, poller, || self.check_io_events())
    }
}

impl InodeIo for HwRngDevice {
    // fn read_at(
    //     &self,
    //     _offset: usize,
    //     writer: &mut VmWriter,
    //     _status_flags: StatusFlags,
    // ) -> Result<usize> {
    //     let mut buf = vec![0u8; writer.avail().min(PAGE_SIZE)];
    //     let mut written_bytes = 0;

    //     while writer.has_avail() {
    //         self.rng.getrandom(&mut buf);
    //         match writer.write_fallible(&mut VmReader::from(buf.as_slice())) {
    //             Ok(len) => written_bytes += len,
    //             Err((err, 0)) if written_bytes == 0 => return Err(err.into()),
    //             Err((_, len)) => return Ok(written_bytes + len),
    //         }
    //     }

    //     Ok(written_bytes)
    // }
    fn read_at(
        &self,
        _offset: usize,
        writer: &mut VmWriter,
        status_flags: StatusFlags,
    ) -> Result<usize> {
        let len = writer.avail();
        // error!("HwRngHandle read_at len {}", len);
        let mut written_bytes = 0;
        let is_nonblocking = status_flags.contains(StatusFlags::O_NONBLOCK);

        while written_bytes < len {
            // 放入接收缓冲区
            let to_read = (len - written_bytes).min(PAGE_SIZE);

            {
                let mut request_queue = self.rng.request_queue.lock();
                self.rng
                    .activate_receive_buffer(&mut request_queue, to_read);
            }

            let try_read_once = |writer: &mut VmWriter| -> Result<usize> {
                // if !self.rng.can_pop() {
                //     // // lxh debug
                //     log::error!("rng can't pop, return EAGAIN");
                //     return_errno_with_message!(Errno::EAGAIN, "entropy buffer not ready");
                // }
                let mut request_queue = self.rng.request_queue.lock();
                let Ok((_, used_len)) = request_queue.pop_used() else {
                    // // lxh debug
                    // log::warn!("rng fail to pop used, return EAGAIN");
                    return_errno_with_message!(Errno::EAGAIN, "entropy buffer not ready");
                };
                drop(request_queue);

                let used_len = (used_len as usize).min(to_read);
                self.rng
                    .receive_buffer
                    .sync_from_device(0..used_len)
                    .unwrap();

                let mut reader = self.rng.receive_buffer.reader().unwrap();
                reader.limit(used_len);
                let copied = reader.read_fallible(writer).map_err(|(err, _)| err)?;
                log::info!("HwRng: successfully read {} bytes", copied);
                self.pollee.invalidate();
                // log::error!("HwRngHandle read {} bytes", copied);
                Ok(copied)
            };

            let copied = if is_nonblocking {
                try_read_once(writer)?
            } else {
                self.wait_events(IoEvents::IN, None, || try_read_once(writer))?
            };

            written_bytes += copied;
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

impl FileIo for HwRngDevice {
    fn check_seekable(&self) -> Result<()> {
        Ok(())
    }

    fn is_offset_aware(&self) -> bool {
        false
    }
}

pub(super) fn init_in_first_process() -> Result<()> {
    register_recv_callback(|| {
        let device_lock = HW_RNG_DEVICE.lock();
        if let Some(hwrng_handle) = &*device_lock {
            log::error!("HwRngHandle notify IN event");
            hwrng_handle.pollee.notify(IoEvents::IN);
        }
    });

    char::register(Arc::new(HwRngHandle))?;

    Ok(())
}
