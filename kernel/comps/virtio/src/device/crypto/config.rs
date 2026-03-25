// SPDX-License-Identifier: MPL-2.0

use core::mem::offset_of;

use aster_util::safe_ptr::SafePtr;
use ostd_pod::FromZeros;

use crate::transport::{ConfigManager, VirtioTransport};

bitflags::bitflags! {
    pub struct CryptoFeatures: u64 {}
}

impl CryptoFeatures {
    pub fn supported_features() -> Self {
        Self::empty()
    }
}

#[repr(C)]
#[derive(Debug, Pod, Clone, Copy)]
pub struct VirtioCryptoConfig {
    pub status: u32,
    pub max_dataqueues: u32,
    pub crypto_services: u32,
    pub cipher_algo_l: u32,
    pub cipher_algo_h: u32,
    pub hash_algo: u32,
    pub mac_algo_l: u32,
    pub mac_algo_h: u32,
    pub aead_algo: u32,
    pub akcipher_algo: u32,
}

impl VirtioCryptoConfig {
    pub(super) fn new_manager(transport: &dyn VirtioTransport) -> ConfigManager<Self> {
        let safe_ptr = transport
            .device_config_mem()
            .map(|mem| SafePtr::new(mem, 0));
        let bar_space = transport.device_config_bar();

        ConfigManager::new(safe_ptr, bar_space)
    }
}

impl ConfigManager<VirtioCryptoConfig> {
    pub(super) fn read_config(&self) -> VirtioCryptoConfig {
        let mut config = VirtioCryptoConfig::new_zeroed();

        config.status = self
            .read_once::<u32>(offset_of!(VirtioCryptoConfig, status))
            .unwrap();
        config.max_dataqueues = self
            .read_once::<u32>(offset_of!(VirtioCryptoConfig, max_dataqueues))
            .unwrap();
        config.crypto_services = self
            .read_once::<u32>(offset_of!(VirtioCryptoConfig, crypto_services))
            .unwrap();
        config.cipher_algo_l = self
            .read_once::<u32>(offset_of!(VirtioCryptoConfig, cipher_algo_l))
            .unwrap();
        config.cipher_algo_h = self
            .read_once::<u32>(offset_of!(VirtioCryptoConfig, cipher_algo_h))
            .unwrap();
        config.hash_algo = self
            .read_once::<u32>(offset_of!(VirtioCryptoConfig, hash_algo))
            .unwrap();
        config.mac_algo_l = self
            .read_once::<u32>(offset_of!(VirtioCryptoConfig, mac_algo_l))
            .unwrap();
        config.mac_algo_h = self
            .read_once::<u32>(offset_of!(VirtioCryptoConfig, mac_algo_h))
            .unwrap();
        config.aead_algo = self
            .read_once::<u32>(offset_of!(VirtioCryptoConfig, aead_algo))
            .unwrap();
        config.akcipher_algo = self
            .read_once::<u32>(offset_of!(VirtioCryptoConfig, akcipher_algo))
            .unwrap();

        config
    }
}
