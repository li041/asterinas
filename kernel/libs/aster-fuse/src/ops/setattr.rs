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
    valid: SetattrValid,
    padding: u32,
    /// File handle used when [`SetattrValid::FATTR_FH`] is set.
    fh: FuseFileHandle,
    /// New file size.
    size: u64,
    /// Lock owner used when [`SetattrValid::FATTR_LOCKOWNER`] is set.
    lock_owner: u64,
    /// New access time in seconds since the Unix epoch.
    atime: u64,
    /// New modification time in seconds since the Unix epoch.
    mtime: u64,
    /// New status-change time in seconds since the Unix epoch.
    ctime: u64,
    /// Nanosecond component of [`SetattrIn::atime`].
    atimensec: u32,
    /// Nanosecond component of [`SetattrIn::mtime`].
    mtimensec: u32,
    /// Nanosecond component of [`SetattrIn::ctime`].
    ctimensec: u32,
    /// New file type and permission bits.
    mode: u32,
    unused4: u32,
    /// New owner user ID.
    uid: u32,
    /// New owner group ID.
    gid: u32,
    unused5: u32,
}

impl SetattrIn {
    /// Creates a `SetattrIn` with the selected valid-field mask.
    pub const fn new(valid: SetattrValid) -> Self {
        Self {
            valid,
            padding: 0,
            fh: FuseFileHandle(0),
            size: 0,
            lock_owner: 0,
            atime: 0,
            mtime: 0,
            ctime: 0,
            atimensec: 0,
            mtimensec: 0,
            ctimensec: 0,
            mode: 0,
            unused4: 0,
            uid: 0,
            gid: 0,
            unused5: 0,
        }
    }

    /// Returns the bitmask selecting which attributes to update.
    pub fn valid(&self) -> SetattrValid {
        self.valid
    }

    /// Sets the file handle used when `FATTR_FH` is present.
    pub const fn with_fh(mut self, fh: FuseFileHandle) -> Self {
        self.fh = fh;
        self
    }

    /// Sets the new file size.
    pub const fn with_size(mut self, size: u64) -> Self {
        self.size = size;
        self
    }

    /// Sets the new file mode bits.
    pub const fn with_mode(mut self, mode: u32) -> Self {
        self.mode = mode;
        self
    }

    /// Sets the new owner user ID.
    pub const fn with_uid(mut self, uid: u32) -> Self {
        self.uid = uid;
        self
    }

    /// Sets the new owner group ID.
    pub const fn with_gid(mut self, gid: u32) -> Self {
        self.gid = gid;
        self
    }

    /// Sets the new access time.
    pub const fn with_atime(mut self, atime: u64, atimensec: u32) -> Self {
        self.atime = atime;
        self.atimensec = atimensec;
        self
    }

    /// Sets the new modification time.
    pub const fn with_mtime(mut self, mtime: u64, mtimensec: u32) -> Self {
        self.mtime = mtime;
        self.mtimensec = mtimensec;
        self
    }

    /// Sets the new status-change time.
    pub const fn with_ctime(mut self, ctime: u64, ctimensec: u32) -> Self {
        self.ctime = ctime;
        self.ctimensec = ctimensec;
        self
    }
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
