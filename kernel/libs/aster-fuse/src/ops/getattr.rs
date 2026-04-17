// SPDX-License-Identifier: MPL-2.0

//! `FUSE_GETATTR` reads attributes for an inode, optionally using an open file
//! handle carried in [`GetattrIn`].
//!
//! The reply body contains [`FuseAttrOut`], which includes the returned
//! attributes and their cache timeout.

use core::mem::size_of;

use bitflags::bitflags;
use ostd::mm::{Infallible, VmReader, VmWriter};

use crate::{Attr, FuseError, FuseFileHandle, FuseOpcode, FuseOperation, FuseResult};

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod)]
pub struct GetattrIn {
    pub getattr_flags: GetattrFlags,
    pub dummy: u32,
    /// File handle used when [`GetattrFlags::GETATTR_FH`] is set.
    pub fh: FuseFileHandle,
}

impl GetattrIn {
    pub const fn new(getattr_flags: GetattrFlags, fh: FuseFileHandle) -> Self {
        Self {
            getattr_flags,
            dummy: 0,
            fh,
        }
    }
}

bitflags! {
    /// Flags for `FUSE_GETATTR` requests.
    ///
    /// Reference: <https://elixir.bootlin.com/linux/v6.18/source/include/uapi/linux/fuse.h#L512-L515>
    #[repr(C)]
    #[derive(Pod)]
    pub struct GetattrFlags: u32 {
        /// The `fh` field is valid and identifies an open file.
        const GETATTR_FH = 1 << 0;
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod)]
pub struct FuseAttrOut {
    /// Attribute-cache timeout in seconds.
    pub attr_valid: u64,
    /// Nanosecond component of [`FuseAttrOut::attr_valid`].
    pub attr_valid_nsec: u32,
    pub dummy: u32,
    pub attr: Attr,
}

pub struct GetattrOperation {
    getattr_in: GetattrIn,
}

impl GetattrOperation {
    pub fn new(getattr_in: GetattrIn) -> Self {
        Self { getattr_in }
    }
}

impl FuseOperation for GetattrOperation {
    type Output = FuseAttrOut;

    fn opcode(&self) -> FuseOpcode {
        FuseOpcode::Getattr
    }

    fn body_len(&self) -> usize {
        size_of::<GetattrIn>()
    }

    fn write_body(&mut self, writer: &mut VmWriter<'_, Infallible>) -> FuseResult<()> {
        writer
            .write_val(&self.getattr_in)
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
