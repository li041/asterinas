// SPDX-License-Identifier: MPL-2.0

//! Aggregated waiter for asynchronous I/O completions.
//!
//! An [`IoBatch`] holds a list of pending [`IoCompletion`]s and waits for
//! all of them at once. It is the generic primitive used by storage
//! subsystems (the block layer, the page cache, future filesystem-specific
//! transports) to express "submit N async operations, wait for all".
//!
//! Backends that complete I/O synchronously inside their submission method
//! simply do not push anything into the batch.

#![no_std]

extern crate alloc;

use alloc::{sync::Arc, vec::Vec};
use core::any::Any;

/// A handle to one in-flight asynchronous I/O.
///
/// Implementations block in [`wait`](Self::wait) until the underlying I/O
/// terminates, then return its outcome. They are typically wrapped in an
/// `Arc` because the same record is held by both the submitter (through an
/// `IoBatch`) and the driver that completes the I/O.
pub trait IoCompletion: Send + Sync + Any {
    fn wait(&self) -> Result<(), IoError>;
}

impl dyn IoCompletion {
    /// Returns a reference to the concrete completion type, if it matches `T`.
    pub fn downcast_ref<T: IoCompletion + 'static>(&self) -> Option<&T> {
        (self as &dyn Any).downcast_ref::<T>()
    }
}

/// A batch of pending [`IoCompletion`]s.
///
/// Used as an out-parameter on async-submission APIs: callers create an
/// `IoBatch`, pass `&mut` to one or more submission calls, then call
/// [`wait_all`](Self::wait_all) to block until everything in the batch
/// finishes.
///
/// A submission that completes synchronously inside its call leaves the
/// batch untouched.
#[must_use]
pub struct IoBatch {
    pending: Vec<Arc<dyn IoCompletion>>,
}

impl IoBatch {
    /// Creates an empty batch.
    pub fn new() -> Self {
        Self {
            pending: Vec::new(),
        }
    }

    /// Creates an empty batch with the specified capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            pending: Vec::with_capacity(capacity),
        }
    }

    /// Returns the completion record at `idx`, if it exists.
    pub fn get(&self, idx: usize) -> Option<&Arc<dyn IoCompletion>> {
        self.pending.get(idx)
    }

    /// Adds one completion record to the batch.
    pub fn push(&mut self, completion: Arc<dyn IoCompletion>) {
        self.pending.push(completion);
    }

    /// Returns the number of pending completions.
    pub fn len(&self) -> usize {
        self.pending.len()
    }

    /// Returns `true` if no completions are pending.
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Waits for every completion in the batch.
    ///
    /// All completions are waited on even if an earlier one fails. The first
    /// observed error is returned.
    pub fn wait_all(&self) -> Result<(), IoError> {
        let mut first_error = None;

        for completion in &self.pending {
            if let Err(error) = completion.wait() {
                first_error.get_or_insert(error);
            }
        }

        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

impl Default for IoBatch {
    fn default() -> Self {
        Self::new()
    }
}

/// A low-level I/O error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IoError {
    /// The operation is not supported by the backend.
    Unsupported,
    /// The device has no free space.
    OutOfSpace,
    /// A generic I/O failure.
    Failed,
}
