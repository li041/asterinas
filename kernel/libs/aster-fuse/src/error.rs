// SPDX-License-Identifier: MPL-2.0

//! Defines error types used by [`crate::FuseOperation`] implementations.

/// Represents a local or remote error while encoding or decoding a FUSE message.
#[derive(Debug)]
pub enum FuseError {
    /// Indicates that the provided buffer cannot hold the requested bytes.
    BufferTooSmall,
    /// Indicates that a computed message length does not fit in the protocol field.
    LengthOverflow,
    /// Indicates that the remote peer replied with a negative Linux `errno` value.
    RemoteError(i32),
}

/// Represents the result type used by FUSE operation encoders and decoders.
pub type FuseResult<T> = core::result::Result<T, FuseError>;
