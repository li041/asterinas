// SPDX-License-Identifier: MPL-2.0

use crate::{FuseOpcode, FuseResult};

/// Describes a typed FUSE request/response pair.
///
/// Implementers specify the opcode, request body, and reply type for one FUSE
/// operation.
///
/// The trait uses closures for request-body writes and reply-payload reads
/// instead of exposing a concrete buffer type. This keeps operation
/// implementations focused on protocol encoding and decoding while leaving
/// buffer management to the caller.
pub trait FuseOperation {
    type Output;

    fn opcode(&self) -> FuseOpcode;

    fn nodeid(&self) -> u64;

    /// Returns the request body length, excluding the `InHeader`.
    fn body_len(&self) -> usize {
        0
    }

    /// Writes the request body bytes.
    fn write_body(
        &self,
        _write_bytes_fn: &mut dyn FnMut(&[u8]) -> FuseResult<()>,
    ) -> FuseResult<()> {
        Ok(())
    }

    /// Returns the expected reply payload size, or `None` if no reply payload is expected.
    fn out_payload_size(&self) -> Option<usize>;

    /// Parses the reply payload into [`Self::Output`].
    fn parse_reply(
        self,
        payload_len: usize,
        read_bytes_fn: &mut dyn FnMut(usize, &mut [u8]) -> FuseResult<()>,
    ) -> FuseResult<Self::Output>;
}
