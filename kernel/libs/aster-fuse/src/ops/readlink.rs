// SPDX-License-Identifier: MPL-2.0

//! `FUSE_READLINK` reads the target of a symbolic-link inode.
//!
//! The request body is empty because the target inode is named by the request
//! header. The reply body contains the symbolic-link target bytes, and the
//! operation returns them as a string without a trailing null byte.

use alloc::{
    string::{String, ToString},
    vec,
};

use ostd::mm::{Infallible, VmReader, VmWriter};

use super::util::read_bytes;
use crate::{FuseOpcode, FuseOperation, FuseResult};

const MAX_READLINK_LEN: usize = 4096;

pub struct ReadlinkOperation;

impl FuseOperation for ReadlinkOperation {
    type Output = String;

    fn opcode(&self) -> FuseOpcode {
        FuseOpcode::Readlink
    }

    fn write_body(&mut self, _writer: &mut VmWriter<'_, Infallible>) -> FuseResult<()> {
        Ok(())
    }

    fn out_payload_size(&self) -> Option<usize> {
        Some(MAX_READLINK_LEN)
    }

    fn parse_reply(
        self,
        payload_len: usize,
        reader: &mut VmReader<'_, Infallible>,
    ) -> FuseResult<Self::Output> {
        let mut buf = vec![0u8; payload_len];
        read_bytes(reader, &mut buf)?;
        let end = buf.iter().position(|&byte| byte == 0).unwrap_or(buf.len());
        Ok(String::from_utf8_lossy(&buf[..end]).to_string())
    }
}
