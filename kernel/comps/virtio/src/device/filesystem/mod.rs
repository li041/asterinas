// SPDX-License-Identifier: MPL-2.0

//! Virtio filesystem device support.
//!
//! This module groups the virtio-fs configuration definitions, device-side
//! request handling, and DMA buffer management used by the virtio subsystem.

pub mod config;
pub mod device;
pub mod pool;

/// Identifies the virtio-fs device in logs and diagnostics.
pub const DEVICE_NAME: &str = "Virtio-FS";
