// SPDX-License-Identifier: MPL-2.0

use core::mem::offset_of;

use aster_util::safe_ptr::SafePtr;
use ostd_pod::FromZeros;

use crate::transport::{ConfigManager, VirtioTransport};

bitflags::bitflags! {
    pub struct Transport9PFeatures: u64 {
        const MOUNT_TAG = 1 << 0;
    }
}

impl Transport9PFeatures {
    pub fn supported_features() -> Self {
        Self::MOUNT_TAG
    }
}

#[repr(C)]
#[derive(Debug, Pod, Clone, Copy)]
pub struct Virtio9PConfig {
    pub tag_len: u16,
    pub tag: [u8; 36],
}

impl Virtio9PConfig {
    pub(super) fn new_manager(transport: &dyn VirtioTransport) -> ConfigManager<Self> {
        let safe_ptr = transport
            .device_config_mem()
            .map(|mem| SafePtr::new(mem, 0));
        let bar_space = transport.device_config_bar();

        ConfigManager::new(safe_ptr, bar_space)
    }
}

impl ConfigManager<Virtio9PConfig> {
    pub(super) fn read_config(&self) -> Virtio9PConfig {
        let mut config = Virtio9PConfig::new_zeroed();

        config.tag_len = self
            .read_once::<u16>(offset_of!(Virtio9PConfig, tag_len))
            .unwrap();

        let len = (config.tag_len as usize).min(config.tag.len());
        for index in 0..len {
            config.tag[index] = self
                .read_once::<u8>(offset_of!(Virtio9PConfig, tag) + index)
                .unwrap();
        }

        config
    }
}
