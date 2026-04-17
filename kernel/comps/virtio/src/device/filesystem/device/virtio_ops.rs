// SPDX-License-Identifier: MPL-2.0

use core::cmp;

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
        let config = config_manager.read_config()?;

        let notify_supported =
            transport.read_device_features() & FileSystemFeatures::NOTIFICATION.bits() != 0;
        let special_queues_count = if notify_supported { 2 } else { 1 };

        let total_queues = transport.num_queues();
        let max_request_queues_from_transport =
            total_queues.saturating_sub(special_queues_count) as usize;
        let request_queue_count = cmp::min(
            config.num_request_queues() as usize,
            max_request_queues_from_transport,
        );

        if request_queue_count == 0 {
            return Err(VirtioDeviceError::QueuesAmountDoNotMatch(
                total_queues,
                special_queues_count + config.num_request_queues() as u16,
            ));
        }

        let mut request_queues = Vec::with_capacity(request_queue_count);
        for idx in 0..request_queue_count {
            let queue_index = special_queues_count + idx as u16;
            request_queues.push(FsRequestQueue::new(Self::new_queue(
                queue_index,
                transport.as_mut(),
            )?));
        }
        let hiprio_queue =
            FsRequestQueue::new(Self::new_queue(HIPRIO_QUEUE_INDEX, transport.as_mut())?);

        let device = Arc::new(Self {
            transport: SpinLock::new(transport),
            hiprio_queue,
            request_queues,
            to_device_pool: FsDmaPool::new(),
            from_device_pool: FsDmaPool::new(),
            next_unique: AtomicU64::new(1),
            tag: Self::parse_tag(config.tag()).to_string(),
            notify_supported,
        });

        let mut transport = device.transport.lock();
        transport.register_cfg_callback(Box::new(|_: &TrapFrame| {
            debug!("Virtio-FS device configuration space change");
        }))?;

        let device_for_hiprio_callback = device.clone();
        let hiprio_wakeup_callback_fn = move |_: &TrapFrame| {
            device_for_hiprio_callback.handle_queue_irq(&device_for_hiprio_callback.hiprio_queue);
        };
        transport.register_queue_callback(
            HIPRIO_QUEUE_INDEX,
            Box::new(hiprio_wakeup_callback_fn),
            false,
        )?;

        for idx in 0..request_queue_count {
            let queue_index = special_queues_count + idx as u16;
            let device_for_callback = device.clone();
            let wakeup_callback_fn = move |_: &TrapFrame| {
                device_for_callback.handle_queue_irq(&device_for_callback.request_queues[idx]);
            };
            transport.register_queue_callback(queue_index, Box::new(wakeup_callback_fn), false)?;
        }

        transport.finish_init();
        drop(transport);

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
        Ok(())
    }

    fn new_queue(
        index: u16,
        transport: &mut dyn VirtioTransport,
    ) -> Result<VirtQueue, VirtioDeviceError> {
        let max_queue_size = transport
            .max_queue_size(index)
            .map_err(|_: VirtioTransportError| VirtioDeviceError::QueueUnknownError)?;
        let queue_size = cmp::min(DEFAULT_QUEUE_SIZE, max_queue_size);
        if queue_size == 0 {
            return Err(VirtioDeviceError::QueueUnknownError);
        }
        VirtQueue::new(index, queue_size, transport).map_err(Into::into)
    }

    pub(super) fn submit(
        &self,
        queue: &FsRequestQueue,
        request: Arc<FuseRequest>,
    ) -> Result<Arc<FuseRequest>, VirtioDeviceError> {
        let mut virt_queue = queue.queue.lock();
        let input_slices = [&request.in_buf];
        let token = match request.out_buf.as_ref() {
            Some(out_buf) => virt_queue.add_dma_bufs(&input_slices, &[out_buf])?,
            None => virt_queue.add_input_bufs(&input_slices)?,
        };
        let token_idx = token as usize;

        let mut in_flight_requests = queue.in_flight_requests.lock();
        let Some(slot) = in_flight_requests.get_mut(token_idx) else {
            warn!(
                "{} returned an out-of-range token: token={}",
                DEVICE_NAME, token
            );
            return Err(VirtioDeviceError::QueueUnknownError);
        };
        if slot.replace(request.clone()).is_some() {
            warn!(
                "{} unexpectedly reused an in-flight token: token={}",
                DEVICE_NAME, token
            );
            return Err(VirtioDeviceError::QueueUnknownError);
        }

        if virt_queue.should_notify() {
            virt_queue.notify();
        }

        Ok(request)
    }

    pub(super) fn handle_queue_irq(&self, queue_state: &FsRequestQueue) {
        loop {
            let token = {
                let mut queue = queue_state.queue.lock();
                match queue.pop_used() {
                    Ok((token, _)) => token,
                    Err(QueueError::NotReady) => break,
                    Err(err) => {
                        warn!("{} failed to pop used descriptor: {:?}", DEVICE_NAME, err);
                        break;
                    }
                }
            };

            let mut in_flight_requests = queue_state.in_flight_requests.lock();
            let Some(slot) = in_flight_requests.get_mut(token as usize) else {
                warn!(
                    "{} completed an out-of-range token: token={}",
                    DEVICE_NAME, token
                );
                continue;
            };
            let Some(request) = slot.take() else {
                continue;
            };
            request.mark_completed();
        }
    }
}
