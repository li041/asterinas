// SPDX-License-Identifier: MPL-2.0

//! `FUSE_MKNOD` creates a special node under the parent directory.
//!
//! The request body contains [`MknodIn`] followed by the null-terminated child
//! name. The reply body contains [`EntryOut`] for the created inode.

use core::mem::size_of;

use ostd::mm::{Infallible, VmReader, VmWriter};

use super::util::{NAME_TERMINATOR, name_body_len};
use crate::{EntryOut, FuseError, FuseOpcode, FuseOperation, FuseResult};

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod)]
pub struct MknodIn {
    /// File type and permission bits for the new inode.
    mode: u32,
    /// Device number for special files.
    rdev: u32,
    /// Process umask of the requesting client, applied by the server when creating the inode.
    umask: u32,
    padding: u32,
}

impl MknodIn {
    pub const fn new(mode: u32, rdev: u32) -> Self {
        Self {
            mode,
            rdev,
            umask: 0,
            padding: 0,
        }
    }
}

pub struct MknodOperation<'a> {
    mknod_in: MknodIn,
    name: &'a str,
}

impl<'a> MknodOperation<'a> {
    pub fn new(mknod_in: MknodIn, name: &'a str) -> Self {
        Self { mknod_in, name }
    }
}

impl FuseOperation for MknodOperation<'_> {
    type Output = EntryOut;

    fn opcode(&self) -> FuseOpcode {
        FuseOpcode::Mknod
    }

    fn body_len(&self) -> usize {
        name_body_len(size_of::<MknodIn>(), self.name)
    }

    fn write_body(&mut self, writer: &mut VmWriter<'_, Infallible>) -> FuseResult<()> {
        if writer.avail() < self.body_len() {
            return Err(FuseError::BufferTooSmall);
        }

        writer.write_val(&self.mknod_in).unwrap();
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
