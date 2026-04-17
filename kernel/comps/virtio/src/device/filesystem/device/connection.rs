// SPDX-License-Identifier: MPL-2.0

//! FUSE session connection for `virtiofs`.
//!
//! [`FuseConnection`] represents one mount-scoped FUSE session. It is created
//! at `VirtioFs::new()` time, performs `FUSE_INIT` negotiation, and exposes
//! typed request helpers. When the last mount using this connection is
//! dropped, [`FuseConnection`] is dropped too and the session ends.
//!
//! This mirrors Linux's `fuse_conn` lifetime: the virtio device
//! (`FileSystemDevice`) is device-scoped and may outlive any individual mount,
//! while `FuseConnection` is mount-scoped.

use alloc::{string::String, sync::Arc, vec::Vec};

use aster_fuse::*;
use ostd::{info, warn};

use super::{super::DEVICE_NAME, FileSystemDevice, ReleaseKind, VirtioFsDirEntry, operation::*};
use crate::device::VirtioDeviceError;

/// A mount-scoped FUSE session.
///
/// One `FuseConnection` corresponds to one `mount(2)` call. It holds the
/// negotiated FUSE protocol state and forwards typed requests to the
/// underlying [`FileSystemDevice`].
pub struct FuseConnection {
    pub(super) device: Arc<FileSystemDevice>,
    proto_major: u32,
    proto_minor: u32,
    max_write: u32,
    max_readahead: u32,
    time_gran: u32,
    max_pages: u16,
    map_alignment: u16,
    // TODO: Apply negotiated `FUSE_INIT` flags to virtio-fs behavior. The
    // current implementation does not expose virtiofsd's cache policy choices
    // and follows the virtiofsd default `cache=auto` behavior, which provides
    // open-to-close consistency without write-back caching.
    negotiated_flags: FuseInitFlags,
}

impl FuseConnection {
    /// Creates a new FUSE session by performing `FUSE_INIT` negotiation with
    /// the daemon. Called from `VirtioFs::new()` at mount time.
    pub fn new(device: Arc<FileSystemDevice>) -> Result<Arc<Self>, VirtioDeviceError> {
        let requested_flags = Self::init_flags();
        let init_out = device.do_fuse_op(InitOperation::new(InitIn::new(
            FUSE_KERNEL_VERSION,
            FUSE_KERNEL_MINOR_VERSION,
            0,
            requested_flags,
            FuseInitFlags::empty(),
        )))?;

        let conn = Arc::new(Self {
            device,
            proto_major: init_out.major,
            proto_minor: init_out.minor,
            max_write: init_out.max_write,
            max_readahead: init_out.max_readahead,
            time_gran: init_out.time_gran,
            max_pages: init_out.max_pages,
            map_alignment: init_out.map_alignment,
            negotiated_flags: FuseInitFlags::from_bits_truncate(init_out.flags),
        });

        info!(
            "{} FUSE session started: protocol {}.{} -> {}.{}, \
             req_flags=0x{:x}, rsp_flags=0x{:x}, flags2=0x{:x}, \
             max_write={}, max_readahead={}, time_gran={}, max_pages={}, map_alignment={}",
            DEVICE_NAME,
            FUSE_KERNEL_VERSION,
            FUSE_KERNEL_MINOR_VERSION,
            init_out.major,
            init_out.minor,
            requested_flags.bits(),
            conn.negotiated_flags.bits(),
            init_out.flags2,
            conn.max_write,
            conn.max_readahead,
            conn.time_gran,
            conn.max_pages,
            conn.map_alignment,
        );

        Ok(conn)
    }

    fn init_flags() -> FuseInitFlags {
        FuseInitFlags::ASYNC_READ
            | FuseInitFlags::ATOMIC_O_TRUNC
            | FuseInitFlags::AUTO_INVAL_DATA
            | FuseInitFlags::BIG_WRITES
            | FuseInitFlags::HANDLE_KILLPRIV
            | FuseInitFlags::MAX_PAGES
            | FuseInitFlags::PARALLEL_DIROPS
            | FuseInitFlags::INIT_EXT
    }

    pub fn proto_major(&self) -> u32 {
        self.proto_major
    }

    pub fn proto_minor(&self) -> u32 {
        self.proto_minor
    }

    pub fn negotiated_flags(&self) -> FuseInitFlags {
        self.negotiated_flags
    }

    pub fn max_write(&self) -> u32 {
        self.max_write
    }
}

// ---------------------------------------------------------------------------
// High-level FUSE request helpers
// ---------------------------------------------------------------------------

// TODO: Send `FUSE_DESTROY` (opcode 38) when the connection is dropped, to
// mirror Linux's `fuse_conn_destroy`. Without it, the daemon cannot release
// per-session state on unmount, which may matter for remount scenarios or
// daemons that track lookup counts across sessions.
// Reference: https://codebrowser.dev/linux/linux/fs/fuse/inode.c.html#fuse_conn_destroy
impl FuseConnection {
    pub fn fuse_lookup(
        &self,
        parent_nodeid: u64,
        name: &str,
    ) -> Result<EntryOut, VirtioDeviceError> {
        self.device
            .do_fuse_op(LookupOperation::new(parent_nodeid, name))
    }

    pub fn fuse_mkdir(
        &self,
        parent_nodeid: u64,
        name: &str,
        mode: u32,
    ) -> Result<EntryOut, VirtioDeviceError> {
        self.device
            .do_fuse_op(MkdirOperation::new(parent_nodeid, MkdirIn::new(mode), name))
    }

