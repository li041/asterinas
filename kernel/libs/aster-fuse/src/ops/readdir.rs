// SPDX-License-Identifier: MPL-2.0

//! `FUSE_READDIR` reads directory entries from an open directory handle.
//!
//! The request body reuses [`ReadIn`] with the directory handle, offset, and
//! maximum byte count. The reply body is a sequence of [`Dirent`] headers
//! followed by 8-byte-padded names, and the operation returns decoded
//! [`FuseDirEntry`] values.

use alloc::{string::ToString, vec::Vec};
use core::{cmp, mem::size_of};

use ostd::mm::{Infallible, VmReader, VmWriter};

use super::util::read_bytes;
use crate::{
    Dirent, DirentType, FuseDirEntry, FuseError, FuseOpcode, FuseOperation, FuseResult, ReadIn,
};

pub struct ReaddirOperation {
    read_in: ReadIn,
    size: usize,
}

impl ReaddirOperation {
    pub fn new(read_in: ReadIn, size: usize) -> Self {
        Self { read_in, size }
    }
}

impl FuseOperation for ReaddirOperation {
    type Output = Vec<FuseDirEntry>;

    fn opcode(&self) -> FuseOpcode {
        FuseOpcode::Readdir
    }

    fn body_len(&self) -> usize {
        size_of::<ReadIn>()
    }

    fn write_body(&mut self, writer: &mut VmWriter<'_, Infallible>) -> FuseResult<()> {
        writer
            .write_val(&self.read_in)
            .map_err(|_| FuseError::BufferTooSmall)
    }

    fn out_payload_size(&self) -> Option<usize> {
        Some(self.size)
    }

    fn parse_reply(
        self,
        payload_len: usize,
        reader: &mut VmReader<'_, Infallible>,
    ) -> FuseResult<Self::Output> {
        // Bound the reader to the actual dirent payload.
        let read_len = cmp::min(payload_len, self.size);
        reader.limit(read_len);

        // FUSE filenames are bounded by `FUSE_NAME_MAX` (1024 in current
        // protocol); a 1 KiB on-stack buffer covers any conforming reply.
        let mut name_buf = [0u8; 1024];
        let mut entries = Vec::new();
        let mut consumed = 0usize;

        while reader.remain() >= size_of::<Dirent>() {
            let header: Dirent = reader.read_val().map_err(|_| FuseError::BufferTooSmall)?;
            if header.namelen == 0 {
                break;
            }

            let namelen = header.namelen as usize;
            if namelen > name_buf.len() || namelen > reader.remain() {
                break;
            }
            read_bytes(reader, &mut name_buf[..namelen])?;

            if let Ok(name) = core::str::from_utf8(&name_buf[..namelen]) {
                entries.push(FuseDirEntry::new(
                    header.ino,
                    header.off,
                    DirentType::try_from(header.typ).unwrap_or(DirentType::DT_UNKNOWN),
                    name.to_string(),
                ));
            }

            // Each dirent is padded to 8-byte alignment.
            let dirent_len = size_of::<Dirent>() + namelen;
            let padded = (dirent_len + 7) & !7;
            let pad = padded - dirent_len;
            if pad > reader.remain() {
                break;
            }
            reader.skip(pad);

            consumed += padded;
            if consumed >= read_len {
                break;
            }
        }

        Ok(entries)
    }
}
