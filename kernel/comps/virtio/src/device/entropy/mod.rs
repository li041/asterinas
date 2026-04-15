// SPDX-License-Identifier: MPL-2.0

//! Manages virtio entropy devices.
//!
//! This module owns the global registry of discovered [`EntropyDevice`] instances.
//! Virtio transport initialization creates devices in [`device`], then registers
//! them here under stable names such as `virtio_rng.0`. Callers can look up one
//! device by name with `get_device` or snapshot the current registry with
//! `all_devices`.
//!
//! The kernel crate consumes this registry from `/dev/hwrng`: its misc-device
//! frontend selects one registered device as the current hardware RNG and blocks
//! on that device's wait queue until fresh entropy arrives.

use alloc::{collections::btree_map::BTreeMap, string::String, sync::Arc, vec::Vec};

use ostd::sync::SpinLock;
use spin::Once;

use crate::device::entropy::device::EntropyDevice;

pub mod device;

pub fn register_device(name: String, device: Arc<EntropyDevice>) {
    ENTROPY_DEVICE_TABLE
        .get()
        .unwrap()
        .lock()
        .insert(name, device);
}

pub fn get_device(name: &str) -> Option<Arc<EntropyDevice>> {
    let lock = ENTROPY_DEVICE_TABLE
        .get()
        .expect("entropy::init() must run before get_device()")
        .lock();
    lock.get(name).cloned()
}

pub fn all_devices() -> Vec<(String, Arc<EntropyDevice>)> {
    let entropy_devs = ENTROPY_DEVICE_TABLE.get().unwrap().lock();

    entropy_devs
        .iter()
        .map(|(name, dev)| (name.clone(), dev.clone()))
        .collect()
}

pub(crate) fn init() {
    ENTROPY_DEVICE_TABLE.call_once(|| SpinLock::new(BTreeMap::new()));
}

static ENTROPY_DEVICE_TABLE: Once<SpinLock<BTreeMap<String, Arc<EntropyDevice>>>> = Once::new();
