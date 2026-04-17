// SPDX-License-Identifier: MPL-2.0

//! Per-opcode [`FuseOperation`] implementations.
//!
//! Each struct encodes one FUSE request type: how to serialize the body into
//! the in-buffer and how to deserialize the reply from the out-buffer.

use alloc::{
    string::{String, ToString},
    vec,
    vec::Vec,
};
use core::{cmp, mem::size_of};

use ostd_pod::{IntoBytes, Pod};

use super::*;

const NAME_TERMINATOR: &[u8] = &[0];

pub(super) struct InitOperation {
    init_in: InitIn,
}

impl InitOperation {
    pub(super) fn new(init_in: InitIn) -> Self {
        Self { init_in }
    }
}

impl FuseOperation for InitOperation {
    type Output = InitOut;

    fn opcode(&self) -> FuseOpcode {
        FuseOpcode::Init
    }

    fn nodeid(&self) -> u64 {
        0
    }

    fn body_len(&self) -> usize {
        size_of::<InitIn>()
    }

    fn write_body(
        &self,
        write_bytes_fn: &mut dyn FnMut(&[u8]) -> FuseResult<()>,
    ) -> FuseResult<()> {
        write_bytes_fn(self.init_in.as_bytes())
    }

    fn out_payload_size(&self) -> Option<usize> {
        Some(size_of::<InitOut>())
    }

    fn parse_reply(
        self,
        _payload_len: usize,
        read_bytes_fn: &mut dyn FnMut(usize, &mut [u8]) -> FuseResult<()>,
    ) -> FuseResult<Self::Output> {
        read_body(0, read_bytes_fn)
    }
}

pub(super) struct LookupOperation<'a> {
    parent_nodeid: u64,
    name: &'a str,
}

impl<'a> LookupOperation<'a> {
    pub(super) fn new(parent_nodeid: u64, name: &'a str) -> Self {
        Self {
            parent_nodeid,
            name,
        }
    }
}

impl FuseOperation for LookupOperation<'_> {
    type Output = EntryOut;

    fn opcode(&self) -> FuseOpcode {
        FuseOpcode::Lookup
    }

    fn nodeid(&self) -> u64 {
        self.parent_nodeid
    }

    fn body_len(&self) -> usize {
        name_body_len(0, self.name)
    }

    fn write_body(
        &self,
        write_bytes_fn: &mut dyn FnMut(&[u8]) -> FuseResult<()>,
    ) -> FuseResult<()> {
        write_bytes_fn(self.name.as_bytes())?;
        write_bytes_fn(NAME_TERMINATOR)
    }

    fn out_payload_size(&self) -> Option<usize> {
        Some(size_of::<EntryOut>())
    }

    fn parse_reply(
        self,
        _payload_len: usize,
        read_bytes_fn: &mut dyn FnMut(usize, &mut [u8]) -> FuseResult<()>,
    ) -> FuseResult<Self::Output> {
        read_body(0, read_bytes_fn)
    }
}

pub(super) struct MkdirOperation<'a> {
    parent_nodeid: u64,
    mkdir_in: MkdirIn,
    name: &'a str,
}

impl<'a> MkdirOperation<'a> {
    pub(super) fn new(parent_nodeid: u64, mkdir_in: MkdirIn, name: &'a str) -> Self {
        Self {
            parent_nodeid,
            mkdir_in,
            name,
        }
    }
}

