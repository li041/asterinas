// SPDX-License-Identifier: MPL-2.0

use core::mem::size_of;

use ostd::mm::VmIo;

use super::*;

impl CryptoDevice {
    pub fn create_cipher_session(
        &self,
        algo: u32,
        key: &[u8],
        _iv_len: u32,
        op: u32,
    ) -> Result<u64, VirtioDeviceError> {
        let request_id = self.alloc_request_id();
        let session_hint = self.alloc_session_id_hint();

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

        let submit_res = self.submit_request_and_wait(
            QueueSelector::Control,
            request_id,
            &[&req_slice, &key_slice],
            &[&status_slice],
        );

        if submit_res.is_err() {
            self.dealloc_session_id_hint(session_hint);
            return submit_res.map(|_| 0);
        }

        status_slice
            .mem_obj()
            .sync_from_device(status_slice.offset().clone())
            .unwrap();
        let status: VirtioCryptoSessionInput = status_slice.read_val(0).unwrap();

        if Self::check_ctrl_status(&status).is_err() {
            self.dealloc_session_id_hint(session_hint);
            return Err(VirtioDeviceError::QueueUnknownError);
        }

        self.dealloc_session_id_hint(session_hint);

        Ok(status.session_id)
    }

    pub fn close_cipher_session(
        &self,
        _algo: u32,
        session_id: u64,
    ) -> Result<(), VirtioDeviceError> {
        let request_id = self.alloc_request_id();

        let req = VirtioCryptoDestroySessionReq {
            header: Self::build_ctrl_header(VIRTIO_CRYPTO_CIPHER_DESTROY_SESSION, 0),
            session_id,
            padding: [0u8; 48],
        };

        let req_buf = self.alloc_to_device_buf(size_of::<VirtioCryptoDestroySessionReq>());
        let req_slice = Slice::new(
            req_buf.clone(),
            0..size_of::<VirtioCryptoDestroySessionReq>(),
        );
        req_slice.write_val(0, &req).unwrap();
        req_slice
            .mem_obj()
            .sync_to_device(req_slice.offset().clone())
            .unwrap();

        let status_buf = self.alloc_from_device_buf(size_of::<VirtioCryptoInHdr>());
        let status_slice = Slice::new(status_buf.clone(), 0..size_of::<VirtioCryptoInHdr>());

        self.submit_request_and_wait(
            QueueSelector::Control,
            request_id,
            &[&req_slice],
            &[&status_slice],
        )?;

        status_slice
            .mem_obj()
            .sync_from_device(status_slice.offset().clone())
            .unwrap();
        let status: VirtioCryptoInHdr = status_slice.read_val(0).unwrap();
        Self::check_inhdr_status(status.status)
    }

    pub fn encrypt(
        &self,
        algo: u32,
        session_id: u64,
        iv: &[u8],
        plaintext: &[u8],
    ) -> Result<Vec<u8>, VirtioDeviceError> {
        self.crypto_op(
            VIRTIO_CRYPTO_CIPHER_ENCRYPT,
            algo,
            session_id,
            iv,
            plaintext,
        )
    }

    pub fn decrypt(
        &self,
        algo: u32,
        session_id: u64,
        iv: &[u8],
        ciphertext: &[u8],
    ) -> Result<Vec<u8>, VirtioDeviceError> {
        self.crypto_op(
            VIRTIO_CRYPTO_CIPHER_DECRYPT,
            algo,
            session_id,
            iv,
            ciphertext,
        )
    }
}
