// SPDX-License-Identifier: MPL-2.0

use alloc::{
    string::{String, ToString},
    vec,
    vec::Vec,
};
use core::{cmp, mem::size_of};

use ostd_pod::{IntoBytes, Pod};

use super::*;

const NAME_TERMINATOR: &[u8] = &[0];

struct InitOperation {
    init_in: InitIn,
}

impl FuseOperation for InitOperation {
    type Output = InitOut;

    fn opcode(&self) -> FuseOpcode {
        FuseOpcode::Init
    }

    fn nodeid(&self) -> u64 {
        0
    }

    fn body_segments(&self) -> Vec<&[u8]> {
        vec![self.init_in.as_bytes()]
    }

    fn out_payload_size(&self) -> Option<usize> {
        Some(size_of::<InitOut>())
    }
}

struct LookupOperation<'a> {
    parent_nodeid: u64,
    name: &'a str,
}

impl FuseOperation for LookupOperation<'_> {
    type Output = EntryOut;

    fn opcode(&self) -> FuseOpcode {
        FuseOpcode::Lookup
    }

    fn nodeid(&self) -> u64 {
        self.parent_nodeid
    }

    fn body_segments(&self) -> Vec<&[u8]> {
        vec![self.name.as_bytes(), NAME_TERMINATOR]
    }

    fn out_payload_size(&self) -> Option<usize> {
        Some(size_of::<EntryOut>())
    }
}

struct MkdirOperation<'a> {
    parent_nodeid: u64,
    mkdir_in: MkdirIn,
    name: &'a str,
}

impl FuseOperation for MkdirOperation<'_> {
    type Output = EntryOut;

    fn opcode(&self) -> FuseOpcode {
        FuseOpcode::Mkdir
    }

    fn nodeid(&self) -> u64 {
        self.parent_nodeid
    }

    fn body_segments(&self) -> Vec<&[u8]> {
        vec![
            self.mkdir_in.as_bytes(),
            self.name.as_bytes(),
            NAME_TERMINATOR,
        ]
    }

    fn out_payload_size(&self) -> Option<usize> {
        Some(size_of::<EntryOut>())
    }
}

struct MknodOperation<'a> {
    parent_nodeid: u64,
    mknod_in: MknodIn,
    name: &'a str,
}

impl FuseOperation for MknodOperation<'_> {
    type Output = EntryOut;

    fn opcode(&self) -> FuseOpcode {
        FuseOpcode::Mknod
    }

    fn nodeid(&self) -> u64 {
        self.parent_nodeid
    }

    fn body_segments(&self) -> Vec<&[u8]> {
        vec![
            self.mknod_in.as_bytes(),
            self.name.as_bytes(),
            NAME_TERMINATOR,
        ]
    }

    fn out_payload_size(&self) -> Option<usize> {
        Some(size_of::<EntryOut>())
    }
}

struct UnlinkOperation<'a> {
    parent_nodeid: u64,
    name: &'a str,
}

impl FuseOperation for UnlinkOperation<'_> {
    type Output = ();

    fn opcode(&self) -> FuseOpcode {
        FuseOpcode::Unlink
    }

    fn nodeid(&self) -> u64 {
        self.parent_nodeid
    }

    fn body_segments(&self) -> Vec<&[u8]> {
        vec![self.name.as_bytes(), NAME_TERMINATOR]
    }

    fn out_payload_size(&self) -> Option<usize> {
        Some(0)
    }

    fn parse_reply(self, _request: &FuseRequest) -> Result<Self::Output, VirtioDeviceError> {
        Ok(())
    }
}

struct RmdirOperation<'a> {
    parent_nodeid: u64,
    name: &'a str,
}

impl FuseOperation for RmdirOperation<'_> {
    type Output = ();

    fn opcode(&self) -> FuseOpcode {
        FuseOpcode::Rmdir
    }

    fn nodeid(&self) -> u64 {
        self.parent_nodeid
    }

    fn body_segments(&self) -> Vec<&[u8]> {
        vec![self.name.as_bytes(), NAME_TERMINATOR]
    }

    fn out_payload_size(&self) -> Option<usize> {
        Some(0)
    }

    fn parse_reply(self, _request: &FuseRequest) -> Result<Self::Output, VirtioDeviceError> {
        Ok(())
    }
}

struct CreateOperation<'a> {
    parent_nodeid: u64,
    create_in: CreateIn,
    name: &'a str,
}