impl FuseOperation for MkdirOperation<'_> {
    type Output = EntryOut;

    fn opcode(&self) -> FuseOpcode {
        FuseOpcode::Mkdir
    }

    fn nodeid(&self) -> u64 {
        self.parent_nodeid
    }

    fn body_len(&self) -> usize {
        name_body_len(size_of::<MkdirIn>(), self.name)
    }

    fn write_body(
        &self,
        write_bytes_fn: &mut dyn FnMut(&[u8]) -> FuseResult<()>,
    ) -> FuseResult<()> {
        write_bytes_fn(self.mkdir_in.as_bytes())?;
        write_bytes_fn(self.name.as_bytes())?;
        write_bytes_fn(NAME_TERMINATOR)
    }

    fn out_payload_size(&self) -> Option<usize> {
        Some(size_of::<EntryOut>())
    }

    fn parse_reply(
        self,
        _payload_len: usize,
        read_bytes_fn: &mut dyn FnMut(usize, &mut [u8]) -> FuseResult<()>,
    ) -> FuseResult<Self::Output> {
        read_body(0, read_bytes_fn)
    }
}

pub(super) struct MknodOperation<'a> {
    parent_nodeid: u64,
    mknod_in: MknodIn,
    name: &'a str,
}

impl<'a> MknodOperation<'a> {
    pub(super) fn new(parent_nodeid: u64, mknod_in: MknodIn, name: &'a str) -> Self {
        Self {
            parent_nodeid,
            mknod_in,
            name,
        }
    }
}

impl FuseOperation for MknodOperation<'_> {
    type Output = EntryOut;

    fn opcode(&self) -> FuseOpcode {
        FuseOpcode::Mknod
    }

    fn nodeid(&self) -> u64 {
        self.parent_nodeid
    }

    fn body_len(&self) -> usize {
        name_body_len(size_of::<MknodIn>(), self.name)
    }

    fn write_body(
        &self,
        write_bytes_fn: &mut dyn FnMut(&[u8]) -> FuseResult<()>,
    ) -> FuseResult<()> {
        write_bytes_fn(self.mknod_in.as_bytes())?;
        write_bytes_fn(self.name.as_bytes())?;
        write_bytes_fn(NAME_TERMINATOR)
    }

    fn out_payload_size(&self) -> Option<usize> {
        Some(size_of::<EntryOut>())
    }

    fn parse_reply(
        self,
        _payload_len: usize,
        read_bytes_fn: &mut dyn FnMut(usize, &mut [u8]) -> FuseResult<()>,
    ) -> FuseResult<Self::Output> {
        read_body(0, read_bytes_fn)
    }
}

pub(super) struct UnlinkOperation<'a> {
    parent_nodeid: u64,
    name: &'a str,
}

impl<'a> UnlinkOperation<'a> {
    pub(super) fn new(parent_nodeid: u64, name: &'a str) -> Self {
        Self {
            parent_nodeid,
            name,
        }
    }
}

impl FuseOperation for UnlinkOperation<'_> {
    type Output = ();

    fn opcode(&self) -> FuseOpcode {
        FuseOpcode::Unlink
    }

    fn nodeid(&self) -> u64 {
        self.parent_nodeid
    }

    fn body_len(&self) -> usize {
        name_body_len(0, self.name)
    }

    fn write_body(
        &self,
        write_bytes_fn: &mut dyn FnMut(&[u8]) -> FuseResult<()>,
    ) -> FuseResult<()> {
        write_bytes_fn(self.name.as_bytes())?;
        write_bytes_fn(NAME_TERMINATOR)
    }

    fn out_payload_size(&self) -> Option<usize> {
        Some(0)
    }

    fn parse_reply(
        self,
        _payload_len: usize,
        _read_bytes_fn: &mut dyn FnMut(usize, &mut [u8]) -> FuseResult<()>,
    ) -> FuseResult<Self::Output> {
        Ok(())
    }
}

pub(super) struct RmdirOperation<'a> {
    parent_nodeid: u64,
    name: &'a str,
}

impl<'a> RmdirOperation<'a> {
    pub(super) fn new(parent_nodeid: u64, name: &'a str) -> Self {
        Self {
            parent_nodeid,
            name,
        }
    }
}

