// SPDX-License-Identifier: MPL-2.0

//! `FUSE_LOOKUP` resolves a child name under a parent directory node.
//!
//! The request body contains only the null-terminated child name. The reply
//! body contains [`EntryOut`] for the resolved inode.

use core::mem::size_of;

use ostd::mm::{Infallible, VmReader, VmWriter};

use super::util::{NAME_TERMINATOR, name_body_len};
use crate::{EntryOut, FuseError, FuseOpcode, FuseOperation, FuseResult};

pub struct LookupOperation<'a> {
    name: &'a str,
}

impl<'a> LookupOperation<'a> {
    pub fn new(name: &'a str) -> Self {
        Self { name }
    }
}

impl FuseOperation for LookupOperation<'_> {
    type Output = EntryOut;

    fn opcode(&self) -> FuseOpcode {
        FuseOpcode::Lookup
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
        Some(size_of::<EntryOut>())
    }

    fn parse_reply(
        self,
        _payload_len: usize,
        reader: &mut VmReader<'_, Infallible>,
    ) -> FuseResult<Self::Output> {
        reader.read_val().map_err(|_| FuseError::BufferTooSmall)
    }
}