impl FuseOperation for CreateOperation<'_> {
    type Output = (EntryOut, OpenOut);

    fn opcode(&self) -> FuseOpcode {
        FuseOpcode::Create
    }

    fn nodeid(&self) -> u64 {
        self.parent_nodeid
    }

    fn body_segments(&self) -> Vec<&[u8]> {
        vec![
            self.create_in.as_bytes(),
            self.name.as_bytes(),
            NAME_TERMINATOR,
        ]
    }

    fn out_payload_size(&self) -> Option<usize> {
        Some(size_of::<EntryOut>() + size_of::<OpenOut>())
    }

    fn parse_reply(self, request: &FuseRequest) -> Result<Self::Output, VirtioDeviceError> {
        let entry_out = request.read_payload(0)?;
        let open_out = request.read_payload(size_of::<EntryOut>())?;
        Ok((entry_out, open_out))
    }
}

struct GetattrOperation {
    nodeid: u64,
    getattr_in: GetattrIn,
}

impl FuseOperation for GetattrOperation {
    type Output = FuseAttrOut;

    fn opcode(&self) -> FuseOpcode {
        FuseOpcode::Getattr
    }

    fn nodeid(&self) -> u64 {
        self.nodeid
    }

    fn body_segments(&self) -> Vec<&[u8]> {
        vec![self.getattr_in.as_bytes()]
    }

    fn out_payload_size(&self) -> Option<usize> {
        Some(size_of::<FuseAttrOut>())
    }
}

struct SetattrOperation {
    nodeid: u64,
    setattr_in: SetattrIn,
}

impl FuseOperation for SetattrOperation {
    type Output = FuseAttrOut;

    fn opcode(&self) -> FuseOpcode {
        FuseOpcode::Setattr
    }

    fn nodeid(&self) -> u64 {
        self.nodeid
    }

    fn body_segments(&self) -> Vec<&[u8]> {
        vec![self.setattr_in.as_bytes()]
    }

    fn out_payload_size(&self) -> Option<usize> {
        Some(size_of::<FuseAttrOut>())
    }
}

struct OpendirOperation {
    nodeid: u64,
    open_in: OpenIn,
}

impl FuseOperation for OpendirOperation {
    type Output = u64;

    fn opcode(&self) -> FuseOpcode {
        FuseOpcode::Opendir
    }

    fn nodeid(&self) -> u64 {
        self.nodeid
    }

    fn body_segments(&self) -> Vec<&[u8]> {
        vec![self.open_in.as_bytes()]
    }

    fn out_payload_size(&self) -> Option<usize> {
        Some(size_of::<OpenOut>())
    }

    fn parse_reply(self, request: &FuseRequest) -> Result<Self::Output, VirtioDeviceError> {
        let open_out: OpenOut = request.read_payload(0)?;
        Ok(open_out.fh)
    }
}

struct ReaddirOperation {
    nodeid: u64,
    read_in: ReadIn,
    size: usize,
}

impl FuseOperation for ReaddirOperation {
    type Output = Vec<VirtioFsDirEntry>;

    fn opcode(&self) -> FuseOpcode {
        FuseOpcode::Readdir
    }

    fn nodeid(&self) -> u64 {
        self.nodeid
    }

    fn body_segments(&self) -> Vec<&[u8]> {
        vec![self.read_in.as_bytes()]
    }

    fn out_payload_size(&self) -> Option<usize> {
        Some(self.size)
    }

    fn parse_reply(self, request: &FuseRequest) -> Result<Self::Output, VirtioDeviceError> {
        let payload_len = request.reply_payload_len()?;
        let payload_len = cmp::min(payload_len, self.size);
        let mut payload = vec![0u8; payload_len];
        request.read_payload_bytes(0, payload.as_mut_slice())?;

        let mut entries = Vec::new();
        let mut pos = 0usize;
        while pos + size_of::<Dirent>() <= payload_len {
            let header = Dirent::from_bytes(&payload[pos..pos + size_of::<Dirent>()]);
            if header.namelen == 0 {
                break;
            }

            let name_start = pos + size_of::<Dirent>();
            let name_end = name_start + header.namelen as usize;
            if name_end > payload_len {
                break;
            }

            if let Ok(name) = core::str::from_utf8(&payload[name_start..name_end]) {
                entries.push(VirtioFsDirEntry {
                    ino: header.ino,
                    offset: header.off,
                    type_: header.typ,
                    name: name.to_string(),
                });
            }

            let dirent_len = size_of::<Dirent>() + header.namelen as usize;
            pos += (dirent_len + 7) & !7;
        }

        Ok(entries)
    }
}