impl FuseOperation for RmdirOperation<'_> {
    type Output = ();

    fn opcode(&self) -> FuseOpcode {
        FuseOpcode::Rmdir
    }

    fn nodeid(&self) -> u64 {
        self.parent_nodeid
    }

    fn body_len(&self) -> usize {
        name_body_len(0, self.name)
    }

    fn write_body(
        &self,
        write_bytes_fn: &mut dyn FnMut(&[u8]) -> FuseResult<()>,
    ) -> FuseResult<()> {
        write_bytes_fn(self.name.as_bytes())?;
        write_bytes_fn(NAME_TERMINATOR)
    }

    fn out_payload_size(&self) -> Option<usize> {
        Some(0)
    }

    fn parse_reply(
        self,
        _payload_len: usize,
        _read_bytes_fn: &mut dyn FnMut(usize, &mut [u8]) -> FuseResult<()>,
    ) -> FuseResult<Self::Output> {
        Ok(())
    }
}

pub(super) struct CreateOperation<'a> {
    parent_nodeid: u64,
    create_in: CreateIn,
    name: &'a str,
}

impl<'a> CreateOperation<'a> {
    pub(super) fn new(parent_nodeid: u64, create_in: CreateIn, name: &'a str) -> Self {
        Self {
            parent_nodeid,
            create_in,
            name,
        }
    }
}

impl FuseOperation for CreateOperation<'_> {
    type Output = (EntryOut, OpenOut);

    fn opcode(&self) -> FuseOpcode {
        FuseOpcode::Create
    }

    fn nodeid(&self) -> u64 {
        self.parent_nodeid
    }

    fn body_len(&self) -> usize {
        name_body_len(size_of::<CreateIn>(), self.name)
    }

    fn write_body(
        &self,
        write_bytes_fn: &mut dyn FnMut(&[u8]) -> FuseResult<()>,
    ) -> FuseResult<()> {
        write_bytes_fn(self.create_in.as_bytes())?;
        write_bytes_fn(self.name.as_bytes())?;
        write_bytes_fn(NAME_TERMINATOR)
    }

    fn out_payload_size(&self) -> Option<usize> {
        Some(size_of::<EntryOut>() + size_of::<OpenOut>())
    }

    fn parse_reply(
        self,
        _payload_len: usize,
        read_bytes_fn: &mut dyn FnMut(usize, &mut [u8]) -> FuseResult<()>,
    ) -> FuseResult<Self::Output> {
        let entry_out = read_body(0, read_bytes_fn)?;
        let open_out = read_body(size_of::<EntryOut>(), read_bytes_fn)?;
        Ok((entry_out, open_out))
    }
}

pub(super) struct GetattrOperation {
    nodeid: u64,
    getattr_in: GetattrIn,
}

impl GetattrOperation {
    pub(super) fn new(nodeid: u64, getattr_in: GetattrIn) -> Self {
        Self { nodeid, getattr_in }
    }
}

impl FuseOperation for GetattrOperation {
    type Output = FuseAttrOut;

    fn opcode(&self) -> FuseOpcode {
        FuseOpcode::Getattr
    }

    fn nodeid(&self) -> u64 {
        self.nodeid
    }

    fn body_len(&self) -> usize {
        size_of::<GetattrIn>()
    }

    fn write_body(
        &self,
        write_bytes_fn: &mut dyn FnMut(&[u8]) -> FuseResult<()>,
    ) -> FuseResult<()> {
        write_bytes_fn(self.getattr_in.as_bytes())
    }

    fn out_payload_size(&self) -> Option<usize> {
        Some(size_of::<FuseAttrOut>())
    }

    fn parse_reply(
        self,
        _payload_len: usize,
        read_bytes_fn: &mut dyn FnMut(usize, &mut [u8]) -> FuseResult<()>,
    ) -> FuseResult<Self::Output> {
        read_body(0, read_bytes_fn)
    }
}

pub(super) struct SetattrOperation {
    nodeid: u64,
    setattr_in: SetattrIn,
}

impl SetattrOperation {
    pub(super) fn new(nodeid: u64, setattr_in: SetattrIn) -> Self {
        Self { nodeid, setattr_in }
    }
}

