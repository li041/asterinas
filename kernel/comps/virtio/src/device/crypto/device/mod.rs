// SPDX-License-Identifier: MPL-2.0

use alloc::{boxed::Box, collections::BTreeMap, sync::Arc, vec, vec::Vec};
use core::{cmp, hint::spin_loop, mem::size_of};

use aster_util::mem_obj_slice::Slice;
use log::{debug, info};
use ostd::{
    arch::trap::TrapFrame,
    mm::{HasSize, VmIo, VmReader, VmWriter, io_util::HasVmReaderWriter},
    sync::{LocalIrqDisabled, SpinLock, Waiter, Waker},
    timer::{Jiffies, TIMER_FREQ},
};
use spin::Once;

use super::{
    DEVICE_NAME,
    config::{CryptoFeatures, VirtioCryptoConfig},
    pool::{CryptoDmaBuf, CryptoDmaPools},
    protocol::*,
};
use crate::{
    device::VirtioDeviceError,
    id_alloc::SyncIdAlloc,
    queue::{QueueError, VirtQueue},
    transport::{VirtioTransport, VirtioTransportError},
};

const DATAQ_INDEX: u16 = 0;
const CONTROLQ_INDEX: u16 = 1;
const DEFAULT_QUEUE_SIZE: u16 = 128;
const REQUEST_ID_ALLOC_CAPACITY: usize = 4096;
const SESSION_ID_ALLOC_CAPACITY: usize = 4096;
const REQUEST_WAIT_TIMEOUT_JIFFIES: u64 = 10 * TIMER_FREQ;

static CRYPTO_DEVICES: Once<SpinLock<Vec<Arc<CryptoDevice>>>> = Once::new();

struct RequestWaitState {
    completed: bool,
    waker: Option<Arc<Waker>>,
}

struct CryptoQueue {
    queue: SpinLock<VirtQueue, LocalIrqDisabled>,
    pending_requests: SpinLock<BTreeMap<u16, usize>>,
    request_states: SpinLock<BTreeMap<usize, RequestWaitState>>,
}

impl CryptoQueue {
    fn new(queue: VirtQueue) -> Self {
        Self {
            queue: SpinLock::new(queue),
            pending_requests: SpinLock::new(BTreeMap::new()),
            request_states: SpinLock::new(BTreeMap::new()),
        }
    }
}

impl core::fmt::Debug for CryptoQueue {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CryptoQueue")
            .field("queue", &self.queue)
            .field(
                "pending_requests_len",
                &self.pending_requests.disable_irq().lock().len(),
            )
            .field(
                "request_states_len",
                &self.request_states.disable_irq().lock().len(),
            )
            .finish()
    }
}

#[derive(Debug, Clone, Copy)]
enum QueueSelector {
    Control,
    Data,
}

pub struct CryptoDevice {
    transport: SpinLock<Box<dyn VirtioTransport>, LocalIrqDisabled>,
    config: VirtioCryptoConfig,
    control_queue: CryptoQueue,
    data_queue: CryptoQueue,
    dma_pools: Arc<CryptoDmaPools>,
    request_id_alloc: SyncIdAlloc,
    session_id_alloc: SyncIdAlloc,
}

impl core::fmt::Debug for CryptoDevice {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CryptoDevice")
            .field("transport", &self.transport)
            .field("config", &self.config)
            .field("control_queue", &self.control_queue)
            .field("data_queue", &self.data_queue)
            .finish()
    }
}

mod client;
mod virtio_ops;

pub fn get_default_device() -> Option<Arc<CryptoDevice>> {
    let devices = CRYPTO_DEVICES.get()?;
    devices.disable_irq().lock().first().cloned()
}

fn config_space_change(_: &TrapFrame) {
    debug!("Virtio-Crypto device configuration space change");
}

fn map_transport_err(_: VirtioTransportError) -> VirtioDeviceError {
    VirtioDeviceError::QueueUnknownError
}

impl CryptoDevice {
    pub fn supports_cipher_service(&self) -> bool {
        self.config.crypto_services & VIRTIO_CRYPTO_SERVICE_CIPHER_MASK != 0
    }

    pub fn max_dataqueues(&self) -> u32 {
        self.config.max_dataqueues
    }

    fn queue(&self, selector: QueueSelector) -> &CryptoQueue {
        match selector {
            QueueSelector::Control => &self.control_queue,
            QueueSelector::Data => &self.data_queue,
        }
    }