struct ReleasedirOperation {
    nodeid: u64,
    release_in: ReleaseIn,
}

impl FuseOperation for ReleasedirOperation {
    type Output = ();

    fn opcode(&self) -> FuseOpcode {
        FuseOpcode::Releasedir
    }

    fn nodeid(&self) -> u64 {
        self.nodeid
    }

    fn body_segments(&self) -> Vec<&[u8]> {
        vec![self.release_in.as_bytes()]
    }

    fn out_payload_size(&self) -> Option<usize> {
        Some(0)
    }

    fn parse_reply(self, _request: &FuseRequest) -> Result<Self::Output, VirtioDeviceError> {
        Ok(())
    }
}

struct ReadlinkOperation {
    nodeid: u64,
}

impl FuseOperation for ReadlinkOperation {
    type Output = String;

    fn opcode(&self) -> FuseOpcode {
        FuseOpcode::Readlink
    }

    fn nodeid(&self) -> u64 {
        self.nodeid
    }

    fn body_segments(&self) -> Vec<&[u8]> {
        vec![]
    }

    fn out_payload_size(&self) -> Option<usize> {
        Some(4096)
    }

    fn parse_reply(self, request: &FuseRequest) -> Result<Self::Output, VirtioDeviceError> {
        let payload_len = request.reply_payload_len()?;
        let mut payload = vec![0u8; payload_len];
        request.read_payload_bytes(0, payload.as_mut_slice())?;

        let end = payload
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(payload_len);
        Ok(String::from_utf8_lossy(&payload[..end]).to_string())
    }
}

struct LinkOperation<'a> {
    link_in: LinkIn,
    new_parent_nodeid: u64,
    new_name: &'a str,
}

impl FuseOperation for LinkOperation<'_> {
    type Output = EntryOut;

    fn opcode(&self) -> FuseOpcode {
        FuseOpcode::Link
    }

    fn nodeid(&self) -> u64 {
        self.new_parent_nodeid
    }

    fn body_segments(&self) -> Vec<&[u8]> {
        vec![
            self.link_in.as_bytes(),
            self.new_name.as_bytes(),
            NAME_TERMINATOR,
        ]
    }

    fn out_payload_size(&self) -> Option<usize> {
        Some(size_of::<EntryOut>())
    }
}

struct OpenOperation {
    nodeid: u64,
    open_in: OpenIn,
}

impl FuseOperation for OpenOperation {
    type Output = OpenOut;

    fn opcode(&self) -> FuseOpcode {
        FuseOpcode::Open
    }

    fn nodeid(&self) -> u64 {
        self.nodeid
    }

    fn body_segments(&self) -> Vec<&[u8]> {
        vec![self.open_in.as_bytes()]
    }

    fn out_payload_size(&self) -> Option<usize> {
        Some(size_of::<OpenOut>())
    }
}

struct ReleaseOperation {
    nodeid: u64,
    release_in: ReleaseIn,
}

impl FuseOperation for ReleaseOperation {
    type Output = ();

    fn opcode(&self) -> FuseOpcode {
        FuseOpcode::Release
    }

    fn nodeid(&self) -> u64 {
        self.nodeid
    }

    fn body_segments(&self) -> Vec<&[u8]> {
        vec![self.release_in.as_bytes()]
    }

    fn out_payload_size(&self) -> Option<usize> {
        Some(0)
    }

    fn parse_reply(self, _request: &FuseRequest) -> Result<Self::Output, VirtioDeviceError> {
        Ok(())
    }
}

struct LseekOperation {
    nodeid: u64,
    lseek_in: LseekIn,
}

impl FuseOperation for LseekOperation {
    type Output = i64;

    fn opcode(&self) -> FuseOpcode {
        FuseOpcode::Lseek
    }

    fn nodeid(&self) -> u64 {
        self.nodeid
    }

    fn body_segments(&self) -> Vec<&[u8]> {
        vec![self.lseek_in.as_bytes()]
    }

    fn out_payload_size(&self) -> Option<usize> {
        Some(size_of::<LseekOut>())
    }