impl FuseOperation for SetattrOperation {
    type Output = FuseAttrOut;

    fn opcode(&self) -> FuseOpcode {
        FuseOpcode::Setattr
    }

    fn nodeid(&self) -> u64 {
        self.nodeid
    }

    fn body_len(&self) -> usize {
        size_of::<SetattrIn>()
    }

    fn write_body(
        &self,
        write_bytes_fn: &mut dyn FnMut(&[u8]) -> FuseResult<()>,
    ) -> FuseResult<()> {
        write_bytes_fn(self.setattr_in.as_bytes())
    }

    fn out_payload_size(&self) -> Option<usize> {
        Some(size_of::<FuseAttrOut>())
    }

    fn parse_reply(
        self,
        _payload_len: usize,
        read_bytes_fn: &mut dyn FnMut(usize, &mut [u8]) -> FuseResult<()>,
    ) -> FuseResult<Self::Output> {
        read_body(0, read_bytes_fn)
    }
}

pub(super) struct OpendirOperation {
    nodeid: u64,
    open_in: OpenIn,
}

impl OpendirOperation {
    pub(super) fn new(nodeid: u64, open_in: OpenIn) -> Self {
        Self { nodeid, open_in }
    }
}

impl FuseOperation for OpendirOperation {
    type Output = FuseFileHandle;

    fn opcode(&self) -> FuseOpcode {
        FuseOpcode::Opendir
    }

    fn nodeid(&self) -> u64 {
        self.nodeid
    }

    fn body_len(&self) -> usize {
        size_of::<OpenIn>()
    }

    fn write_body(
        &self,
        write_bytes_fn: &mut dyn FnMut(&[u8]) -> FuseResult<()>,
    ) -> FuseResult<()> {
        write_bytes_fn(self.open_in.as_bytes())
    }

    fn out_payload_size(&self) -> Option<usize> {
        Some(size_of::<OpenOut>())
    }

    fn parse_reply(
        self,
        _payload_len: usize,
        read_bytes_fn: &mut dyn FnMut(usize, &mut [u8]) -> FuseResult<()>,
    ) -> FuseResult<Self::Output> {
        let open_out: OpenOut = read_body(0, read_bytes_fn)?;
        Ok(open_out.fh)
    }
}

pub(super) struct ReaddirOperation {
    nodeid: u64,
    read_in: ReadIn,
    size: usize,
}

impl ReaddirOperation {
    pub(super) fn new(nodeid: u64, read_in: ReadIn, size: usize) -> Self {
        Self {
            nodeid,
            read_in,
            size,
        }
    }
}

impl FuseOperation for ReaddirOperation {
    type Output = Vec<VirtioFsDirEntry>;

    fn opcode(&self) -> FuseOpcode {
        FuseOpcode::Readdir
    }

    fn nodeid(&self) -> u64 {
        self.nodeid
    }

    fn body_len(&self) -> usize {
        size_of::<ReadIn>()
    }

    fn write_body(
        &self,
        write_bytes_fn: &mut dyn FnMut(&[u8]) -> FuseResult<()>,
    ) -> FuseResult<()> {
        write_bytes_fn(self.read_in.as_bytes())
    }

    fn out_payload_size(&self) -> Option<usize> {
        Some(self.size)
    }

