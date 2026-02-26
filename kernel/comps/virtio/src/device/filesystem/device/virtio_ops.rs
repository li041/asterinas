// SPDX-License-Identifier: MPL-2.0

use super::*;

impl FileSystemDevice {
    pub(super) fn new_queue(
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

    pub(super) fn queue_state(&self, selector: RequestQueueSelector) -> &FsRequestQueue {
        match selector {
            RequestQueueSelector::Hiprio => &self.hiprio_queue,
            RequestQueueSelector::Request(index) => &self.request_queues[index],
        }
    }

    pub(super) fn submit_request_and_wait(
        &self,
        queue_index: usize,
        unique: u64,
        in_slices: &[&Slice<FsDmaBuf>],
        out_slices: &[&Slice<FsDmaBuf>],
    ) -> Result<(), VirtioDeviceError> {
        {
            let mut queue = self.request_queues[queue_index].queue.lock();
            let token = queue.add_dma_buf(in_slices, out_slices)?;
            self.register_pending_request(queue_index, token, unique);
            if queue.should_notify() {
                queue.notify();
            }
        }

        self.wait_for_unique(queue_index, unique)
    }

    pub(super) fn submit_request_and_wait_early(
        &self,
        queue_index: usize,
        unique: u64,
        in_slices: &[&Slice<FsDmaBuf>],
        out_slices: &[&Slice<FsDmaBuf>],
    ) -> Result<(), VirtioDeviceError> {
        {
            let mut queue = self.request_queues[queue_index].queue.lock();
            let token = queue.add_dma_buf(in_slices, out_slices)?;
            self.register_pending_request(queue_index, token, unique);
            if queue.should_notify() {
                queue.notify();
            }
        }

        self.wait_for_unique_early(queue_index, unique)
    }

    pub(super) fn read_reply_header(
        &self,
        out_header_slice: &Slice<FsDmaBuf>,
        unique: u64,
        op_name: &str,
        map_fs_error: bool,
    ) -> Result<OutHeader, VirtioDeviceError> {
        out_header_slice
            .mem_obj()
            .sync_from_device(out_header_slice.offset().clone())
            .unwrap();
        let out_header: OutHeader = out_header_slice.read_val(0).unwrap();
        if out_header.unique != unique || out_header.error != 0 {
            warn!(
                "{} {} failed: unique={}, error={}, out_len={}",
                DEVICE_NAME, op_name, out_header.unique, out_header.error, out_header.len
            );
            if map_fs_error && out_header.unique == unique && out_header.error != 0 {
                return Err(VirtioDeviceError::FileSystemError(out_header.error));
            }
            return Err(VirtioDeviceError::QueueUnknownError);
        }

        Ok(out_header)
    }

    pub(super) fn register_pending_request(&self, queue_index: usize, token: u16, unique: u64) {
        self.register_pending_request_with_reply(queue_index, token, unique);
    }

    pub(super) fn register_pending_request_with_reply(
        &self,
        queue_index: usize,
        token: u16,
        unique: u64,
    ) {
        self.register_pending_request_on_queue(
            RequestQueueSelector::Request(queue_index),
            token,
            unique,
        );
    }

    pub(super) fn register_pending_request_on_queue(
        &self,
        selector: RequestQueueSelector,
        token: u16,
        unique: u64,
    ) {
        let queue_state = self.queue_state(selector);
        queue_state
            .pending_requests
            .disable_irq()
            .lock()
            .insert(token, Self::unique_id(unique));
    }

    pub(super) fn handle_queue_irq(&self, selector: RequestQueueSelector) {
        let queue_state = self.queue_state(selector);
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
        queue_index: usize,
        unique: u64,
    ) -> Result<(), VirtioDeviceError> {
        self.wait_for_unique_on(RequestQueueSelector::Request(queue_index), unique)
    }

    pub(super) fn wait_for_unique_on(
        &self,
        selector: RequestQueueSelector,
        unique: u64,
    ) -> Result<(), VirtioDeviceError> {
        let queue_state = self.queue_state(selector);
        let unique_id = Self::unique_id(unique);

        {
            let mut request_states = queue_state.request_states.disable_irq().lock();
            if let Some(state) = request_states.get(&unique_id)
                && state.completed
            {
                request_states.remove(&unique_id);
                self.unique_id_alloc.dealloc(unique_id);
                return Ok(());
            }
        }

        let (waiter, waker) = Waiter::new_pair();
        {
            let mut request_states = queue_state.request_states.disable_irq().lock();
            let state = request_states.entry(unique_id).or_insert(RequestWaitState {
                completed: false,
                waker: None,
            });
            if state.completed {
                request_states.remove(&unique_id);
                self.unique_id_alloc.dealloc(unique_id);
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
                if let Some(state) = request_states.get(&unique_id)
                    && state.completed
                {
                    request_states.remove(&unique_id);
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
            self.unique_id_alloc.dealloc(unique_id);
            return Ok(());
        }

        let mut request_states = queue_state.request_states.disable_irq().lock();
        if let Some(state) = request_states.get_mut(&unique_id) {
            state.waker = None;
        }
        request_states.remove(&unique_id);
        self.unique_id_alloc.dealloc(unique_id);

        Err(VirtioDeviceError::QueueUnknownError)
    }

    /// Wait for a reply from the device by spinning;
    /// intended for early boot or non-task contexts.
    pub(super) fn wait_for_unique_early(
        &self,
        queue_index: usize,
        unique: u64,
    ) -> Result<(), VirtioDeviceError> {
        let selector = RequestQueueSelector::Request(queue_index);
        let queue_state = self.queue_state(selector);
        let unique_id = Self::unique_id(unique);

        loop {
            self.handle_queue_irq(selector);

            let mut request_states = queue_state.request_states.disable_irq().lock();
            if let Some(state) = request_states.get(&unique_id)
                && state.completed
            {
                request_states.remove(&unique_id);
                self.unique_id_alloc.dealloc(unique_id);
                return Ok(());
            }

            spin_loop();
        }
    }
}
