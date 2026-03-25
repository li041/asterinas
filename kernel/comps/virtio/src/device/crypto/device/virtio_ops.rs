// SPDX-License-Identifier: MPL-2.0

use core::mem::size_of;

use ostd::mm::VmIo;

use super::*;

impl CryptoDevice {
    pub fn negotiate_features(features: u64) -> u64 {
        let device_features = CryptoFeatures::from_bits_truncate(features);
        let supported_features = CryptoFeatures::supported_features();
        let crypto_features = device_features & supported_features;
        debug!("crypto features negotiated: {:?}", crypto_features);
        crypto_features.bits()
    }

    pub fn init(mut transport: Box<dyn VirtioTransport>) -> Result<(), VirtioDeviceError> {
        let config_manager = VirtioCryptoConfig::new_manager(transport.as_ref());
        let config = config_manager.read_config();

        let total_queues = transport.num_queues();
        if total_queues < 2 {
            return Err(VirtioDeviceError::QueuesAmountDoNotMatch(total_queues, 2));
        }

        let control_queue = CryptoQueue::new(Self::new_queue(CONTROLQ_INDEX, transport.as_mut())?);
        let data_queue = CryptoQueue::new(Self::new_queue(DATAQ_INDEX, transport.as_mut())?);

        let device = Arc::new(Self {
            transport: SpinLock::new(transport),
            config,
            control_queue,
            data_queue,
            dma_pools: CryptoDmaPools::new(),
            request_id_alloc: SyncIdAlloc::with_capacity(REQUEST_ID_ALLOC_CAPACITY),
            session_id_alloc: SyncIdAlloc::with_capacity(SESSION_ID_ALLOC_CAPACITY),
        });

        let mut transport = device.transport.lock();
        transport
            .register_cfg_callback(Box::new(config_space_change))
            .unwrap();

        let control_device = device.clone();
        let control_wakeup = move |_: &TrapFrame| {
            control_device.handle_queue_irq(QueueSelector::Control);
        };
        transport
            .register_queue_callback(CONTROLQ_INDEX, Box::new(control_wakeup), false)
            .unwrap();

        let data_device = device.clone();
        let data_wakeup = move |_: &TrapFrame| {
            data_device.handle_queue_irq(QueueSelector::Data);
        };
        transport
            .register_queue_callback(DATAQ_INDEX, Box::new(data_wakeup), false)
            .unwrap();

        transport.finish_init();
        drop(transport);

        CRYPTO_DEVICES
            .call_once(|| SpinLock::new(Vec::new()))
            .disable_irq()
            .lock()
            .push(device.clone());

        info!(
            "{} initialized, max_dataqueues={}, services={:#x}",
            DEVICE_NAME,
            device.max_dataqueues(),
            device.config.crypto_services
        );

        Ok(())
    }

    pub fn create_cipher_session_early(
        &self,
        algo: u32,
        key: &[u8],
        _iv_len: u32,
        op: u32,
    ) -> Result<u64, VirtioDeviceError> {
        let request_id = self.alloc_request_id();

        let req = VirtioCryptoCreateSessionReq {
            header: Self::build_ctrl_header(VIRTIO_CRYPTO_CIPHER_CREATE_SESSION, algo),
            para: VirtioCryptoCipherSessionPara {
                algo,
                keylen: key.len() as u32,
                op,
                padding: 0,
            },
            cipher_padding: [0u8; 32],
            op_type: VIRTIO_CRYPTO_SYM_OP_CIPHER,
            padding: 0,
        };

        let req_buf = self.alloc_to_device_buf(size_of::<VirtioCryptoCreateSessionReq>());
        let req_slice = Slice::new(
            req_buf.clone(),
            0..size_of::<VirtioCryptoCreateSessionReq>(),
        );
        req_slice.write_val(0, &req).unwrap();
        req_slice
            .mem_obj()
            .sync_to_device(req_slice.offset().clone())
            .unwrap();

        let key_buf = self.alloc_to_device_buf(key.len());
        let key_slice = Slice::new(key_buf.clone(), 0..key.len());
        self.write_bytes_to_buf(&key_buf, key);
        key_slice
            .mem_obj()
            .sync_to_device(key_slice.offset().clone())
            .unwrap();

        let status_buf = self.alloc_from_device_buf(size_of::<VirtioCryptoSessionInput>());
        let status_slice = Slice::new(status_buf.clone(), 0..size_of::<VirtioCryptoSessionInput>());

        self.submit_request(
            QueueSelector::Control,
            request_id,
            &[&req_slice, &key_slice],
            &[&status_slice],
        )?;
        self.wait_for_request_early(QueueSelector::Control, request_id as usize)?;

        status_slice
            .mem_obj()
            .sync_from_device(status_slice.offset().clone())
            .unwrap();

        let status: VirtioCryptoSessionInput = status_slice.read_val(0).unwrap();
        Self::check_ctrl_status(&status)?;

        Ok(status.session_id)
    }
}