    fn parse_reply(self, request: &FuseRequest) -> Result<Self::Output, VirtioDeviceError> {
        let lseek_out: LseekOut = request.read_payload(0)?;
        Ok(lseek_out.offset)
    }
}

struct ReadOperation {
    nodeid: u64,
    read_in: ReadIn,
    size: usize,
}

impl FuseOperation for ReadOperation {
    type Output = Vec<u8>;

    fn opcode(&self) -> FuseOpcode {
        FuseOpcode::Read
    }

    fn nodeid(&self) -> u64 {
        self.nodeid
    }

    fn body_segments(&self) -> Vec<&[u8]> {
        vec![self.read_in.as_bytes()]
    }

    fn out_payload_size(&self) -> Option<usize> {
        Some(self.size)
    }

    fn parse_reply(self, request: &FuseRequest) -> Result<Self::Output, VirtioDeviceError> {
        let payload_len = request.reply_payload_len()?;
        let payload_len = cmp::min(payload_len, self.size);
        let mut content = vec![0u8; payload_len];
        request.read_payload_bytes(0, content.as_mut_slice())?;
        Ok(content)
    }
}

struct WriteOperation<'a> {
    nodeid: u64,
    write_in: WriteIn,
    data: &'a [u8],
}

impl FuseOperation for WriteOperation<'_> {
    type Output = usize;

    fn opcode(&self) -> FuseOpcode {
        FuseOpcode::Write
    }

    fn nodeid(&self) -> u64 {
        self.nodeid
    }

    fn body_segments(&self) -> Vec<&[u8]> {
        vec![self.write_in.as_bytes(), self.data]
    }

    fn out_payload_size(&self) -> Option<usize> {
        Some(size_of::<WriteOut>())
    }

    fn parse_reply(self, request: &FuseRequest) -> Result<Self::Output, VirtioDeviceError> {
        let write_out: WriteOut = request.read_payload(0)?;
        Ok(write_out.size as usize)
    }
}

impl FileSystemDevice {
    pub(crate) fn fuse_init(&self) -> Result<(), VirtioDeviceError> {
        let operation = InitOperation {
            init_in: InitIn::new(FUSE_KERNEL_VERSION, FUSE_KERNEL_MINOR_VERSION, 0, 0, 0),
        };
        let request = operation.request(self)?;
        let request = alloc::sync::Arc::new(request);
        self.submit_to_queue(&self.request_queues[0], request.clone())?;
        loop {
            self.handle_queue_irq(&self.request_queues[0]);

            if request.wait_state.lock().completed {
                break;
            }

            core::hint::spin_loop();
        }
        request.check_reply()?;
        let init_out = operation.parse_reply(&request)?;

        info!(
            "{} FUSE session started: protocol {}.{} -> {}.{}, max_write={}, flags=0x{:x}",
            DEVICE_NAME,
            FUSE_KERNEL_VERSION,
            FUSE_KERNEL_MINOR_VERSION,
            init_out.major,
            init_out.minor,
            init_out.max_write,
            init_out.flags,
        );

        Ok(())
    }

    pub fn fuse_lookup(
        &self,
        parent_nodeid: u64,
        name: &str,
    ) -> Result<EntryOut, VirtioDeviceError> {
        self.execute(LookupOperation {
            parent_nodeid,
            name,
        })
    }

    pub fn fuse_mkdir(
        &self,
        parent_nodeid: u64,
        name: &str,
        mode: u32,
    ) -> Result<EntryOut, VirtioDeviceError> {
        self.execute(MkdirOperation {
            parent_nodeid,
            mkdir_in: MkdirIn::new(mode),
            name,
        })
    }

    pub fn fuse_mknod(
        &self,
        parent_nodeid: u64,
        name: &str,
        mode: u32,
        rdev: u32,
    ) -> Result<EntryOut, VirtioDeviceError> {
        self.execute(MknodOperation {
            parent_nodeid,
            mknod_in: MknodIn::new(mode, rdev),
            name,
        })
    }

    pub fn fuse_unlink(&self, parent_nodeid: u64, name: &str) -> Result<(), VirtioDeviceError> {
        self.execute(UnlinkOperation {
            parent_nodeid,
            name,
        })
    }

    pub fn fuse_rmdir(&self, parent_nodeid: u64, name: &str) -> Result<(), VirtioDeviceError> {
        self.execute(RmdirOperation {
            parent_nodeid,
            name,
        })
    }

