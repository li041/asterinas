// SPDX-License-Identifier: MPL-2.0

//! Virtiofs device request handling.
//!
//! This module defines [`FileSystemDevice`], which initializes the virtiofs
//! queues, tracks in-flight requests, and sends typed FUSE operations to the
//! backend.

mod helpers;
pub mod session;
mod virtio_ops;

use alloc::{
    boxed::Box,
    string::{String, ToString},
    sync::Arc,
    vec,
    vec::Vec,
};
use core::{
    mem::size_of,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use aster_fuse::{
    FuseCompleteFn, FuseError, FuseNodeId, FuseOperation, FuseStatus, FuseStatusError, FuseUnique,
    OutHeader,
};
use aster_util::{mem_obj_slice::Slice, slot_vec::SlotVec};
use ostd::{
    arch::trap::TrapFrame,
    debug, info,
    mm::{
        dma::{FromDevice, ToDevice},
        io::util::HasVmReaderWriter,
    },
    sync::{LocalIrqDisabled, SpinLock, Waiter, Waker},
    timer::{self, Jiffies},
    warn,
};
pub use session::{AttrVersion, FuseSession};
use spin::Once;

use super::{
    config::{FileSystemFeatures, VirtioFsConfig},
    pool::FsDmaPool,
    DEVICE_NAME,
};
use crate::{
    device::{
        filesystem::pool::{FsDmaStorage, FuseDataBuf, FuseReadBuf, FuseWriteBuf},
        VirtioDeviceError,
    },
    queue::{PopUsedError, VirtQueue},
    transport::VirtioTransport,
};

/// Virtio-fs reserves queue 0 for high-priority requests such as `FUSE_FORGET`.
const HIPRIO_QUEUE_INDEX: u16 = 0;

/// The default queue size for any queue in virtio-fs.
const DEFAULT_QUEUE_SIZE: u16 = 128;

/// Bound FUSE waits so a stalled daemon does not block a task forever.
const REQUEST_WAIT_TIMEOUT: Duration = Duration::from_secs(10);

static FILESYSTEM_DEVICES: Once<SpinLock<Vec<Arc<FileSystemDevice>>, LocalIrqDisabled>> =
    Once::new();

/// A virtiofs device that issues FUSE requests to a backend server.
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

impl FileSystemDevice {
    fn new(
        transport: Box<dyn VirtioTransport>,
        hiprio_queue: FsRequestQueue,
        request_queues: Vec<FsRequestQueue>,
        tag: String,
        notify_supported: bool,
    ) -> Self {
        Self {
            transport: SpinLock::new(transport),
            hiprio_queue,
            request_queues,
            to_device_pool: FsDmaPool::new(),
            from_device_pool: FsDmaPool::new(),
            // Start request IDs at 1 and keep 0 unused. In FUSE,
            // `unique == 0` is reserved for unsolicited notification messages
            // rather than ordinary request/reply matching.
            next_unique: AtomicU64::new(1),
            tag,
            notify_supported,
        }
    }

    /// Submit a FUSE operation to virtio-fs request queues
    /// and return a waiter for the request completion.
    pub(crate) fn submit_fuse_op<Op: FuseOperation>(
        &self,
        nodeid: FuseNodeId,
        operation: &mut Op,
        data_buf: Option<FuseDataBuf>,
        complete_fn: Option<FuseCompleteFn>,
    ) -> Result<Arc<FuseWaiter>, FuseError> {
        let request = Arc::new(self.prepare_request(nodeid, operation, data_buf, complete_fn)?);
        let waiter = request.waiter.clone();

        let queue = self.select_request_queue(request.nodeid);
        self.submit(queue, request.clone());

        Ok(waiter)
    }

    fn prepare_request<Op: FuseOperation>(
        &self,
        nodeid: FuseNodeId,
        operation: &mut Op,
        data_buf: Option<FuseDataBuf>,
        complete_fn: Option<FuseCompleteFn>,
    ) -> Result<FuseRequest, FuseError> {
        let unique = self.alloc_unique();
        let in_buf = self.prepare_in_buf(nodeid, operation, unique)?;

        let mut in_bufs = vec![in_buf];

        let out_bufs = match data_buf {
            Some(FuseDataBuf::Read(data_buf)) => {
                Some(vec![self.prepare_out_header_buf()?, data_buf])
            }
            Some(FuseDataBuf::Write(data_buf)) => {
                data_buf
                    .mem_obj()
                    .sync_to_device(data_buf.offset().clone())
                    .unwrap();
                in_bufs.push(data_buf);

                let payload_size = operation.out_payload_size().unwrap();
                let out_buf = self.prepare_out_buf(payload_size)?;

                Some(vec![out_buf])
            }
            None => operation
                .out_payload_size()
                .map(|payload_size| self.prepare_out_buf(payload_size))
                .transpose()?
                .map(|out_buf| vec![out_buf]),
        };

        Ok(FuseRequest::new(
            unique,
            nodeid,
            in_bufs,
            out_bufs,
            complete_fn,
        ))
    }

    fn select_request_queue(&self, nodeid: FuseNodeId) -> &FsRequestQueue {
        let request_queue_count = self.request_queues.len();
        let queue_index = if request_queue_count <= 1 {
            0
        } else {
            (nodeid.as_u64() as usize) % request_queue_count
        };

        &self.request_queues[queue_index]
    }
}

pub(super) struct FuseRequest {
    nodeid: FuseNodeId,
    in_bufs: Vec<Arc<Slice<FsDmaStorage<ToDevice>>>>,
    waiter: Arc<FuseWaiter>,
}

impl FuseRequest {
    fn new(
        unique: FuseUnique,
        nodeid: FuseNodeId,
        in_bufs: Vec<Arc<Slice<FsDmaStorage<ToDevice>>>>,
        out_bufs: Option<Vec<Arc<Slice<FsDmaStorage<FromDevice>>>>>,
        complete_fn: Option<FuseCompleteFn>,
    ) -> Self {
        Self {
            nodeid,
            in_bufs,
            waiter: Arc::new(FuseWaiter::new(unique, out_bufs, complete_fn)),
        }
    }

    fn wake_completed(&self) {
        self.waiter.complete(FuseStatus::Complete);
    }

    fn wake_if_expired(&self, now: Duration) {
        let status = self.waiter.wake_if_expired(now);
        if status == FuseStatus::Error(FuseStatusError::Timeout) {
            warn!("virtiofs request timed out");
        }
    }
}

/// A handle for one submitted FUSE request.
#[must_use]
pub struct FuseWaiter {
    unique: FuseUnique,
    out_bufs: Option<Vec<Arc<Slice<FsDmaStorage<FromDevice>>>>>,
    inner: SpinLock<FuseWaiterInner, LocalIrqDisabled>,
}

impl FuseWaiter {
    fn new(
        unique: FuseUnique,
        out_bufs: Option<Vec<Arc<Slice<FsDmaStorage<FromDevice>>>>>,
        complete_fn: Option<FuseCompleteFn>,
    ) -> Self {
        Self {
            unique,
            out_bufs,
            inner: SpinLock::new(FuseWaiterInner {
                status: FuseStatus::Pending,
                timeout_deadline: None,
                waker: None,
                complete_fn,
            }),
        }
    }

    pub fn wait(&self) -> FuseStatus {
        let mut inner = self.inner.lock();
        if !inner.status.is_pending() {
            return inner.status;
        }

        let (waiter, waker) = Waiter::new_pair();
        let timeout_deadline = Jiffies::elapsed()
            .as_duration()
            .saturating_add(REQUEST_WAIT_TIMEOUT);
        inner.timeout_deadline = Some(timeout_deadline);
        inner.waker = Some(waker);
        drop(inner);

        let wait_res = waiter.wait_until_or_cancelled(
            || {
                let status = self.inner.lock().status;
                (!status.is_pending()).then_some(status)
            },
            || {
                if Jiffies::elapsed().as_duration() >= timeout_deadline {
                    Err(())
                } else {
                    Ok(())
                }
            },
        );

        if let Ok(status) = wait_res {
            return status;
        }

        let mut inner = self.inner.lock();
        if !inner.status.is_pending() {
            return inner.status;
        }
        inner.waker = None;
        inner.timeout_deadline = None;
        drop(inner);

        self.complete(FuseStatus::Error(FuseStatusError::Timeout))
    }

    pub(super) fn out_bufs(&self) -> Option<&[Arc<Slice<FsDmaStorage<FromDevice>>>]> {
        self.out_bufs.as_deref()
    }

    pub(super) fn check_device_output(&self) -> Result<usize, FuseError> {
        let result = self.check_device_output_result();
        let status = match result {
            Ok(_) => FuseStatus::Complete,
            Err(FuseError::RemoteError(_)) => FuseStatus::Error(FuseStatusError::RemoteError),
            Err(_) => FuseStatus::Error(FuseStatusError::MalformedResponse),
        };
        self.call_complete_fn(status);

        result
    }

    fn wake_if_expired(&self, now: Duration) -> FuseStatus {
        let is_expired = {
            let inner = self.inner.lock();
            inner.status.is_pending()
                && inner
                    .timeout_deadline
                    .is_some_and(|deadline| now >= deadline)
        };
        if !is_expired {
            return self.inner.lock().status;
        }

        self.complete(FuseStatus::Error(FuseStatusError::Timeout))
    }

    fn complete(&self, status: FuseStatus) -> FuseStatus {
        let waker = {
            let mut inner = self.inner.lock();
            if !inner.status.is_pending() {
                return inner.status;
            }

            inner.status = status;
            inner.timeout_deadline = None;

            inner.waker.take()
        };

        if let Some(waker) = waker {
            let _ = waker.wake_up();
        }

        status
    }

    fn check_device_output_result(&self) -> Result<usize, FuseError> {
        let out_header_buf = self.out_header_buf()?;
        if let Some(out_bufs) = self.out_bufs.as_ref() {
            for out_buf in out_bufs {
                out_buf
                    .mem_obj()
                    .sync_from_device(out_buf.offset().clone())
                    .unwrap();
            }
        }

        let mut reader = out_header_buf.reader().unwrap();
        let out_header = reader
            .read_val::<OutHeader>()
            .map_err(|_| FuseError::MalformedResponse)?;

        let payload_len = (out_header.len() as usize)
            .checked_sub(size_of::<OutHeader>())
            .ok_or(FuseError::MalformedResponse)?;
        if out_header.unique() != self.unique {
            return Err(FuseError::MalformedResponse);
        }
        if out_header.error() != 0 {
            return Err(FuseError::RemoteError(out_header.error()));
        }

        Ok(payload_len)
    }

    fn out_header_buf(&self) -> Result<&Arc<Slice<FsDmaStorage<FromDevice>>>, FuseError> {
        let Some(out_bufs) = self.out_bufs.as_ref() else {
            return Err(FuseError::MalformedResponse);
        };
        let Some(out_buf) = out_bufs.first() else {
            return Err(FuseError::MalformedResponse);
        };
        Ok(out_buf)
    }

    fn call_complete_fn(&self, status: FuseStatus) {
        let complete_fn = self.inner.lock().complete_fn.take();
        if let Some(complete_fn) = complete_fn {
            complete_fn(status);
        }
    }
}

struct FsRequestQueue {
    queue: SpinLock<VirtQueue, LocalIrqDisabled>,
    in_flight_requests: SpinLock<SlotVec<Arc<FuseRequest>>, LocalIrqDisabled>,
}

impl FsRequestQueue {
    fn new(queue: VirtQueue) -> Self {
        Self {
            queue: SpinLock::new(queue),
            in_flight_requests: SpinLock::new(SlotVec::new()),
        }
    }
}

impl core::fmt::Debug for FsRequestQueue {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let in_flight_requests_len = self.in_flight_requests.lock().len();

        f.debug_struct("FsRequestQueue")
            .field("queue", &self.queue)
            .field("in_flight_requests_len", &in_flight_requests_len)
            .finish()
    }
}

