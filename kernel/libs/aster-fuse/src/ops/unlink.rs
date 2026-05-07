// SPDX-License-Identifier: MPL-2.0

//! `FUSE_UNLINK` removes a non-directory entry from parent directory.
//!
//! The request body contains only the null-terminated child name. Successful
//! replies do not carry a payload.

use ostd::mm::{Infallible, VmReader, VmWriter};

use super::util::{NAME_TERMINATOR, name_body_len};
use crate::{FuseError, FuseOpcode, FuseOperation, FuseResult};

pub struct UnlinkOperation<'a> {
    name: &'a str,
}

impl<'a> UnlinkOperation<'a> {
    pub fn new(name: &'a str) -> Self {
        Self { name }
    }
}

impl FuseOperation for UnlinkOperation<'_> {
    type Output = ();

    fn opcode(&self) -> FuseOpcode {
        FuseOpcode::Unlink
    }

    fn body_len(&self) -> usize {
        name_body_len(0, self.name)
    }

    fn write_body(&mut self, writer: &mut VmWriter<'_, Infallible>) -> FuseResult<()> {
        if writer.avail() < self.body_len() {
            return Err(FuseError::BufferTooSmall);
        }

        writer.write(&mut VmReader::from(self.name.as_bytes()));
        writer.write(&mut VmReader::from(NAME_TERMINATOR));

        Ok(())
    }

    fn out_payload_size(&self) -> Option<usize> {
        Some(0)
    }

    fn parse_reply(
        self,
        _payload_len: usize,
        _reader: &mut VmReader<'_, Infallible>,
    ) -> FuseResult<Self::Output> {
        Ok(())
    }
}