    pub fn fuse_mknod(
        &self,
        parent_nodeid: u64,
        name: &str,
        mode: u32,
        rdev: u32,
    ) -> Result<EntryOut, VirtioDeviceError> {
        self.device.do_fuse_op(MknodOperation::new(
            parent_nodeid,
            MknodIn::new(mode, rdev),
            name,
        ))
    }

    pub fn fuse_unlink(&self, parent_nodeid: u64, name: &str) -> Result<(), VirtioDeviceError> {
        self.device
            .do_fuse_op(UnlinkOperation::new(parent_nodeid, name))
    }

    pub fn fuse_rmdir(&self, parent_nodeid: u64, name: &str) -> Result<(), VirtioDeviceError> {
        self.device
            .do_fuse_op(RmdirOperation::new(parent_nodeid, name))
    }

    pub fn fuse_create(
        &self,
        parent_nodeid: u64,
        name: &str,
        mode: u32,
    ) -> Result<(EntryOut, OpenOut), VirtioDeviceError> {
        self.device.do_fuse_op(CreateOperation::new(
            parent_nodeid,
            CreateIn::new(super::O_RDWR, mode),
            name,
        ))
    }

    pub fn fuse_getattr(
        &self,
        nodeid: u64,
        fh: FuseFileHandle,
    ) -> Result<FuseAttrOut, VirtioDeviceError> {
        self.device
            .do_fuse_op(GetattrOperation::new(nodeid, GetattrIn::new(fh)))
    }

    pub fn fuse_setattr(
        &self,
        nodeid: u64,
        setattr_in: SetattrIn,
    ) -> Result<FuseAttrOut, VirtioDeviceError> {
        self.device
            .do_fuse_op(SetattrOperation::new(nodeid, setattr_in))
    }

    pub fn fuse_opendir(&self, nodeid: u64) -> Result<FuseFileHandle, VirtioDeviceError> {
        self.device
            .do_fuse_op(OpendirOperation::new(nodeid, OpenIn::new(0)))
    }

    pub fn fuse_readdir(
        &self,
        nodeid: u64,
        fh: FuseFileHandle,
        offset: u64,
        size: u32,
    ) -> Result<Vec<VirtioFsDirEntry>, VirtioDeviceError> {
        self.device.do_fuse_op(ReaddirOperation::new(
            nodeid,
            ReadIn::new(fh, offset, size),
            size as usize,
        ))
    }

    pub fn fuse_readlink(&self, nodeid: u64) -> Result<String, VirtioDeviceError> {
        self.device.do_fuse_op(ReadlinkOperation::new(nodeid))
    }

    pub fn fuse_link(
        &self,
        old_nodeid: u64,
        new_parent_nodeid: u64,
        new_name: &str,
    ) -> Result<EntryOut, VirtioDeviceError> {
        self.device.do_fuse_op(LinkOperation::new(
            LinkIn::new(old_nodeid),
            new_parent_nodeid,
            new_name,
        ))
    }

    pub fn fuse_open(&self, nodeid: u64, flags: u32) -> Result<OpenOut, VirtioDeviceError> {
        self.device
            .do_fuse_op(OpenOperation::new(nodeid, OpenIn::new(flags)))
    }

    pub fn fuse_release(&self, nodeid: u64, fh: FuseFileHandle, flags: u32, kind: ReleaseKind) {
        // Linux does not propagate `FUSE_RELEASE` / `FUSE_RELEASEDIR` failures
        // back to the close path. The release request is best-effort cleanup,
        // and resource teardown proceeds regardless of transport/server errors.
        // Reference: https://codebrowser.dev/linux/linux/fs/fuse/file.c.html#113
        // Reference: https://codebrowser.dev/linux/linux/fs/fuse/file.c.html#117
        if let Err(err) = self.device.do_fuse_op(ReleaseOperation::new(
            nodeid,
            ReleaseIn::new(fh, flags),
            kind,
        )) {
            warn!("virtiofs release failed for inode {}: {:?}", nodeid, err);
        }
    }

    pub fn fuse_lseek(
        &self,
        nodeid: u64,
        fh: FuseFileHandle,
        offset: i64,
        whence: u32,
    ) -> Result<i64, VirtioDeviceError> {
        self.device.do_fuse_op(LseekOperation::new(
            nodeid,
            LseekIn::new(fh, offset, whence),
        ))
    }

    pub fn fuse_read(
        &self,
        nodeid: u64,
        fh: FuseFileHandle,
        offset: u64,
        size: u32,
    ) -> Result<Vec<u8>, VirtioDeviceError> {
        self.device.do_fuse_op(ReadOperation::new(
            nodeid,
            ReadIn::new(fh, offset, size),
            size as usize,
        ))
    }

    pub fn fuse_write(
        &self,
        nodeid: u64,
        fh: FuseFileHandle,
        offset: u64,
        data: &[u8],
    ) -> Result<usize, VirtioDeviceError> {
        let data_len =
            u32::try_from(data.len()).map_err(|_| VirtioDeviceError::ResourceAllocError)?;
        self.device.do_fuse_op(WriteOperation::new(
            nodeid,
            WriteIn::new(fh, offset, data_len),
            data,
        ))
    }

    pub fn fuse_forget(&self, nodeid: u64, nlookup: u64) {
        self.device.forget(nodeid, nlookup)
    }
}
