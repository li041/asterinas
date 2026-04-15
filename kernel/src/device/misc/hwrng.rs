// SPDX-License-Identifier: MPL-2.0

use alloc::{boxed::Box, string::String, sync::Arc};

use aster_virtio::device::entropy::{all_devices, device::EntropyDevice};
use device_id::{DeviceId, MinorId};
use ostd::mm::{VmReader, VmWriter};

use crate::{
    device::{Device, DeviceType, registry::char},
    events::IoEvents,
    fs::{
        file::{FileIo, StatusFlags},
        vfs::inode::InodeIo,
    },
    prelude::*,
    process::signal::{PollHandle, Pollable},
};

const HWRNG_MINOR: u32 = 183;

/// The currently in-use hardware RNG device.
//
// TODO: Users can select a device by writing its name to `/sys/class/misc/hw_random/rng_current`,
// which is not supported yet.
static RNG_CURRENT: Mutex<Option<Arc<EntropyDevice>>> = Mutex::new(None);

/// The `/dev/hwrng` device.
struct HwRngDevice {
    id: DeviceId,
}

impl HwRngDevice {
    fn new() -> Arc<Self> {
        let major = super::MISC_MAJOR.get().unwrap().get();
        let minor = MinorId::new(HWRNG_MINOR);

        let id = DeviceId::new(major, minor);
        Arc::new(Self { id })
    }
}

impl Device for HwRngDevice {
    fn type_(&self) -> DeviceType {
        DeviceType::Char
    }

    fn id(&self) -> DeviceId {
        self.id
    }

    fn devtmpfs_path(&self) -> Option<String> {
        Some("hwrng".into())
    }

    fn open(&self) -> Result<Box<dyn FileIo>> {
        Ok(Box::new(RngCurrent))
    }
}

/// A file handle opened from `/dev/hwrng`.
struct RngCurrent;

impl Pollable for RngCurrent {
    // Linux's `/dev/hwrng` does not implement `.poll`, so userspace sees the VFS
    // default ("always ready"). We mirror that contract here: `poll` reports
    // `IN | OUT` unconditionally and does not register `poller`, so `epoll`
    // clients should not rely on edge-triggered wake-ups from this device.
    // See `drivers/char/hw_random/core.c` (`rng_chrdev_ops`) upstream.
    fn poll(&self, mask: IoEvents, _poller: Option<&mut PollHandle>) -> IoEvents {
        mask & (IoEvents::IN | IoEvents::OUT)
    }
}

impl InodeIo for RngCurrent {
    fn read_at(
        &self,
        _offset: usize,
        writer: &mut VmWriter,
        status_flags: StatusFlags,
    ) -> Result<usize> {
        let dev = current_device()?;
        let len = writer.avail();
        let mut written_bytes = 0;
        let is_nonblocking = status_flags.contains(StatusFlags::O_NONBLOCK);

        while written_bytes < len {
            // Clone the writer so that the cursor does not advance on partial `write_fallible`.
            let mut new_writer = writer.clone_exclusive();

            let read_res = if is_nonblocking {
                try_read_entropy(dev.as_ref(), &mut new_writer)
            } else {
                dev.wait_queue().wait_until(|| {
                    match try_read_entropy(dev.as_ref(), &mut new_writer) {
                        Ok(copied) => Some(Ok(copied)),
                        Err(err) if err.error() == Errno::EAGAIN => None,
                        Err(err) => Some(Err(err)),
                    }
                })
            };

            match read_res {
                Ok(copied) => {
                    writer.skip(copied);
                    written_bytes += copied;
                }
                Err(err) if written_bytes == 0 => return Err(err),
                Err(_) => break,
            }
        }

        Ok(written_bytes)
    }

    fn write_at(
        &self,
        _offset: usize,
        _reader: &mut VmReader,
        _status_flags: StatusFlags,
    ) -> Result<usize> {
        // FIXME: Opening this device with `O_WRONLY` or `O_RDWR` fails on Linux. Therefore, the
        // write operation should never be reached. However, we need to return an error here
        // because `Device::open` does not accept the access mode as an argument.
        return_errno_with_message!(
            Errno::EBADF,
            "the hardware RNG device does not support writing"
        );
    }
}

impl FileIo for RngCurrent {
    fn check_seekable(&self) -> Result<()> {
        Ok(())
    }

    fn is_offset_aware(&self) -> bool {
        false
    }
}

pub(super) fn init_in_first_kthread() {
    if let Some((_, device)) = all_devices().into_iter().next() {
        *RNG_CURRENT.lock() = Some(device);
    }

    char::register(HwRngDevice::new()).unwrap();
}

fn current_device() -> Result<Arc<EntropyDevice>> {
    let Some(rng) = RNG_CURRENT.lock().clone() else {
        return_errno_with_message!(Errno::ENODEV, "no current hardware RNG device is selected");
    };
    Ok(rng)
}

fn try_read_entropy(rng: &EntropyDevice, writer: &mut VmWriter) -> Result<usize> {
    let Some(copied) = rng.try_read_into(writer)? else {
        return_errno_with_message!(Errno::EAGAIN, "no random data is available");
    };

    Ok(copied)
}