    fn alloc_to_device_buf(&self, size: usize) -> CryptoDmaBuf {
        self.dma_pools.alloc_to_device(size)
    }

    fn alloc_from_device_buf(&self, size: usize) -> CryptoDmaBuf {
        self.dma_pools.alloc_from_device(size)
    }

    fn alloc_request_id(&self) -> u64 {
        self.request_id_alloc.alloc() as u64
    }

    fn alloc_session_id_hint(&self) -> u64 {
        self.session_id_alloc.alloc() as u64
    }

    fn dealloc_session_id_hint(&self, session_id: u64) {
        let index = session_id as usize;
        if index < SESSION_ID_ALLOC_CAPACITY {
            self.session_id_alloc.dealloc(index);
        }
    }

    fn submit_request(
        &self,
        selector: QueueSelector,
        request_id: u64,
        in_slices: &[&Slice<CryptoDmaBuf>],
        out_slices: &[&Slice<CryptoDmaBuf>],
    ) -> Result<(), VirtioDeviceError> {
        let queue = self.queue(selector);

        {
            let mut virt_queue = queue.queue.lock();
            let token = virt_queue.add_dma_buf(in_slices, out_slices)?;
            queue
                .pending_requests
                .disable_irq()
                .lock()
                .insert(token, request_id as usize);

            if virt_queue.should_notify() {
                virt_queue.notify();
            }
        }

        Ok(())
    }

    fn submit_request_and_wait(
        &self,
        selector: QueueSelector,
        request_id: u64,
        in_slices: &[&Slice<CryptoDmaBuf>],
        out_slices: &[&Slice<CryptoDmaBuf>],
    ) -> Result<(), VirtioDeviceError> {
        self.submit_request(selector, request_id, in_slices, out_slices)?;
        self.wait_for_request(selector, request_id as usize)
    }

