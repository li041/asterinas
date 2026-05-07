// SPDX-License-Identifier: MPL-2.0

//! `FUSE_LINK` creates a hard link to an existing inode in a target directory.
//!
//! The request body contains [`LinkIn`] followed by the null-terminated new
//! name under the target parent directory node. The reply body contains
//! [`EntryOut`] for the linked inode.

use core::mem::size_of;

use ostd::mm::{Infallible, VmReader, VmWriter};

use super::util::{NAME_TERMINATOR, name_body_len};
use crate::{EntryOut, FuseError, FuseNodeId, FuseOpcode, FuseOperation, FuseResult};

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod)]
pub struct LinkIn {
    /// Existing inode that will receive the new hard link.
    oldnodeid: FuseNodeId,
}

impl LinkIn {
    pub const fn new(oldnodeid: FuseNodeId) -> Self {
        Self { oldnodeid }
    }
}

pub struct LinkOperation<'a> {
    link_in: LinkIn,
    new_name: &'a str,
}

impl<'a> LinkOperation<'a> {
    pub fn new(link_in: LinkIn, new_name: &'a str) -> Self {
        Self { link_in, new_name }
    }
}

impl FuseOperation for LinkOperation<'_> {
    type Output = EntryOut;

    fn opcode(&self) -> FuseOpcode {
        FuseOpcode::Link
    }

    fn body_len(&self) -> usize {
        name_body_len(size_of::<LinkIn>(), self.new_name)
    }

    fn write_body(&mut self, writer: &mut VmWriter<'_, Infallible>) -> FuseResult<()> {
        if writer.avail() < self.body_len() {
            return Err(FuseError::BufferTooSmall);
        }

        writer.write_val(&self.link_in).unwrap();
        writer.write(&mut VmReader::from(self.new_name.as_bytes()));
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
