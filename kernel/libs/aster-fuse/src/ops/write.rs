// SPDX-License-Identifier: MPL-2.0

//! `FUSE_WRITE` writes bytes to an open file handle sitting at server-side.
//!
//! The request body contains [`WriteIn`] followed by the raw bytes to write.
//! The reply body contains [`WriteOut`], and the operation returns the number
//! of bytes accepted by the server.

use core::mem::size_of;

use bitflags::bitflags;
use ostd::mm::{FallibleVmRead, Infallible, VmReader, VmWriter};

use crate::{FuseError, FuseFileHandle, FuseOpcode, FuseOperation, FuseResult};

bitflags! {
    /// Flags for `FUSE_WRITE` requests.
    ///
    /// Reference: <https://elixir.bootlin.com/linux/v6.18/source/include/uapi/linux/fuse.h#L522-L531>
    #[repr(C)]
    #[derive(Pod)]
    pub struct WriteFlags: u32 {
        /// The write is serviced from the page cache (writeback mode).
        const WRITE_CACHE = 1 << 0;
        /// `lock_owner` is valid and should be used for lock-aware writes.
        const WRITE_LOCKOWNER = 1 << 1;
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod)]
pub struct WriteIn {
    /// File handle to write to.
    fh: FuseFileHandle,
    /// File offset to start writing at.
    offset: u64,
    /// Number of bytes to write.
    size: u32,
    /// FUSE-specific write flags.
    write_flags: WriteFlags,
    /// Lock owner for lock-aware writes.
    lock_owner: u64,
    /// POSIX open flags associated with the handle.
    flags: u32,
    padding: u32,
}

impl WriteIn {
    pub const fn new(
        fh: FuseFileHandle,
        offset: u64,
        size: u32,
        flags: u32,
        write_flags: WriteFlags,
    ) -> Self {
        Self {
            fh,
            offset,
            size,
            write_flags,
            lock_owner: 0,
            flags,
            padding: 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod)]
pub struct WriteOut {
    /// Number of bytes written by the server.
    size: u32,
    padding: u32,
}

impl WriteOut {
    /// Returns the number of bytes written by the server.
    pub fn size(&self) -> u32 {
        self.size
    }
}

pub struct WriteOperation<'a, 'b> {
    write_in: WriteIn,
    reader: &'a mut VmReader<'b>,
}

impl<'a, 'b> WriteOperation<'a, 'b> {
    pub fn new(write_in: WriteIn, reader: &'a mut VmReader<'b>) -> Self {
        Self { write_in, reader }
    }
}

impl FuseOperation for WriteOperation<'_, '_> {
    type Output = usize;

    fn opcode(&self) -> FuseOpcode {
        FuseOpcode::Write
    }

    fn body_len(&self) -> usize {
        size_of::<WriteIn>().saturating_add(self.reader.remain())
    }

    fn write_body(&mut self, writer: &mut VmWriter<'_, Infallible>) -> FuseResult<()> {
        if writer.avail() < self.body_len() {
            return Err(FuseError::BufferTooSmall);
        }

        writer.write_val(&self.write_in).unwrap();

        let mut new_reader = self.reader.clone();

        let bytes_written = new_reader
            .read_fallible(writer)
            .map_err(|_| FuseError::PageFault)?;

        self.reader.skip(bytes_written);

        Ok(())
    }

    fn out_payload_size(&self) -> Option<usize> {
        Some(size_of::<WriteOut>())
    }

    fn parse_reply(
        self,
        _payload_len: usize,
        reader: &mut VmReader<'_, Infallible>,
    ) -> FuseResult<Self::Output> {
        let write_out: WriteOut = reader.read_val().map_err(|_| FuseError::PageFault)?;
        Ok(write_out.size() as usize)
    }
}
