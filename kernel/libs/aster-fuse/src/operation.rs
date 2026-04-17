// SPDX-License-Identifier: MPL-2.0

//! Defines the core trait for FUSE protocol operations.

use ostd::mm::{Infallible, VmReader, VmWriter};

use crate::{FuseOpcode, FuseResult};

/// A FUSE protocol operation with typed request and reply bodies.
///
/// Each implementer represents one request/reply pair defined by the FUSE protocol.
pub trait FuseOperation {
    /// Describes the successful reply produced by that operation.
    type Output;

    /// Returns the opcode identifying this operation's wire format.
    fn opcode(&self) -> FuseOpcode;

    /// Returns the request body length, excluding the `InHeader`.
    fn body_len(&self) -> usize {
        0
    }

    /// Writes the request body bytes into the transport buffer.
    ///
    /// The writer is positioned immediately after the `InHeader`.
    fn write_body(&mut self, _writer: &mut VmWriter<'_, Infallible>) -> FuseResult<()> {
        Ok(())
    }

    /// Returns the expected reply payload size.
    ///
    /// `None` means no reply is expected. `Some(0)` means a zero-
    /// length reply payload.
    fn out_payload_size(&self) -> Option<usize>;

    /// Parses the reply payload into [`Self::Output`].
    ///
    /// The reader is positioned at the start of the reply payload (after the
    /// `OutHeader`) and is limited to `payload_len` bytes.
    fn parse_reply(
        self,
        payload_len: usize,
        reader: &mut VmReader<'_, Infallible>,
    ) -> FuseResult<Self::Output>;
}