struct FuseWaiterInner {
    status: FuseStatus,
    timeout_deadline: Option<Duration>,
    waker: Option<Arc<Waker>>,
    complete_fn: Option<FuseCompleteFn>,
}

pub(super) fn filesystem_devices() -> &'static SpinLock<Vec<Arc<FileSystemDevice>>, LocalIrqDisabled>
{
    FILESYSTEM_DEVICES.call_once(|| {
        let devices = SpinLock::new(Vec::new());
        timer::register_callback_on_cpu(wake_expired_filesystem_requests);
        devices
    })
}

fn wake_expired_filesystem_requests() {
    let Some(devices) = FILESYSTEM_DEVICES.get() else {
        return;
    };

    let now = Jiffies::elapsed().as_duration();
    let devices = devices.lock();
    for device in devices.iter() {
        for queue in core::iter::once(&device.hiprio_queue).chain(device.request_queues.iter()) {
            let requests = queue
                .in_flight_requests
                .lock()
                .iter()
                .cloned()
                .collect::<Vec<_>>();

            requests
                .iter()
                .for_each(|request| request.wake_if_expired(now));
        }
    }
}

/// Finds the virtio-fs device registered with the given `tag`.
pub fn find_device_by_tag(tag: &str) -> Option<Arc<FileSystemDevice>> {
    let devices = FILESYSTEM_DEVICES.get()?;
    let devices = devices.lock();
    devices.iter().find(|device| device.tag == tag).cloned()
}