    fn parse_reply(
        self,
        payload_len: usize,
        read_bytes_fn: &mut dyn FnMut(usize, &mut [u8]) -> FuseResult<()>,
    ) -> FuseResult<Self::Output> {
        let read_len = cmp::min(payload_len, self.size);
        let mut buf = vec![0u8; read_len];
        read_bytes_fn(0, &mut buf)?;

        let mut entries = Vec::new();
        let mut pos = 0usize;
        while pos + size_of::<Dirent>() <= read_len {
            let header = Dirent::from_bytes(&buf[pos..pos + size_of::<Dirent>()]);
            if header.namelen == 0 {
                break;
            }

            let name_start = pos + size_of::<Dirent>();
            let name_end = name_start + header.namelen as usize;
            if name_end > read_len {
                break;
            }

            if let Ok(name) = core::str::from_utf8(&buf[name_start..name_end]) {
                entries.push(VirtioFsDirEntry {
                    ino: header.ino,
                    offset: header.off,
                    type_: DirentType::try_from(header.typ).unwrap_or(DirentType::DT_UNKNOWN),
                    name: name.to_string(),
                });
            }

            let dirent_len = size_of::<Dirent>() + header.namelen as usize;
            pos += (dirent_len + 7) & !7;
        }

        Ok(entries)
    }
}

impl ReleaseKind {
    fn opcode(self) -> FuseOpcode {
        match self {
            Self::File => FuseOpcode::Release,
            Self::Directory => FuseOpcode::Releasedir,
        }
    }
}

pub(super) struct ReadlinkOperation {
    nodeid: u64,
}

impl ReadlinkOperation {
    pub(super) fn new(nodeid: u64) -> Self {
        Self { nodeid }
    }
}

impl FuseOperation for ReadlinkOperation {
    type Output = String;

    fn opcode(&self) -> FuseOpcode {
        FuseOpcode::Readlink
    }

    fn nodeid(&self) -> u64 {
        self.nodeid
    }

    fn out_payload_size(&self) -> Option<usize> {
        Some(4096)
    }

    fn parse_reply(
        self,
        payload_len: usize,
        read_bytes_fn: &mut dyn FnMut(usize, &mut [u8]) -> FuseResult<()>,
    ) -> FuseResult<Self::Output> {
        let mut buf = vec![0u8; payload_len];
        read_bytes_fn(0, &mut buf)?;
        let end = buf.iter().position(|&byte| byte == 0).unwrap_or(buf.len());
        Ok(String::from_utf8_lossy(&buf[..end]).to_string())
    }
}

pub(super) struct LinkOperation<'a> {
    link_in: LinkIn,
    new_parent_nodeid: u64,
    new_name: &'a str,
}

impl<'a> LinkOperation<'a> {
    pub(super) fn new(link_in: LinkIn, new_parent_nodeid: u64, new_name: &'a str) -> Self {
        Self {
            link_in,
            new_parent_nodeid,
            new_name,
        }
    }
}

impl FuseOperation for LinkOperation<'_> {
    type Output = EntryOut;

    fn opcode(&self) -> FuseOpcode {
        FuseOpcode::Link
    }

    fn nodeid(&self) -> u64 {
        self.new_parent_nodeid
    }

    fn body_len(&self) -> usize {
        name_body_len(size_of::<LinkIn>(), self.new_name)
    }

    fn write_body(
        &self,
        write_bytes_fn: &mut dyn FnMut(&[u8]) -> FuseResult<()>,
    ) -> FuseResult<()> {
        write_bytes_fn(self.link_in.as_bytes())?;
        write_bytes_fn(self.new_name.as_bytes())?;
        write_bytes_fn(NAME_TERMINATOR)
    }

    fn out_payload_size(&self) -> Option<usize> {
        Some(size_of::<EntryOut>())
    }

    fn parse_reply(
        self,
        _payload_len: usize,
        read_bytes_fn: &mut dyn FnMut(usize, &mut [u8]) -> FuseResult<()>,
    ) -> FuseResult<Self::Output> {
        read_body(0, read_bytes_fn)
    }
}

pub(super) struct OpenOperation {
    nodeid: u64,
    open_in: OpenIn,
}

impl OpenOperation {
    pub(super) fn new(nodeid: u64, open_in: OpenIn) -> Self {
        Self { nodeid, open_in }
    }
}

impl FuseOperation for OpenOperation {
    type Output = OpenOut;

    fn opcode(&self) -> FuseOpcode {
        FuseOpcode::Open
    }

