// SPDX-License-Identifier: MPL-2.0

//! `FUSE_LSEEK` computes a file offset for an open file handle.
//!
//! The request body contains [`LseekIn`] with the handle, base offset, and
//! seek mode. The reply body contains [`LseekOut`].

use core::mem::size_of;

use ostd::mm::{Infallible, VmReader, VmWriter};

use crate::{FuseError, FuseFileHandle, FuseOpcode, FuseOperation, FuseResult};

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod)]
pub struct LseekIn {
    pub fh: FuseFileHandle,
    /// Base offset for the seek operation.
    pub offset: i64,
    /// Determines how `offset` is interpreted when computing the new file position.
    pub whence: u32,
    pub padding: u32,
}

impl LseekIn {
    pub const fn new(fh: FuseFileHandle, offset: i64, whence: u32) -> Self {
        Self {
            fh,
            offset,
            whence,
            padding: 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod)]
pub struct LseekOut {
    pub offset: i64,
}

pub struct LseekOperation {
    lseek_in: LseekIn,
}

impl LseekOperation {
    pub fn new(lseek_in: LseekIn) -> Self {
        Self { lseek_in }
    }
}

impl FuseOperation for LseekOperation {
    type Output = LseekOut;

    fn opcode(&self) -> FuseOpcode {
        FuseOpcode::Lseek
    }

    fn body_len(&self) -> usize {
        size_of::<LseekIn>()
    }

    fn write_body(&mut self, writer: &mut VmWriter<'_, Infallible>) -> FuseResult<()> {
        writer
            .write_val(&self.lseek_in)
            .map_err(|_| FuseError::BufferTooSmall)
    }

    fn out_payload_size(&self) -> Option<usize> {
        Some(size_of::<LseekOut>())
    }

    fn parse_reply(
        self,
        _payload_len: usize,
        reader: &mut VmReader<'_, Infallible>,
    ) -> FuseResult<Self::Output> {
        reader.read_val().map_err(|_| FuseError::BufferTooSmall)
    }
}
