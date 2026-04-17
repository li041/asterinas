// SPDX-License-Identifier: MPL-2.0

//! `FUSE_SETATTR` updates selected attributes of an inode.
//!
//! The request body contains [`SetattrIn`], whose [`SetattrValid`] mask selects
//! the fields to apply. The reply body contains [`FuseAttrOut`] with the
//! updated attributes and their cache timeout.

use core::mem::size_of;

use bitflags::bitflags;
use ostd::mm::{Infallible, VmReader, VmWriter};

use crate::{FuseAttrOut, FuseError, FuseFileHandle, FuseOpcode, FuseOperation, FuseResult};

bitflags! {
    /// Selects which fields in [`SetattrIn`] are valid.
    #[repr(C)]
    #[derive(Pod, Default)]
    pub struct SetattrValid: u32 {
        const FATTR_MODE = 1 << 0;
        const FATTR_UID = 1 << 1;
        const FATTR_GID = 1 << 2;
        const FATTR_SIZE = 1 << 3;
        const FATTR_ATIME = 1 << 4;
        const FATTR_MTIME = 1 << 5;
        const FATTR_FH = 1 << 6;
        const FATTR_ATIME_NOW = 1 << 7;
        const FATTR_MTIME_NOW = 1 << 8;
        const FATTR_LOCKOWNER = 1 << 9;
        const FATTR_CTIME = 1 << 10;
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Default)]
pub struct SetattrIn {
    /// Bitmask selecting the attributes to update.
    pub valid: SetattrValid,
    pub padding: u32,
    /// File handle used when [`SetattrValid::FATTR_FH`] is set.
    pub fh: FuseFileHandle,
    /// New file size.
    pub size: u64,
    /// Lock owner used when [`SetattrValid::FATTR_LOCKOWNER`] is set.
    pub lock_owner: u64,
    /// New access time in seconds since the Unix epoch.
    pub atime: u64,
    /// New modification time in seconds since the Unix epoch.
    pub mtime: u64,
    /// New status-change time in seconds since the Unix epoch.
    pub ctime: u64,
    /// Nanosecond component of [`SetattrIn::atime`].
    pub atimensec: u32,
    /// Nanosecond component of [`SetattrIn::mtime`].
    pub mtimensec: u32,
    /// Nanosecond component of [`SetattrIn::ctime`].
    pub ctimensec: u32,
    /// New file type and permission bits.
    pub mode: u32,
    pub unused4: u32,
    /// New owner user ID.
    pub uid: u32,
    /// New owner group ID.
    pub gid: u32,
    pub unused5: u32,
}

/// Encodes a `FUSE_SETATTR` request.
pub struct SetattrOperation {
    setattr_in: SetattrIn,
}

impl SetattrOperation {
    /// Only the fields selected by [`SetattrIn::valid`] are applied;
    pub fn new(setattr_in: SetattrIn) -> Self {
        Self { setattr_in }
    }
}

impl FuseOperation for SetattrOperation {
    type Output = FuseAttrOut;

    fn opcode(&self) -> FuseOpcode {
        FuseOpcode::Setattr
    }

    fn body_len(&self) -> usize {
        size_of::<SetattrIn>()
    }

    fn write_body(&mut self, writer: &mut VmWriter<'_, Infallible>) -> FuseResult<()> {
        writer
            .write_val(&self.setattr_in)
            .map_err(|_| FuseError::BufferTooSmall)
    }

    fn out_payload_size(&self) -> Option<usize> {
        Some(size_of::<FuseAttrOut>())
    }

    fn parse_reply(
        self,
        _payload_len: usize,
        reader: &mut VmReader<'_, Infallible>,
    ) -> FuseResult<Self::Output> {
        reader.read_val().map_err(|_| FuseError::BufferTooSmall)
    }
}
