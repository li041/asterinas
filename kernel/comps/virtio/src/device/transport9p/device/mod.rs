// SPDX-License-Identifier: MPL-2.0

use alloc::{
    boxed::Box,
    collections::BTreeMap,
    string::{String, ToString},
    sync::Arc,
    vec::Vec,
};
use core::{
    cmp,
    hint::spin_loop,
    sync::atomic::{AtomicU32, Ordering},
};

use aster_util::mem_obj_slice::Slice;
use log::{debug, info, warn};
use ostd::{
    arch::trap::TrapFrame,
    mm::{HasSize, VmReader, VmWriter, io_util::HasVmReaderWriter},
    sync::{LocalIrqDisabled, SpinLock, Waiter, Waker},
    timer::{Jiffies, TIMER_FREQ},
};
use spin::Once;

use super::{
    DEVICE_NAME,
    config::{Transport9PFeatures, Virtio9PConfig},
    pool::{P9DmaBuf, P9DmaPools},
    protocol::*,
};
use crate::{
    device::VirtioDeviceError,
    id_alloc::SyncIdAlloc,
    queue::{QueueError, VirtQueue},
    transport::{VirtioTransport, VirtioTransportError},
};

const REQUEST_QUEUE_INDEX: u16 = 0;
const DEFAULT_QUEUE_SIZE: u16 = 128;
const TAG_ALLOC_CAPACITY: usize = 4096;
const REQUEST_WAIT_TIMEOUT_JIFFIES: u64 = 10 * TIMER_FREQ;

static TRANSPORT9P_DEVICES: Once<SpinLock<Vec<Arc<Transport9PDevice>>>> = Once::new();

struct RequestWaitState {
    completed: bool,
    waker: Option<Arc<Waker>>,
}

struct P9RequestQueue {
    queue: SpinLock<VirtQueue, LocalIrqDisabled>,
    pending_requests: SpinLock<BTreeMap<u16, usize>>,
    request_states: SpinLock<BTreeMap<usize, RequestWaitState>>,
}

impl P9RequestQueue {
    fn new(queue: VirtQueue) -> Self {
        Self {
            queue: SpinLock::new(queue),
            pending_requests: SpinLock::new(BTreeMap::new()),
            request_states: SpinLock::new(BTreeMap::new()),
        }
    }
}

impl core::fmt::Debug for P9RequestQueue {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("P9RequestQueue")
            .field("queue", &self.queue)
            .field(
                "pending_requests_len",
                &self.pending_requests.disable_irq().lock().len(),
            )
            .finish()
    }
}

pub struct Transport9PDevice {
    transport: SpinLock<Box<dyn VirtioTransport>, LocalIrqDisabled>,
    request_queue: P9RequestQueue,
    dma_pools: Arc<P9DmaPools>,
    tag_alloc: SyncIdAlloc,
    tag: String,
    msize: AtomicU32,
}

impl core::fmt::Debug for Transport9PDevice {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Transport9PDevice")
            .field("transport", &self.transport)
            .field("tag", &self.tag)
            .field("msize", &self.msize.load(Ordering::Relaxed))
            .finish()
    }
}

mod client;
mod helpers;
mod virtio_ops;

pub fn get_device_by_tag(tag: &str) -> Option<Arc<Transport9PDevice>> {
    let devices = TRANSPORT9P_DEVICES.get()?;
    let devices = devices.disable_irq().lock();
    devices.iter().find(|device| device.tag == tag).cloned()
}

fn config_space_change(_: &TrapFrame) {
    debug!("Virtio-9P device configuration space change");
}

fn map_transport_err(_: VirtioTransportError) -> VirtioDeviceError {
    VirtioDeviceError::QueueUnknownError
}

impl Transport9PDevice {
    pub fn tag(&self) -> &str {
        &self.tag
    }

    pub fn msize(&self) -> u32 {
        self.msize.load(Ordering::Relaxed)
    }

