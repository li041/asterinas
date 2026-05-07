// SPDX-License-Identifier: MPL-2.0

use core::cmp;

use super::*;

impl FileSystemDevice {
    pub(crate) fn negotiate_features(features: u64) -> u64 {
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
            config.num_request_queues() as usize,
            max_request_queues_from_transport,
        );

        if request_queue_count == 0 {
            return Err(VirtioDeviceError::UnsupportedConfig);
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

        let device = Arc::new(Self::new(
            transport,
            hiprio_queue,
            request_queues,
            Self::parse_tag(config.tag()).to_string(),
            notify_supported,
        ));

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
            .map_err(VirtioDeviceError::from)?;
        let queue_size = cmp::min(DEFAULT_QUEUE_SIZE, max_queue_size);
        if queue_size == 0 {
            return Err(VirtioDeviceError::UnsupportedConfig);
        }
        VirtQueue::new(index, queue_size, transport).map_err(Into::into)
    }

    pub(super) fn submit(
        &self,
        request_queue: &FsRequestQueue,
        request: Arc<FuseRequest>,
    ) -> Arc<FuseRequest> {
        let mut queue = request_queue.queue.lock();
        let input_slices = [&request.in_buf];
        let token = match request.out_buf.as_ref() {
            Some(out_buf) => queue.add_dma_bufs(&input_slices, &[out_buf]).unwrap(),
            None => queue.add_input_bufs(&input_slices).unwrap(),
        };
        let token_idx = token as usize;

        let mut in_flight_requests = request_queue.in_flight_requests.lock();
        let slot = in_flight_requests.get_mut(token_idx).unwrap();
        slot.replace(request.clone());

        if queue.should_notify() {
            queue.notify();
        }

        request
    }

    pub(super) fn handle_queue_irq(&self, queue_state: &FsRequestQueue) {
        loop {
            let mut queue = queue_state.queue.lock();

            let token = {
                match queue.pop_used() {
                    Ok((token, _)) => token,
                    Err(PopUsedError::NotReady) => break,
                }
            };

            let mut in_flight_requests = queue_state.in_flight_requests.lock();
            let slot = in_flight_requests.get_mut(token as usize).unwrap();
            let Some(request) = slot.take() else {
                continue;
            };
            request.mark_completed();
        }
    }
}
