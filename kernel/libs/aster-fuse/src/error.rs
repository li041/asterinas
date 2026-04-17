// SPDX-License-Identifier: MPL-2.0

//! FUSE error type used by [`crate::FuseOperation`] implementations.

#[derive(Debug)]
pub enum FuseError {
    BufferTooSmall,
    LengthOverflow,
    RemoteError(i32),
}

pub type FuseResult<T> = core::result::Result<T, FuseError>;
