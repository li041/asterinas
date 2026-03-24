// SPDX-License-Identifier: MPL-2.0

use super::*;

impl Transport9PDevice {
    pub fn negotiate_features(features: u64) -> u64 {
        let device_features = Transport9PFeatures::from_bits_truncate(features);
        let supported_features = Transport9PFeatures::supported_features();
        let fs_features = device_features & supported_features;
        debug!("9P features negotiated: {:?}", fs_features);
        fs_features.bits()
    }

    pub fn init(mut transport: Box<dyn VirtioTransport>) -> Result<(), VirtioDeviceError> {
        let config_manager = Virtio9PConfig::new_manager(transport.as_ref());
        let config = config_manager.read_config();

        let total_queues = transport.num_queues();
        if total_queues < 1 {
            return Err(VirtioDeviceError::QueuesAmountDoNotMatch(total_queues, 1));
        }

        let request_queue =
            P9RequestQueue::new(Self::new_queue(REQUEST_QUEUE_INDEX, transport.as_mut())?);

        let dma_pools = P9DmaPools::new();

        let tag = Self::parse_tag(&config.tag, config.tag_len);
        let device = Arc::new(Self {
            transport: SpinLock::new(transport),
            request_queue,
            dma_pools,
            tag_alloc: SyncIdAlloc::with_capacity(TAG_ALLOC_CAPACITY),
            tag,
            msize: AtomicU32::new(DEFAULT_MSIZE),
        });

        let mut transport = device.transport.lock();
        transport
            .register_cfg_callback(Box::new(config_space_change))
            .unwrap();

        let device_for_callback = device.clone();
        let wakeup_callback = move |_: &TrapFrame| {
            device_for_callback.handle_queue_irq();
        };
        transport
            .register_queue_callback(REQUEST_QUEUE_INDEX, Box::new(wakeup_callback), false)
            .unwrap();

        transport.finish_init();
        drop(transport);

        // Perform 9P version negotiation.
        device.p9_version()?;

        TRANSPORT9P_DEVICES
            .call_once(|| SpinLock::new(Vec::new()))
            .disable_irq()
            .lock()
            .push(device.clone());

        info!(
            "{} initialized, tag = {}, msize = {}",
            DEVICE_NAME,
            device.tag,
            device.msize.load(Ordering::Relaxed),
        );

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
}