    pub fn fuse_create(
        &self,
        parent_nodeid: u64,
        name: &str,
        mode: u32,
    ) -> Result<(EntryOut, OpenOut), VirtioDeviceError> {
        self.execute(CreateOperation {
            parent_nodeid,
            create_in: CreateIn::new(O_RDWR, mode),
            name,
        })
    }

    pub fn fuse_getattr(&self, nodeid: u64) -> Result<FuseAttrOut, VirtioDeviceError> {
        self.execute(GetattrOperation {
            nodeid,
            getattr_in: GetattrIn::new(0),
        })
    }

    pub fn fuse_setattr(
        &self,
        nodeid: u64,
        setattr_in: SetattrIn,
    ) -> Result<FuseAttrOut, VirtioDeviceError> {
        self.execute(SetattrOperation { nodeid, setattr_in })
    }

    pub fn fuse_opendir(&self, nodeid: u64) -> Result<u64, VirtioDeviceError> {
        self.execute(OpendirOperation {
            nodeid,
            open_in: OpenIn::new(0),
        })
    }

    pub fn fuse_readdir(
        &self,
        nodeid: u64,
        fh: u64,
        offset: u64,
        size: u32,
    ) -> Result<Vec<VirtioFsDirEntry>, VirtioDeviceError> {
        self.execute(ReaddirOperation {
            nodeid,
            read_in: ReadIn::new(fh, offset, size),
            size: size as usize,
        })
    }

    pub fn fuse_releasedir(&self, nodeid: u64, fh: u64) -> Result<(), VirtioDeviceError> {
        self.execute(ReleasedirOperation {
            nodeid,
            release_in: ReleaseIn::new(fh, 0),
        })
    }

    pub fn fuse_readlink(&self, nodeid: u64) -> Result<String, VirtioDeviceError> {
        self.execute(ReadlinkOperation { nodeid })
    }

    pub fn fuse_link(
        &self,
        old_nodeid: u64,
        new_parent_nodeid: u64,
        new_name: &str,
    ) -> Result<EntryOut, VirtioDeviceError> {
        self.execute(LinkOperation {
            link_in: LinkIn::new(old_nodeid),
            new_parent_nodeid,
            new_name,
        })
    }

    pub fn fuse_open(&self, nodeid: u64, flags: u32) -> Result<OpenOut, VirtioDeviceError> {
        self.execute(OpenOperation {
            nodeid,
            open_in: OpenIn::new(flags),
        })
    }

    pub fn fuse_release(&self, nodeid: u64, fh: u64, flags: u32) -> Result<(), VirtioDeviceError> {
        self.execute(ReleaseOperation {
            nodeid,
            release_in: ReleaseIn::new(fh, flags),
        })
    }

    pub fn fuse_lseek(
        &self,
        nodeid: u64,
        fh: u64,
        offset: i64,
        whence: u32,
    ) -> Result<i64, VirtioDeviceError> {
        self.execute(LseekOperation {
            nodeid,
            lseek_in: LseekIn::new(fh, offset, whence),
        })
    }

    pub fn fuse_read(
        &self,
        nodeid: u64,
        fh: u64,
        offset: u64,
        size: u32,
    ) -> Result<Vec<u8>, VirtioDeviceError> {
        self.execute(ReadOperation {
            nodeid,
            read_in: ReadIn::new(fh, offset, size),
            size: size as usize,
        })
    }

    pub fn fuse_write(
        &self,
        nodeid: u64,
        fh: u64,
        offset: u64,
        data: &[u8],
    ) -> Result<usize, VirtioDeviceError> {
        self.execute(WriteOperation {
            nodeid,
            write_in: WriteIn::new(fh, offset, data.len() as u32),
            data,
        })
    }

    pub fn fuse_forget(&self, nodeid: u64, nlookup: u64) -> Result<(), VirtioDeviceError> {
        if nodeid == FUSE_ROOT_ID || nlookup == 0 {
            return Ok(());
        }
        let request = self.prepare_fuse_request(
            FuseOpcode::Forget as u32,
            nodeid,
            &[ForgetIn::new(nlookup).as_bytes()],
            None,
        )?;
        let _ = self.submit_to_queue(&self.hiprio_queue, alloc::sync::Arc::new(request))?;
        Ok(())
    }
}