    fn nodeid(&self) -> u64 {
        self.nodeid
    }

    fn body_len(&self) -> usize {
        size_of::<OpenIn>()
    }

    fn write_body(
        &self,
        write_bytes_fn: &mut dyn FnMut(&[u8]) -> FuseResult<()>,
    ) -> FuseResult<()> {
        write_bytes_fn(self.open_in.as_bytes())
    }

    fn out_payload_size(&self) -> Option<usize> {
        Some(size_of::<OpenOut>())
    }

    fn parse_reply(
        self,
        _payload_len: usize,
        read_bytes_fn: &mut dyn FnMut(usize, &mut [u8]) -> FuseResult<()>,
    ) -> FuseResult<Self::Output> {
        read_body(0, read_bytes_fn)
    }
}

pub(super) struct ReleaseOperation {
    nodeid: u64,
    release_in: ReleaseIn,
    kind: ReleaseKind,
}

impl ReleaseOperation {
    pub(super) fn new(nodeid: u64, release_in: ReleaseIn, kind: ReleaseKind) -> Self {
        Self {
            nodeid,
            release_in,
            kind,
        }
    }
}

impl FuseOperation for ReleaseOperation {
    type Output = ();

    fn opcode(&self) -> FuseOpcode {
        self.kind.opcode()
    }

    fn nodeid(&self) -> u64 {
        self.nodeid
    }

    fn body_len(&self) -> usize {
        size_of::<ReleaseIn>()
    }

    fn write_body(
        &self,
        write_bytes_fn: &mut dyn FnMut(&[u8]) -> FuseResult<()>,
    ) -> FuseResult<()> {
        write_bytes_fn(self.release_in.as_bytes())
    }

    fn out_payload_size(&self) -> Option<usize> {
        Some(0)
    }

    fn parse_reply(
        self,
        _payload_len: usize,
        _read_bytes_fn: &mut dyn FnMut(usize, &mut [u8]) -> FuseResult<()>,
    ) -> FuseResult<Self::Output> {
        Ok(())
    }
}

pub(super) struct LseekOperation {
    nodeid: u64,
    lseek_in: LseekIn,
}

impl LseekOperation {
    pub(super) fn new(nodeid: u64, lseek_in: LseekIn) -> Self {
        Self { nodeid, lseek_in }
    }
}

impl FuseOperation for LseekOperation {
    type Output = i64;

    fn opcode(&self) -> FuseOpcode {
        FuseOpcode::Lseek
    }

    fn nodeid(&self) -> u64 {
        self.nodeid
    }

    fn body_len(&self) -> usize {
        size_of::<LseekIn>()
    }

    fn write_body(
        &self,
        write_bytes_fn: &mut dyn FnMut(&[u8]) -> FuseResult<()>,
    ) -> FuseResult<()> {
        write_bytes_fn(self.lseek_in.as_bytes())
    }

    fn out_payload_size(&self) -> Option<usize> {
        Some(size_of::<LseekOut>())
    }

    fn parse_reply(
        self,
        _payload_len: usize,
        read_bytes_fn: &mut dyn FnMut(usize, &mut [u8]) -> FuseResult<()>,
    ) -> FuseResult<Self::Output> {
        let lseek_out: LseekOut = read_body(0, read_bytes_fn)?;
        Ok(lseek_out.offset)
    }
}

pub(super) struct ReadOperation {
    nodeid: u64,
    read_in: ReadIn,
    size: usize,
}

impl ReadOperation {
    pub(super) fn new(nodeid: u64, read_in: ReadIn, size: usize) -> Self {
        Self {
            nodeid,
            read_in,
            size,
        }
    }
}

impl FuseOperation for ReadOperation {
    type Output = Vec<u8>;

    fn opcode(&self) -> FuseOpcode {
        FuseOpcode::Read
    }

    fn nodeid(&self) -> u64 {
        self.nodeid
    }

