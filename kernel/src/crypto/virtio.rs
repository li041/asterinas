// SPDX-License-Identifier: MPL-2.0

#![allow(dead_code)]

use alloc::sync::Arc;

use aster_virtio::device::crypto::device::{CryptoDevice, get_default_device};

use crate::{
    error::Errno,
    prelude::{Error, Result},
};

pub struct VirtioCrypto {
    device: Arc<CryptoDevice>,
}

impl VirtioCrypto {
    pub fn new_default() -> Result<Self> {
        let device = get_default_device()
            .ok_or_else(|| Error::with_message(Errno::ENODEV, "virtio-crypto device not found"))?;
        Ok(Self { device })
    }

    pub fn open_cipher_session(&self, algo: u32, key: &[u8], iv_len: u32) -> Result<CryptoSession> {
        let enc_session_id = self
            .device
            .create_cipher_session(
                algo,
                key,
                iv_len,
                aster_virtio::device::crypto::protocol::VIRTIO_CRYPTO_OP_ENCRYPT,
            )
            .map_err(Error::from)?;

        let dec_session_id = self
            .device
            .create_cipher_session(
                algo,
                key,
                iv_len,
                aster_virtio::device::crypto::protocol::VIRTIO_CRYPTO_OP_DECRYPT,
            )
            .map_err(Error::from)?;

        Ok(CryptoSession {
            device: self.device.clone(),
            algo,
            enc_session_id,
            dec_session_id,
        })
    }
}

pub struct CryptoSession {
    device: Arc<CryptoDevice>,
    algo: u32,
    enc_session_id: u64,
    dec_session_id: u64,
}

impl CryptoSession {
    pub fn session_id(&self) -> u64 {
        self.enc_session_id
    }

    pub fn encrypt(&self, iv: &[u8], plaintext: &[u8]) -> Result<alloc::vec::Vec<u8>> {
        self.device
            .encrypt(self.algo, self.enc_session_id, iv, plaintext)
            .map_err(Error::from)
    }

    pub fn decrypt(&self, iv: &[u8], ciphertext: &[u8]) -> Result<alloc::vec::Vec<u8>> {
        self.device
            .decrypt(self.algo, self.dec_session_id, iv, ciphertext)
            .map_err(Error::from)
    }

    pub fn close(self) -> Result<()> {
        self.device
            .close_cipher_session(self.algo, self.enc_session_id)
            .map_err(Error::from)?;
        self.device
            .close_cipher_session(self.algo, self.dec_session_id)
            .map_err(Error::from)
    }
}