    pub(super) fn submit_request(
        &self,
        unique: u64,
        in_slices: &[&Slice<P9DmaBuf>],
        out_slices: &[&Slice<P9DmaBuf>],
    ) -> Result<(), VirtioDeviceError> {
        let queue = &self.request_queue;

        {
            let mut virt_queue = queue.queue.lock();

            let token = virt_queue.add_dma_buf(in_slices, out_slices)?;

            queue
                .pending_requests
                .disable_irq()
                .lock()
                .insert(token, unique as usize);

            if virt_queue.should_notify() {
                virt_queue.notify();
            }
        }
        Ok(())
    }

    pub(super) fn submit_request_and_wait(
        &self,
        unique: u64,
        in_slices: &[&Slice<P9DmaBuf>],
        out_slices: &[&Slice<P9DmaBuf>],
    ) -> Result<(), VirtioDeviceError> {
        self.submit_request(unique, in_slices, out_slices)?;
        self.wait_for_unique(unique as usize)
    }

    pub(super) fn handle_queue_irq(&self) {
        let queue_state = &self.request_queue;
        loop {
            let pop_result = {
                let mut queue = queue_state.queue.lock();
                queue.pop_used()
            };

            let (token, _) = match pop_result {
                Ok(completed) => completed,
                Err(QueueError::NotReady) => break,
                Err(_) => break,
            };

            let pending = queue_state
                .pending_requests
                .disable_irq()
                .lock()
                .remove(&token);

            let Some(pending) = pending else {
                continue;
            };

            let mut request_states = queue_state.request_states.disable_irq().lock();
            let request_state = request_states.entry(pending).or_insert(RequestWaitState {
                completed: false,
                waker: None,
            });
            request_state.completed = true;

            if let Some(waker) = request_state.waker.take() {
                let _ = waker.wake_up();
            }
        }
    }

    pub(super) fn wait_for_unique(
        &self,
        unique: usize,
    ) -> Result<(), VirtioDeviceError> {
        let queue_state = &self.request_queue;

        {
            let mut request_states = queue_state.request_states.disable_irq().lock();
            if let Some(state) = request_states.get(&unique)
                && state.completed
            {
                request_states.remove(&unique);
                self.tag_alloc.dealloc(unique);
                return Ok(());
            }
        }

        let (waiter, waker) = Waiter::new_pair();
        {
            let mut request_states = queue_state.request_states.disable_irq().lock();
            let state = request_states.entry(unique).or_insert(RequestWaitState {
                completed: false,
                waker: None,
            });
            if state.completed {
                request_states.remove(&unique);
                self.tag_alloc.dealloc(unique);
                return Ok(());
            }
            state.waker = Some(waker);
        }

        let timeout_deadline = Jiffies::elapsed()
            .as_u64()
            .saturating_add(REQUEST_WAIT_TIMEOUT_JIFFIES);

        let wait_res = waiter.wait_until_or_cancelled(
            || {
                let mut request_states = queue_state.request_states.disable_irq().lock();
                if let Some(state) = request_states.get(&unique)
                    && state.completed
                {
                    request_states.remove(&unique);
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
            self.tag_alloc.dealloc(unique);
            return Ok(());
        }

        let mut request_states = queue_state.request_states.disable_irq().lock();
        if let Some(state) = request_states.get_mut(&unique) {
            state.waker = None;
        }
        request_states.remove(&unique);
        self.tag_alloc.dealloc(unique);

        Err(VirtioDeviceError::QueueUnknownError)
    }

    /// Wait for a reply from the device by spinning;
    /// intended for early boot or non-task contexts.
    pub(super) fn wait_for_unique_early(
        &self,
        unique: usize,
    ) -> Result<(), VirtioDeviceError> {
        let queue_state = &self.request_queue;

        loop {
            self.handle_queue_irq();

            let mut request_states = queue_state.request_states.disable_irq().lock();
            if let Some(state) = request_states.get(&unique)
                && state.completed
            {
                request_states.remove(&unique);
                self.tag_alloc.dealloc(unique);
                return Ok(());
            }

            spin_loop();
        }
    }
}
