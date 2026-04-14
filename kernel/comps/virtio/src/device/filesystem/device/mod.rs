// SPDX-License-Identifier: MPL-2.0

use alloc::{
    boxed::Box,
    string::{String, ToString},
    sync::Arc,
    vec,
    vec::Vec,
};
use core::{
    cmp,
    hint::spin_loop,
    mem::size_of,
    sync::atomic::{AtomicU64, Ordering},
};

use aster_fuse::*;
use ostd::{
    arch::trap::TrapFrame,
    debug, info,
    mm::{
        HasSize, VmIo, VmReader,
        dma::{FromDevice, ToDevice},
        io::util::HasVmReaderWriter,
    },
    sync::{LocalIrqDisabled, SpinLock, Waiter, Waker},
    timer::{Jiffies, TIMER_FREQ},
    warn,
};
use ostd_pod::Pod;
use spin::Once;

use super::{
    DEVICE_NAME,
    config::{FileSystemFeatures, VirtioFsConfig},
    pool::{FsDmaBuf, FsDmaPool},
};
use crate::{
    device::VirtioDeviceError,
    queue::{QueueError, VirtQueue},
    transport::{VirtioTransport, VirtioTransportError},
};

const HIPRIO_QUEUE_INDEX: u16 = 0;
const DEFAULT_QUEUE_SIZE: u16 = 128;
const REQUEST_WAIT_TIMEOUT_JIFFIES: u64 = 10 * TIMER_FREQ;
const O_RDWR: u32 = 2;

static FILESYSTEM_DEVICES: Once<SpinLock<Vec<Arc<FileSystemDevice>>>> = Once::new();

type FsInBuf = FsDmaBuf<ToDevice>;
type FsOutBuf = FsDmaBuf<FromDevice>;

#[derive(Debug, Clone)]
pub struct VirtioFsDirEntry {
    pub ino: u64,
    pub offset: u64,
    pub type_: u32,
    pub name: String,
}

struct RequestWaitState {
    completed: bool,
    waker: Option<Arc<Waker>>,
}

struct FsRequest {
    _input_buffers: Vec<FsInBuf>,
    output_buffers: Vec<FsOutBuf>,
    wait_state: SpinLock<RequestWaitState, LocalIrqDisabled>,
}

impl FsRequest {
    fn new(input_buffers: Vec<FsInBuf>, output_buffers: Vec<FsOutBuf>) -> Arc<Self> {
        Arc::new(Self {
            _input_buffers: input_buffers,
            output_buffers,
            wait_state: SpinLock::new(RequestWaitState {
                completed: false,
                waker: None,
            }),
        })
    }

    fn check_reply(&self, unique: u64) -> Result<OutHeader, VirtioDeviceError> {
        let out_header_buf = self
            .output_buffers
            .first()
            .ok_or(VirtioDeviceError::QueueUnknownError)?;
        out_header_buf
            .mem_obj()
            .sync_from_device(out_header_buf.offset().clone())
            .map_err(|_| VirtioDeviceError::QueueUnknownError)?;

        let out_header: OutHeader = out_header_buf
            .read_val(0)
            .map_err(|_| VirtioDeviceError::QueueUnknownError)?;
        if out_header.unique != unique {
            warn!(
                "{} failed: unique={}, error={}, out_len={}",
                DEVICE_NAME, out_header.unique, out_header.error, out_header.len
            );
            return Err(VirtioDeviceError::QueueUnknownError);
        }
        if out_header.error != 0 {
            warn!(
                "{} failed: unique={}, error={}, out_len={}",
                DEVICE_NAME, out_header.unique, out_header.error, out_header.len
            );
            return Err(VirtioDeviceError::FileSystemError(out_header.error));
        }

        Ok(out_header)
    }

    fn read_payload<T: Pod>(&self, offset: usize) -> Result<T, VirtioDeviceError> {
        let mut value = T::new_zeroed();
        self.read_payload_bytes(offset, value.as_mut_bytes())?;
        Ok(value)
    }

