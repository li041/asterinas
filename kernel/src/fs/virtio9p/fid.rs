// SPDX-License-Identifier: MPL-2.0

//! FID manager for 9P — manages server-side file identifiers.

use alloc::{collections::BTreeMap, sync::Arc, vec::Vec};
use core::sync::atomic::{AtomicU32, Ordering};

use aster_virtio::device::transport9p::{
    device::Transport9PDevice,
    protocol::{P9Qid, P9_NOFID},
};
use log::warn;
use ostd::sync::SpinLock;

/// Information tracked per active FID.
#[derive(Debug, Clone)]
pub(super) struct FidInfo {
    pub qid: Option<P9Qid>,
}

/// Manages FID allocation and tracking for a 9P session.
pub(super) struct FidManager {
    device: Arc<Transport9PDevice>,
    next_fid: AtomicU32,
    active_fids: SpinLock<BTreeMap<u32, FidInfo>>,
}

impl FidManager {
    pub fn new(device: Arc<Transport9PDevice>) -> Self {
        Self {
            device,
            next_fid: AtomicU32::new(1),
            active_fids: SpinLock::new(BTreeMap::new()),
        }
    }

    /// Allocate a new unique FID number.
    pub fn alloc_fid(&self) -> u32 {
        let fid = self.next_fid.fetch_add(1, Ordering::Relaxed);
        self.active_fids
            .disable_irq()
            .lock()
            .insert(fid, FidInfo { qid: None });
        fid
    }

    /// Walk from parent_fid along names, allocating a new FID.
    /// Returns (new_fid, qids_from_walk).
    pub fn walk(
        &self,
        parent_fid: u32,
        names: &[&str],
    ) -> Result<(u32, Vec<P9Qid>), aster_virtio::device::VirtioDeviceError> {
        let newfid = self.alloc_fid();
        match self.device.p9_walk(parent_fid, newfid, names) {
            Ok(qids) => {
                if let Some(qid) = qids.last() {
                    self.active_fids
                        .disable_irq()
                        .lock()
                        .entry(newfid)
                        .and_modify(|info| info.qid = Some(*qid));
                }
                Ok((newfid, qids))
            }
            Err(e) => {
                // Walk failed; remove the FID from tracking.
                self.active_fids.disable_irq().lock().remove(&newfid);
                Err(e)
            }
        }
    }

    /// Clone a FID by walking with empty names.
    pub fn clone_fid(
        &self,
        fid: u32,
    ) -> Result<u32, aster_virtio::device::VirtioDeviceError> {
        let (newfid, _) = self.walk(fid, &[])?;
        Ok(newfid)
    }

    /// Clunk (release) a FID.
    pub fn clunk(&self, fid: u32) {
        if fid == P9_NOFID {
            return;
        }
        self.active_fids.disable_irq().lock().remove(&fid);
        if let Err(e) = self.device.p9_clunk(fid) {
            warn!("9P clunk fid {} failed: {:?}", fid, e);
        }
    }

    pub fn device(&self) -> &Arc<Transport9PDevice> {
        &self.device
    }
}

impl Drop for FidManager {
    fn drop(&mut self) {
        let fids: Vec<u32> = self.active_fids.get_mut().keys().copied().collect();
        for fid in fids {
            let _ = self.device.p9_clunk(fid);
        }
    }
}
