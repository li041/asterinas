// SPDX-License-Identifier: MPL-2.0

//! Virtiofs device request handling.
//!
//! This module defines [`FileSystemDevice`], which initializes the virtiofs
//! queues, tracks in-flight requests, and sends typed FUSE operations to the
//! backend.

pub mod connection;
mod helpers;
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
};

use aster_fuse::*;
pub use connection::{AttrVersion, FuseSession};
use ostd::{
    arch::trap::TrapFrame,
    debug, info,
    mm::{
        dma::{FromDevice, ToDevice},
        io::util::HasVmReaderWriter,
    },
    sync::{LocalIrqDisabled, SpinLock, Waiter, Waker},
    timer::{Jiffies, TIMER_FREQ},
    warn,
};
use spin::Once;

use super::{
    DEVICE_NAME,
    config::{FileSystemFeatures, VirtioFsConfig},
    pool::{FsDmaBuf, FsDmaPool},
};
use crate::{
    device::VirtioDeviceError,
    queue::{PopUsedError, VirtQueue},
    transport::VirtioTransport,
};

/// Virtio-fs reserves queue 0 for high-priority requests such as `FUSE_FORGET`.
const HIPRIO_QUEUE_INDEX: u16 = 0;

/// The default queue size for any queue in virtio-fs.
const DEFAULT_QUEUE_SIZE: u16 = 128;

/// Bound FUSE waits so a stalled daemon does not block a task forever.
const REQUEST_WAIT_TIMEOUT_JIFFIES: u64 = 10 * TIMER_FREQ;

static FILESYSTEM_DEVICES: Once<SpinLock<Vec<Arc<FileSystemDevice>>>> = Once::new();

type FsInBuf = FsDmaBuf<ToDevice>;
type FsOutBuf = FsDmaBuf<FromDevice>;

/// A virtiofs device that issues FUSE requests to a backend server.
pub struct FileSystemDevice {
    transport: SpinLock<Box<dyn VirtioTransport>, LocalIrqDisabled>,
    hiprio_queue: FsRequestQueue,
    request_queues: Vec<FsRequestQueue>,
    to_device_pool: Arc<FsDmaPool<ToDevice>>,
    from_device_pool: Arc<FsDmaPool<FromDevice>>,
    /// Start request IDs at 1 and keep 0 unused. In FUSE,
    /// `unique == 0` is reserved for unsolicited notification messages
    /// rather than ordinary request/reply matching.
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
            next_unique: AtomicU64::new(1),
            tag,
            notify_supported,
        }
    }

    pub(crate) fn do_fuse_op<Op: FuseOperation>(
        &self,
        nodeid: FuseNodeId,
        mut operation: Op,
    ) -> Result<Op::Output, FuseError> {
        let request = Arc::new(self.prepare_request(nodeid, &mut operation)?);
        let queue = self.select_request_queue(request.nodeid);
        self.submit(queue, request.clone());
        // FIXME: `do_fuse_op` currently blocks in the device layer after submit.
        // This can stall the current thread in an unexpected context.
        request.wait()?;
        request.read_reply(operation)
    }

    /// Sends a `FUSE_FORGET` request on the high-priority queue.
    ///
    /// `FUSE_FORGET` is a no-reply request. The backend must not send a
    /// response, so this method only submits the request and never waits for
    /// completion. Local prepare or enqueue failures are logged and otherwise
    /// ignored because callers cannot observe a protocol-level error for
    /// `FUSE_FORGET`.
    pub(crate) fn forget(&self, nodeid: FuseNodeId, nlookup: u64) {
        if nodeid == FUSE_ROOT_ID || nlookup == 0 {
            return;
        }
        let mut operation = ForgetOperation::new(ForgetIn::new(nlookup));
        let request = match self.prepare_request(nodeid, &mut operation) {
            Ok(request) => request,
            Err(err) => {
                warn!(
                    "virtiofs forget failed to prepare inode {:?} with nlookup {}: {:?}",
                    nodeid, nlookup, err
                );
                return;
            }
        };
        self.submit(&self.hiprio_queue, Arc::new(request));
    }

    pub(super) fn prepare_request<Op: FuseOperation>(
        &self,
        nodeid: FuseNodeId,
        operation: &mut Op,
    ) -> Result<FuseRequest, FuseError> {
        let unique = self.alloc_unique();
        let in_buf = self.prepare_in_buf(nodeid, operation, unique)?;
        let out_buf = operation
            .out_payload_size()
            .map(|payload_size| self.prepare_out_buf(payload_size))
            .transpose()?;

        Ok(FuseRequest::new(unique, nodeid, in_buf, out_buf))
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
    unique: u64,
    nodeid: FuseNodeId,
    in_buf: FsInBuf,
    out_buf: Option<FsOutBuf>,
    wait_state: SpinLock<RequestWaitState, LocalIrqDisabled>,
}

impl FuseRequest {
    fn new(unique: u64, nodeid: FuseNodeId, in_buf: FsInBuf, out_buf: Option<FsOutBuf>) -> Self {
        Self {
            unique,
            nodeid,
            in_buf,
            out_buf,
            wait_state: SpinLock::new(RequestWaitState {
                completed: false,
                waker: None,
            }),
        }
    }

    /// Blocks until the backend completes the request or the timeout elapses.
    pub(super) fn wait(&self) -> Result<(), FuseError> {
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
            || self.wait_state.lock().completed.then_some(()),
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

        Err(FuseError::Timeout)
    }
}

impl FuseRequest {
    /// Reads and decodes the server reply from the out-buffer.
    pub(super) fn read_reply<Op: FuseOperation>(
        &self,
        operation: Op,
    ) -> Result<Op::Output, FuseError> {
        let out_buf = self
            .out_buf
            .as_ref()
            .expect("read_reply called on a no-reply FUSE request");

        out_buf
            .mem_obj()
            .sync_from_device(out_buf.offset().clone())
            .expect("FsDmaBuf offset must lie within its backing storage");

        let mut reader = out_buf.reader().unwrap();
        let out_header: OutHeader = reader.read_val().unwrap();

        let out_len = out_header.len as usize;
        let payload_len = out_len - size_of::<OutHeader>();
        if out_header.unique != self.unique {
            return Err(FuseError::MalformedResponse);
        }
        if out_header.error != 0 {
            return Err(FuseError::RemoteError(out_header.error));
        }

        operation.parse_reply(payload_len, &mut reader)
    }

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
    in_flight_requests: SpinLock<Vec<Option<Arc<FuseRequest>>>, LocalIrqDisabled>,
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
        let in_flight_requests_len = self
            .in_flight_requests
            .lock()
            .iter()
            .filter(|request| request.is_some())
            .count();

        f.debug_struct("FsRequestQueue")
            .field("queue", &self.queue)
            .field("in_flight_requests_len", &in_flight_requests_len)
            .finish()
    }
}

struct RequestWaitState {
    completed: bool,
    waker: Option<Arc<Waker>>,
}

/// Finds the virtio-fs device registered with the given `tag`.
pub fn find_device_by_tag(tag: &str) -> Option<Arc<FileSystemDevice>> {
    let devices = FILESYSTEM_DEVICES.get()?;
    let devices = devices.disable_irq().lock();
    devices.iter().find(|device| device.tag == tag).cloned()
}