    fn body_len(&self) -> usize {
        size_of::<ReadIn>()
    }

    fn write_body(
        &self,
        write_bytes_fn: &mut dyn FnMut(&[u8]) -> FuseResult<()>,
    ) -> FuseResult<()> {
        write_bytes_fn(self.read_in.as_bytes())
    }

    fn out_payload_size(&self) -> Option<usize> {
        Some(self.size)
    }

    fn parse_reply(
        self,
        payload_len: usize,
        read_bytes_fn: &mut dyn FnMut(usize, &mut [u8]) -> FuseResult<()>,
    ) -> FuseResult<Self::Output> {
        let read_len = cmp::min(payload_len, self.size);
        let mut buf = vec![0u8; read_len];
        read_bytes_fn(0, &mut buf)?;
        Ok(buf)
    }
}

pub(super) struct WriteOperation<'a> {
    nodeid: u64,
    write_in: WriteIn,
    data: &'a [u8],
}

impl<'a> WriteOperation<'a> {
    pub(super) fn new(nodeid: u64, write_in: WriteIn, data: &'a [u8]) -> Self {
        Self {
            nodeid,
            write_in,
            data,
        }
    }
}

impl FuseOperation for WriteOperation<'_> {
    type Output = usize;

    fn opcode(&self) -> FuseOpcode {
        FuseOpcode::Write
    }

    fn nodeid(&self) -> u64 {
        self.nodeid
    }

    fn body_len(&self) -> usize {
        size_of::<WriteIn>().saturating_add(self.data.len())
    }

    fn write_body(
        &self,
        write_bytes_fn: &mut dyn FnMut(&[u8]) -> FuseResult<()>,
    ) -> FuseResult<()> {
        write_bytes_fn(self.write_in.as_bytes())?;
        write_bytes_fn(self.data)
    }

    fn out_payload_size(&self) -> Option<usize> {
        Some(size_of::<WriteOut>())
    }

    fn parse_reply(
        self,
        _payload_len: usize,
        read_bytes_fn: &mut dyn FnMut(usize, &mut [u8]) -> FuseResult<()>,
    ) -> FuseResult<Self::Output> {
        let write_out: WriteOut = read_body(0, read_bytes_fn)?;
        Ok(write_out.size as usize)
    }
}

pub(super) struct ForgetOperation {
    nodeid: u64,
    forget_in: ForgetIn,
}

impl ForgetOperation {
    pub(super) fn new(nodeid: u64, forget_in: ForgetIn) -> Self {
        Self { nodeid, forget_in }
    }
}

impl FuseOperation for ForgetOperation {
    type Output = ();

    fn opcode(&self) -> FuseOpcode {
        FuseOpcode::Forget
    }

    fn nodeid(&self) -> u64 {
        self.nodeid
    }

    fn body_len(&self) -> usize {
        size_of::<ForgetIn>()
    }

    fn write_body(
        &self,
        write_bytes_fn: &mut dyn FnMut(&[u8]) -> FuseResult<()>,
    ) -> FuseResult<()> {
        write_bytes_fn(self.forget_in.as_bytes())
    }

    fn out_payload_size(&self) -> Option<usize> {
        None
    }

    fn parse_reply(
        self,
        _payload_len: usize,
        _read_bytes_fn: &mut dyn FnMut(usize, &mut [u8]) -> FuseResult<()>,
    ) -> FuseResult<Self::Output> {
        Ok(())
    }
}

fn name_body_len(prefix_len: usize, name: &str) -> usize {
    prefix_len
        .saturating_add(name.len())
        .saturating_add(NAME_TERMINATOR.len())
}

fn read_body<T: Pod>(
    offset: usize,
    read_bytes_fn: &mut dyn FnMut(usize, &mut [u8]) -> FuseResult<()>,
) -> FuseResult<T> {
    let mut value = T::new_zeroed();
    read_bytes_fn(offset, value.as_mut_bytes())?;
    Ok(value)
}
