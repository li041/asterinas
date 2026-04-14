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
    mem::size_of,
    sync::atomic::{AtomicU64, Ordering},
};

use aster_fuse::*;
use ostd::{
    arch::trap::TrapFrame,
    debug, info,
    mm::{
        HasSize, VmIo,
        dma::{FromDevice, ToDevice},
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

trait InFlightRequest: Send + Sync {
    fn mark_completed(&self);
}

pub(super) trait FuseOperation {
    type Output;

    fn build_request(&self, fs: &FileSystemDevice) -> Result<FuseRequest, VirtioDeviceError>;

    fn decode_reply(
        self,
        request: &FuseRequest,
        out_header: OutHeader,
    ) -> Result<Self::Output, VirtioDeviceError>;
}

pub(super) struct FuseRequest {
    unique: u64,
    nodeid: u64,
    input_buffers: Vec<FsInBuf>,
    output_buffers: Vec<FsOutBuf>,
    wait_state: SpinLock<RequestWaitState, LocalIrqDisabled>,
}

impl FuseRequest {
    pub(super) fn check_reply(&self) -> Result<OutHeader, VirtioDeviceError> {
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
        if out_header.unique != self.unique {
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

    pub(super) fn read_payload<T: Pod>(&self, offset: usize) -> Result<T, VirtioDeviceError> {
        let mut value = T::new_zeroed();
        self.read_payload_bytes(offset, value.as_mut_bytes())?;
        Ok(value)
    }

    pub(super) fn read_payload_bytes(
        &self,
        offset: usize,
        dst: &mut [u8],
    ) -> Result<(), VirtioDeviceError> {
        if dst.is_empty() {
            return Ok(());
        }

        let end_offset = offset
            .checked_add(dst.len())
            .ok_or(VirtioDeviceError::QueueUnknownError)?;
        let mut payload_offset = 0usize;
        let mut copied = 0usize;

        for (index, payload_buf) in self.output_buffers.iter().enumerate() {
            let buf_payload_offset = if index == 0 {
                size_of::<OutHeader>()
            } else {
                0
            };
            let buf_payload_len = payload_buf.size().saturating_sub(buf_payload_offset);
            if buf_payload_len == 0 {
                continue;
            }

            let next_payload_offset = payload_offset
                .checked_add(buf_payload_len)
                .ok_or(VirtioDeviceError::QueueUnknownError)?;
            let read_start = cmp::max(offset, payload_offset);
            let read_end = cmp::min(end_offset, next_payload_offset);

            if read_start < read_end {
                payload_buf
                    .mem_obj()
                    .sync_from_device(payload_buf.offset().clone())
                    .map_err(|_| VirtioDeviceError::QueueUnknownError)?;

                let local_offset = buf_payload_offset + read_start - payload_offset;
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

    fn new(unique: u64, nodeid: u64, in_buf: FsInBuf, out_buf: Option<FsOutBuf>) -> Self {
        Self {
            unique,
            nodeid,
            input_buffers: vec![in_buf],
            output_buffers: out_buf.into_iter().collect(),
            wait_state: SpinLock::new(RequestWaitState {
                completed: false,
                waker: None,
            }),
        }
    }

    pub(super) fn wait(&self) -> Result<(), VirtioDeviceError> {
        let mut wait_state = self.wait_state.lock();
        if wait_state.completed {
            return Ok(());
        }
        let (waiter, waker) = Waiter::new_pair();
        wait_state.waker = Some(waker);
        drop(wait_state);

        let timeout_deadline = Jiffies::elapsed()
            .as_u64()
            .saturating_add(REQUEST_WAIT_TIMEOUT_JIFFIES);

        let wait_res = waiter.wait_until_or_cancelled(
            || {
                if self.wait_state.lock().completed {
                    return Some(());
                }
                None
            },
            || {
                if Jiffies::elapsed().as_u64() >= timeout_deadline {
                    Err(())
                } else {
                    Ok(())
                }
            },
        );

        if wait_res.is_ok() {
            return Ok(());
        }

        let mut wait_state = self.wait_state.lock();
        if wait_state.completed {
            return Ok(());
        }
        wait_state.waker = None;

        Err(VirtioDeviceError::QueueUnknownError)
    }
}

impl InFlightRequest for FuseRequest {
    fn mark_completed(&self) {
        let waker = {
            let mut wait_state = self.wait_state.lock();
            wait_state.completed = true;
            wait_state.waker.take()
        };

        if let Some(waker) = waker {
            let _ = waker.wake_up();
        }
    }
}

struct FsRequestQueue {
    queue: SpinLock<VirtQueue, LocalIrqDisabled>,
    in_flight_requests: SpinLock<Vec<Option<Arc<dyn InFlightRequest>>>, LocalIrqDisabled>,
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

impl FileSystemDevice {
    pub(super) fn execute<Op: FuseOperation>(
        &self,
        operation: Op,
    ) -> Result<Op::Output, VirtioDeviceError> {
        let request = operation.build_request(self)?;
        let request = Arc::new(request);
        let request_queue_count = self.request_queues.len();
        let queue_index = if request_queue_count <= 1 {
            0
        } else {
            (request.nodeid as usize) % request_queue_count
        };
        self.submit_to_queue(&self.request_queues[queue_index], request.clone())?;
        request.wait()?;
        let out_header = request.check_reply()?;

        operation.decode_reply(&request, out_header)
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