    fn handle_queue_irq(&self, selector: QueueSelector) {
        let queue_state = self.queue(selector);
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

    fn wait_for_request(
        &self,
        selector: QueueSelector,
        request_id: usize,
    ) -> Result<(), VirtioDeviceError> {
        let queue_state = self.queue(selector);

        {
            let mut request_states = queue_state.request_states.disable_irq().lock();
            if let Some(state) = request_states.get(&request_id)
                && state.completed
            {
                request_states.remove(&request_id);
                self.request_id_alloc.dealloc(request_id);
                return Ok(());
            }
        }

        let (waiter, waker) = Waiter::new_pair();
        {
            let mut request_states = queue_state.request_states.disable_irq().lock();
            let state = request_states
                .entry(request_id)
                .or_insert(RequestWaitState {
                    completed: false,
                    waker: None,
                });
            if state.completed {
                request_states.remove(&request_id);
                self.request_id_alloc.dealloc(request_id);
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
                if let Some(state) = request_states.get(&request_id)
                    && state.completed
                {
                    request_states.remove(&request_id);
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
            self.request_id_alloc.dealloc(request_id);
            return Ok(());
        }

        let mut request_states = queue_state.request_states.disable_irq().lock();
        if let Some(state) = request_states.get_mut(&request_id) {
            state.waker = None;
        }
        request_states.remove(&request_id);
        self.request_id_alloc.dealloc(request_id);

        Err(VirtioDeviceError::QueueUnknownError)
    }

    fn wait_for_request_early(
        &self,
        selector: QueueSelector,
        request_id: usize,
    ) -> Result<(), VirtioDeviceError> {
        let queue_state = self.queue(selector);

        loop {
            self.handle_queue_irq(selector);

            let mut request_states = queue_state.request_states.disable_irq().lock();
            if let Some(state) = request_states.get(&request_id)
                && state.completed
            {
                request_states.remove(&request_id);
                self.request_id_alloc.dealloc(request_id);
                return Ok(());
            }

            spin_loop();
        }
    }

    fn write_bytes_to_buf(&self, buf: &CryptoDmaBuf, data: &[u8]) {
        if data.is_empty() {
            return;
        }

        {
            let mut writer = buf.writer().unwrap();
            let mut reader = VmReader::from(data);
            let _ = writer.write(&mut reader);
        }
    }

    fn read_bytes_from_slice(&self, slice: &Slice<CryptoDmaBuf>) -> Vec<u8> {
        let mut data = vec![0u8; slice.size()];
        {
            let mut reader = slice.reader().unwrap();
            reader.read(&mut VmWriter::from(data.as_mut_slice()));
        }
        data
    }

    fn check_ctrl_status(status: &VirtioCryptoSessionInput) -> Result<(), VirtioDeviceError> {
        if status.status == VIRTIO_CRYPTO_OK {
            return Ok(());
        }

        let _ = VirtioCryptoStatus::try_from(status.status);
        Err(VirtioDeviceError::QueueUnknownError)
    }

    fn check_inhdr_status(status: u8) -> Result<(), VirtioDeviceError> {
        if status as u32 == VIRTIO_CRYPTO_OK {
            return Ok(());
        }

        let _ = VirtioCryptoStatus::try_from(status as u32);
        Err(VirtioDeviceError::QueueUnknownError)
    }

    fn check_data_status(status: &VirtioCryptoDataStatus) -> Result<(), VirtioDeviceError> {
        Self::check_inhdr_status(status.status)
    }

    fn new_queue(
        index: u16,
        transport: &mut dyn VirtioTransport,
    ) -> Result<VirtQueue, VirtioDeviceError> {
        let max_queue_size = transport.max_queue_size(index).map_err(map_transport_err)?;
        let queue_size = cmp::min(DEFAULT_QUEUE_SIZE, max_queue_size);
        if queue_size == 0 {
            return Err(VirtioDeviceError::QueueUnknownError);
        }
        VirtQueue::new(index, queue_size, transport).map_err(Into::into)
    }

    fn build_ctrl_header(opcode: u32, algo: u32) -> VirtioCryptoCtrlHeader {
        VirtioCryptoCtrlHeader {
            opcode,
            algo,
            flag: 0,
            queue_id: 0,
        }
    }

    fn build_data_header(opcode: u32, algo: u32, session_id: u64) -> VirtioCryptoOpHeader {
        VirtioCryptoOpHeader {
            opcode,
            algo,
            session_id,
            flag: 0,
            padding: 0,
        }
    }

    fn crypto_op(
        &self,
        opcode: u32,
        algo: u32,
        session_id: u64,
        iv: &[u8],
        input: &[u8],
    ) -> Result<Vec<u8>, VirtioDeviceError> {
        let request_id = self.alloc_request_id();

        let req = VirtioCryptoCipherDataReq {
            header: Self::build_data_header(opcode, algo, session_id),
            iv_len: iv.len() as u32,
            src_len: input.len() as u32,
            dst_len: input.len() as u32,
            para_padding: 0,
            cipher_padding: [0u8; 24],
            op_type: VIRTIO_CRYPTO_SYM_OP_CIPHER,
            req_padding: 0,
        };

        let req_buf = self.alloc_to_device_buf(size_of::<VirtioCryptoCipherDataReq>());
        let req_slice = Slice::new(req_buf.clone(), 0..size_of::<VirtioCryptoCipherDataReq>());
        req_slice.write_val(0, &req).unwrap();
        req_slice
            .mem_obj()
            .sync_to_device(req_slice.offset().clone())
            .unwrap();

        let iv_buf = self.alloc_to_device_buf(iv.len());
        let iv_slice = Slice::new(iv_buf.clone(), 0..iv.len());
        self.write_bytes_to_buf(&iv_buf, iv);
        iv_slice
            .mem_obj()
            .sync_to_device(iv_slice.offset().clone())
            .unwrap();

        let input_buf = self.alloc_to_device_buf(input.len());
        let input_slice = Slice::new(input_buf.clone(), 0..input.len());
        self.write_bytes_to_buf(&input_buf, input);
        input_slice
            .mem_obj()
            .sync_to_device(input_slice.offset().clone())
            .unwrap();

        let output_buf = self.alloc_from_device_buf(input.len());
        let output_slice = Slice::new(output_buf.clone(), 0..input.len());

        let status_buf = self.alloc_from_device_buf(size_of::<VirtioCryptoDataStatus>());
        let status_slice = Slice::new(status_buf.clone(), 0..size_of::<VirtioCryptoDataStatus>());

        self.submit_request_and_wait(
            QueueSelector::Data,
            request_id,
            &[&req_slice, &iv_slice, &input_slice],
            &[&output_slice, &status_slice],
        )?;

        status_slice
            .mem_obj()
            .sync_from_device(status_slice.offset().clone())
            .unwrap();
        let status: VirtioCryptoDataStatus = status_slice.read_val(0).unwrap();
        Self::check_data_status(&status)?;

        output_slice
            .mem_obj()
            .sync_from_device(output_slice.offset().clone())
            .unwrap();

        Ok(self.read_bytes_from_slice(&output_slice))
    }
}
