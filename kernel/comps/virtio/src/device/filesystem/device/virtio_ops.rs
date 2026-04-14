// SPDX-License-Identifier: MPL-2.0

use super::*;

impl FileSystemDevice {
    pub fn negotiate_features(features: u64) -> u64 {
        let device_features = FileSystemFeatures::from_bits_truncate(features);
        let supported_features = FileSystemFeatures::supported_features();
        let fs_features = device_features & supported_features;
        debug!("features negotiated: {:?}", fs_features);
        fs_features.bits()
    }

    pub(crate) fn init(mut transport: Box<dyn VirtioTransport>) -> Result<(), VirtioDeviceError> {
        let config_manager = VirtioFsConfig::new_manager(transport.as_ref());
        let config = config_manager.read_config();

        let notify_supported =
            transport.read_device_features() & FileSystemFeatures::NOTIFICATION.bits() != 0;
        let special_queues_count = if notify_supported { 2 } else { 1 };

        let total_queues = transport.num_queues();
        let max_request_queues_from_transport =
            total_queues.saturating_sub(special_queues_count) as usize;
        let request_queue_count = cmp::min(
            config.num_request_queues as usize,
            max_request_queues_from_transport,
        );

        if request_queue_count == 0 {
            return Err(VirtioDeviceError::QueuesAmountDoNotMatch(
                total_queues,
                special_queues_count + config.num_request_queues as u16,
            ));
        }

        let hiprio_queue =
            FsRequestQueue::new(Self::new_queue(HIPRIO_QUEUE_INDEX, transport.as_mut())?);

        let to_device_pool = FsDmaPool::new();
        let from_device_pool = FsDmaPool::new();

        let mut request_queues = Vec::with_capacity(request_queue_count);
        for idx in 0..request_queue_count {
            let queue_index = special_queues_count + idx as u16;
            request_queues.push(FsRequestQueue::new(Self::new_queue(
                queue_index,
                transport.as_mut(),
            )?));
        }

        let tag = Self::parse_tag(&config.tag);
        let device = Arc::new(Self {
            transport: SpinLock::new(transport),
            hiprio_queue,
            request_queues,
            to_device_pool,
            from_device_pool,
            next_unique: AtomicU64::new(0),
            tag: tag.to_string(),
            notify_supported,
        });

        let mut transport = device.transport.lock();
        transport
            .register_cfg_callback(Box::new(config_space_change))
            .unwrap();
        let device_for_hiprio_callback = device.clone();
        let hiprio_wakeup_callback = move |_: &TrapFrame| {
            device_for_hiprio_callback.handle_queue_irq(QueueSelector::Hiprio);
        };
        transport
            .register_queue_callback(HIPRIO_QUEUE_INDEX, Box::new(hiprio_wakeup_callback), false)
            .unwrap();
        for idx in 0..request_queue_count {
            let queue_idx = special_queues_count + idx as u16;
            let device_for_callback = device.clone();
            let wakeup_callback = move |_: &TrapFrame| {
                device_for_callback.handle_queue_irq(QueueSelector::Request(
                    (queue_idx - special_queues_count) as usize,
                ));
            };
            transport
                .register_queue_callback(queue_idx, Box::new(wakeup_callback), false)
                .unwrap();
        }
        transport.finish_init();
        drop(transport);

        device.fuse_init()?;

        FILESYSTEM_DEVICES
            .call_once(|| SpinLock::new(Vec::new()))
            .disable_irq()
            .lock()
            .push(device.clone());

        info!(
            "{} initialized, tag = {}, request_queues = {}, notify = {}",
            DEVICE_NAME,
            device.tag,
            device.request_queues.len(),
            device.notify_supported
        );
        info!(
            "{} test file read is deferred; call debug_read_test_file_for_all_devices() later",
            DEVICE_NAME
        );

        let _ = &device.hiprio_queue;
        let _ = &device.request_queues;

        Ok(())
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

    pub(super) fn queue(&self, selector: QueueSelector) -> &FsRequestQueue {
        match selector {
            QueueSelector::Hiprio => &self.hiprio_queue,
            QueueSelector::Request(index) => &self.request_queues[index],
        }
    }

    pub(super) fn select_request_queue(&self, nodeid: u64) -> QueueSelector {
        let request_queue_count = self.request_queues.len();
        if request_queue_count <= 1 {
            return QueueSelector::Request(0);
        }

        QueueSelector::Request((nodeid as usize) % request_queue_count)
    }

    pub(super) fn submit_request(
        &self,
        selector: QueueSelector,
        unique: u64,
        input_buffers: Vec<FsInBuf>,
        output_buffers: Vec<FsOutBuf>,
    ) -> Result<Arc<FsRequest>, VirtioDeviceError> {
        let queue = self.queue(selector);

        {
            let mut virt_queue = queue.queue.lock();
            let input_slices: Vec<_> = input_buffers.iter().collect();
            let output_slices: Vec<_> = output_buffers.iter().collect();

            let token =
                virt_queue.add_dma_buf(input_slices.as_slice(), output_slices.as_slice())?;
            let token_idx = token as usize;
            let request = FsRequest::new(input_buffers, output_buffers);

            let mut in_flight_requests = queue.in_flight_requests.lock();
            let Some(slot) = in_flight_requests.get_mut(token_idx) else {
                warn!(
                    "{} returned an out-of-range token: queue={:?}, token={}, unique={}",
                    DEVICE_NAME, selector, token, unique
                );
                return Err(VirtioDeviceError::QueueUnknownError);
            };
            if slot.replace(request.clone()).is_some() {
                warn!(
                    "{} unexpectedly reused an in-flight token: queue={:?}, token={}, unique={}",
                    DEVICE_NAME, selector, token, unique
                );
                return Err(VirtioDeviceError::QueueUnknownError);
            }

            if virt_queue.should_notify() {
                virt_queue.notify();
            }

            Ok(request)
        }
    }

    pub(super) fn submit_request_and_wait(
        &self,
        selector: QueueSelector,
        unique: u64,
        input_buffers: Vec<FsInBuf>,
        output_buffers: Vec<FsOutBuf>,
    ) -> Result<Arc<FsRequest>, VirtioDeviceError> {
        let request = self.submit_request(selector, unique, input_buffers, output_buffers)?;
        self.wait_for_request(&request).map(|_| request)
    }

    pub(super) fn handle_queue_irq(&self, selector: QueueSelector) {
        let queue_state = self.queue(selector);
        loop {
            let token = {
                let mut queue = queue_state.queue.lock();
                match queue.pop_used() {
                    Ok((token, _)) => token,
                    Err(QueueError::NotReady) => break,
                    Err(_) => break,
                }
            };

            let mut in_flight_requests = queue_state.in_flight_requests.lock();
            let Some(slot) = in_flight_requests.get_mut(token as usize) else {
                warn!(
                    "{} completed an out-of-range token: queue={:?}, token={}",
                    DEVICE_NAME, selector, token
                );
                continue;
            };
            let Some(request) = slot.take() else {
                continue;
            };

            let waker = {
                let mut wait_state = request.wait_state.lock();
                wait_state.completed = true;
                wait_state.waker.take()
            };

            if let Some(waker) = waker {
                let _ = waker.wake_up();
            }
        }
    }

    pub(super) fn wait_for_request(
        &self,
        request: &Arc<FsRequest>,
    ) -> Result<(), VirtioDeviceError> {
        let mut wait_state = request.wait_state.lock();
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
                if request.wait_state.lock().completed {
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

        let mut wait_state = request.wait_state.lock();
        if wait_state.completed {
            return Ok(());
        }
        wait_state.waker = None;

        Err(VirtioDeviceError::QueueUnknownError)
    }

    /// Wait for a reply from the device by spinning;
    /// intended for early boot or non-task contexts.
    pub(super) fn wait_for_request_early(
        &self,
        selector: QueueSelector,
        request: &Arc<FsRequest>,
    ) -> Result<(), VirtioDeviceError> {
        loop {
            self.handle_queue_irq(selector);

            if request.wait_state.lock().completed {
                return Ok(());
            }

            spin_loop();
        }
    }
}
