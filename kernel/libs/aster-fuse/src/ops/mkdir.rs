// SPDX-License-Identifier: MPL-2.0

//! `FUSE_MKDIR` creates a directory under a parent directory node.
//!
//! The request body contains [`MkdirIn`] followed by the null-terminated child
//! name. The reply body contains [`EntryOut`] for the created directory inode.

use core::mem::size_of;

use ostd::mm::{Infallible, VmReader, VmWriter};

use super::util::{NAME_TERMINATOR, name_body_len};
use crate::{EntryOut, FuseError, FuseOpcode, FuseOperation, FuseResult};

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod)]
pub struct MkdirIn {
    /// File type and permission bits for the new directory.
    mode: u32,
    /// Process umask of the requesting client, applied by the server when creating the inode.
    umask: u32,
}

impl MkdirIn {
    pub const fn new(mode: u32) -> Self {
        Self { mode, umask: 0 }
    }
}

pub struct MkdirOperation<'a> {
    mkdir_in: MkdirIn,
    name: &'a str,
}

impl<'a> MkdirOperation<'a> {
    pub fn new(mkdir_in: MkdirIn, name: &'a str) -> Self {
        Self { mkdir_in, name }
    }
}

impl FuseOperation for MkdirOperation<'_> {
    type Output = EntryOut;

    fn opcode(&self) -> FuseOpcode {
        FuseOpcode::Mkdir
    }

    fn body_len(&self) -> usize {
        name_body_len(size_of::<MkdirIn>(), self.name)
    }

    fn write_body(&mut self, writer: &mut VmWriter<'_, Infallible>) -> FuseResult<()> {
        if writer.avail() < self.body_len() {
            return Err(FuseError::BufferTooSmall);
        }

        writer.write_val(&self.mkdir_in).unwrap();
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
