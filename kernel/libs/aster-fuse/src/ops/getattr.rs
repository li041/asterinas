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
    getattr_flags: GetattrFlags,
    dummy: u32,
    /// File handle used when [`GetattrFlags::GETATTR_FH`] is set.
    fh: FuseFileHandle,
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
    attr_valid: u64,
    /// Nanosecond component of [`FuseAttrOut::attr_valid`].
    attr_valid_nsec: u32,
    dummy: u32,
    attr: Attr,
}

impl FuseAttrOut {
    /// Creates a `FuseAttrOut` from the returned attributes and timeout.
    pub const fn new(attr_valid: u64, attr_valid_nsec: u32, attr: Attr) -> Self {
        Self {
            attr_valid,
            attr_valid_nsec,
            dummy: 0,
            attr,
        }
    }

    /// Returns the attribute-cache timeout in seconds.
    pub fn attr_valid(&self) -> u64 {
        self.attr_valid
    }

    /// Returns the nanosecond component of the attribute-cache timeout.
    pub fn attr_valid_nsec(&self) -> u32 {
        self.attr_valid_nsec
    }

    /// Returns the returned inode attributes.
    pub fn attr(&self) -> Attr {
        self.attr
    }
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
