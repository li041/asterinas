// SPDX-License-Identifier: MPL-2.0

use alloc::{boxed::Box, collections::btree_map::BTreeMap, sync::Arc, vec::Vec};

use ostd::{
    arch::trap::TrapFrame,
    sync::{LocalIrqDisabled, SpinLock},
};
use spin::Once;

use crate::device::entropy::device::EntropyDevice;

pub mod device;

pub trait EntropyDeviceIrqHandler = Fn() + Send + Sync + 'static;

pub fn register_device(id: usize, device: Arc<EntropyDevice>) {
    ENTROPY_DEVICE_TABLE
        .get()
        .unwrap()
        .lock()
        .insert(id, device);
}

pub fn get_first_device() -> Option<Arc<EntropyDevice>> {
    let lock = ENTROPY_DEVICE_TABLE.get().unwrap().lock();
    let Some((_, device)) = lock.iter().next() else {
        return None;
    };

    Some(device.clone())
}

pub fn all_devices() -> Vec<Arc<EntropyDevice>> {
    let entropy_devs = ENTROPY_DEVICE_TABLE.get().unwrap().lock();
    entropy_devs.values().cloned().collect()
}

pub fn register_recv_callback(callback: impl EntropyDeviceIrqHandler) {
    ENTROPY_DEVICE_CALLBACK.call_once(|| Box::new(callback));
}

pub fn handle_recv_irq(_: &TrapFrame) {
    ENTROPY_DEVICE_CALLBACK.get().unwrap()()
}

pub fn init() {
    ENTROPY_DEVICE_TABLE.call_once(|| SpinLock::new(BTreeMap::new()));
}

pub static ENTROPY_DEVICE_CALLBACK: Once<Box<dyn EntropyDeviceIrqHandler>> = Once::new();

pub static ENTROPY_DEVICE_TABLE: Once<
    SpinLock<BTreeMap<usize, Arc<EntropyDevice>>, LocalIrqDisabled>,
> = Once::new();