    fn read_payload_bytes(&self, offset: usize, dst: &mut [u8]) -> Result<(), VirtioDeviceError> {
        if dst.is_empty() {
            return Ok(());
        }

        let end_offset = offset
            .checked_add(dst.len())
            .ok_or(VirtioDeviceError::QueueUnknownError)?;
        let mut payload_offset = 0usize;
        let mut copied = 0usize;

        for payload_buf in self.output_buffers.iter().skip(1) {
            let next_payload_offset = payload_offset
                .checked_add(payload_buf.size())
                .ok_or(VirtioDeviceError::QueueUnknownError)?;
            let read_start = cmp::max(offset, payload_offset);
            let read_end = cmp::min(end_offset, next_payload_offset);

            if read_start < read_end {
                payload_buf
                    .mem_obj()
                    .sync_from_device(payload_buf.offset().clone())
                    .map_err(|_| VirtioDeviceError::QueueUnknownError)?;

                let local_offset = read_start - payload_offset;
                let copy_len = read_end - read_start;
                payload_buf
                    .read_bytes(local_offset, &mut dst[copied..copied + copy_len])
                    .map_err(|_| VirtioDeviceError::QueueUnknownError)?;
                copied += copy_len;

                if copied == dst.len() {
                    return Ok(());
                }
            }

            payload_offset = next_payload_offset;
        }

        Err(VirtioDeviceError::QueueUnknownError)
    }
}

struct FsRequestQueue {
    queue: SpinLock<VirtQueue, LocalIrqDisabled>,
    in_flight_requests: SpinLock<Vec<Option<Arc<FsRequest>>>, LocalIrqDisabled>,
}

impl FsRequestQueue {
    fn new(queue: VirtQueue) -> Self {
        let queue_size = queue.available_desc();
        Self {
            queue: SpinLock::new(queue),
            in_flight_requests: SpinLock::new(vec![None; queue_size]),
        }
    }
}

impl core::fmt::Debug for FsRequestQueue {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("FsRequestQueue")
            .field("queue", &self.queue)
            .field(
                "in_flight_requests_len",
                &self
                    .in_flight_requests
                    .lock()
                    .iter()
                    .filter(|request| request.is_some())
                    .count(),
            )
            .finish()
    }
}

#[derive(Debug, Clone, Copy)]
enum QueueSelector {
    Hiprio,
    Request(usize),
}

pub struct FileSystemDevice {
    transport: SpinLock<Box<dyn VirtioTransport>, LocalIrqDisabled>,
    hiprio_queue: FsRequestQueue,
    request_queues: Vec<FsRequestQueue>,
    to_device_pool: Arc<FsDmaPool<ToDevice>>,
    from_device_pool: Arc<FsDmaPool<FromDevice>>,
    next_unique: AtomicU64,
    tag: String,
    notify_supported: bool,
}

impl core::fmt::Debug for FileSystemDevice {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("FileSystemDevice")
            .field("transport", &self.transport)
            .field("hiprio_queue", &self.hiprio_queue)
            .field("request_queues", &self.request_queues)
            .field("tag", &self.tag)
            .field("notify_supported", &self.notify_supported)
            .finish()
    }
}

mod client;
mod helpers;
mod virtio_ops;

pub fn get_device_by_tag(tag: &str) -> Option<Arc<FileSystemDevice>> {
    let devices = FILESYSTEM_DEVICES.get()?;
    let devices = devices.disable_irq().lock();
    devices.iter().find(|device| device.tag == tag).cloned()
}

fn config_space_change(_: &TrapFrame) {
    debug!("Virtio-FS device configuration space change");
}

fn map_transport_err(_: VirtioTransportError) -> VirtioDeviceError {
    VirtioDeviceError::QueueUnknownError
}
