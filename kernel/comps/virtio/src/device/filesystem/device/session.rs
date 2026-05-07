// SPDX-License-Identifier: MPL-2.0

//! FUSE session connection for `virtiofs`.
//!
//! [`FuseSession`] is mount-scoped FUSE session. It performs
//! `FUSE_INIT` negotiation, and exposes typed request helpers.

use alloc::{string::String, sync::Arc, vec::Vec};
use core::sync::atomic::{AtomicU64, Ordering};

use aster_fuse::{
    EntryOut, FUSE_KERNEL_MINOR_VERSION, FUSE_KERNEL_VERSION, FUSE_ROOT_ID, FuseDirEntry,
    FuseError, FuseFileHandle, FuseNodeId, LseekOut, MIN_MAX_WRITE, OpenOut,
    ops::{
        create::{CreateIn, CreateOperation},
        getattr::{FuseAttrOut, GetattrFlags, GetattrIn, GetattrOperation},
        init::{FuseInitFlags, FuseInitFlags2, InitIn, InitOperation},
        link::{LinkIn, LinkOperation},
        lookup::LookupOperation,
        lseek::{LseekIn, LseekOperation},
        mkdir::{MkdirIn, MkdirOperation},
        mknod::{MknodIn, MknodOperation},
        open::{OpenIn, OpenOperation, OpendirOperation},
        read::{ReadIn, ReadOperation},
        readdir::ReaddirOperation,
        readlink::ReadlinkOperation,
        release::{ReleaseFlags, ReleaseIn, ReleaseKind, ReleaseOperation},
        rmdir::RmdirOperation,
        setattr::{SetattrIn, SetattrOperation},
        unlink::UnlinkOperation,
        write::{WriteFlags, WriteIn, WriteOperation},
    },
};
use ostd::{
    info,
    mm::{VmReader, VmWriter},
    warn,
};

use super::{super::DEVICE_NAME, FileSystemDevice};

/// A mount-scoped FUSE session.
///
/// One `FuseSession` corresponds to one `mount(2)` call. It holds the
/// negotiated FUSE protocol state and forwards typed requests to the
/// underlying [`FileSystemDevice`].
pub struct FuseSession {
    /// The transport used to submit FUSE requests for this session.
    pub(super) device: Arc<FileSystemDevice>,
    /// Attribute cache version shared by all inodes in this session.
    attr_version: AtomicU64,
    /// The negotiated FUSE protocol major version.
    proto_major: u32,
    /// The negotiated FUSE protocol minor version.
    proto_minor: u32,
    /// The maximum write size accepted by the daemon.
    max_write: u32,
    /// The maximum readahead size accepted by the daemon.
    max_readahead: u32,
    /// The timestamp granularity in nanoseconds.
    time_gran: u32,
    /// The maximum number of pages in one request.
    max_pages: u16,
    /// The mapping alignment requirement as a power-of-two page count.
    map_alignment: u16,
    /// The feature flags selected by `FUSE_INIT`.
    //
    // TODO: Apply negotiated `FUSE_INIT` flags to conduct virtio-fs behavior.
    negotiated_flags: FuseInitFlags,
}

impl FuseSession {
    /// Creates a new FUSE session by performing `FUSE_INIT` negotiation with
    /// the daemon.
    pub fn new(device: Arc<FileSystemDevice>) -> Result<Arc<Self>, FuseError> {
        let requested_flags = Self::init_flags();
        let init_out = device.do_fuse_op(
            FUSE_ROOT_ID,
            InitOperation::new(InitIn::new(
                FUSE_KERNEL_VERSION,
                FUSE_KERNEL_MINOR_VERSION,
                0,
                requested_flags,
                FuseInitFlags2::empty(),
            )),
        )?;

        let max_write = init_out.max_write().max(MIN_MAX_WRITE);
        let conn = Arc::new(Self {
            device,
            attr_version: AtomicU64::new(1),
            proto_major: init_out.major(),
            proto_minor: init_out.minor(),
            max_write,
            max_readahead: init_out.max_readahead(),
            time_gran: init_out.time_gran(),
            max_pages: init_out.max_pages(),
            map_alignment: init_out.map_alignment(),
            negotiated_flags: init_out.flags(),
        });

        info!(
            "{} FUSE session started: protocol {}.{} -> {}.{}, \
             req_flags=0x{:x}, rsp_flags=0x{:x}, flags2=0x{:x}, \
             max_write={}, max_readahead={}, time_gran={}, max_pages={}, map_alignment={}",
            DEVICE_NAME,
            FUSE_KERNEL_VERSION,
            FUSE_KERNEL_MINOR_VERSION,
            init_out.major(),
            init_out.minor(),
            requested_flags.bits(),
            conn.negotiated_flags.bits(),
            init_out.flags2().bits(),
            conn.max_write,
            conn.max_readahead,
            conn.time_gran,
            conn.max_pages,
            conn.map_alignment,
        );

        Ok(conn)
    }

    /// Returns the current attribute version for a request snapshot.
    pub fn snapshot_attr_version(&self) -> AttrVersion {
        AttrVersion(self.attr_version.load(Ordering::Relaxed))
    }

    /// Commits a new attribute version and returns it.
    pub fn bump_attr_version(&self) -> AttrVersion {
        let mut current = self.attr_version.load(Ordering::Relaxed);
        loop {
            let next = current
                .checked_add(1)
                .expect("virtiofs attribute version overflow");
            match self.attr_version.compare_exchange_weak(
                current,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return AttrVersion(next),
                Err(actual) => current = actual,
            }
        }
    }

    /// Returns the FUSE protocol major version.
    pub fn proto_major(&self) -> u32 {
        self.proto_major
    }

    /// Returns the FUSE protocol minor version.
    pub fn proto_minor(&self) -> u32 {
        self.proto_minor
    }

    /// Returns the FUSE feature flags selected after negotiation.
    pub fn negotiated_flags(&self) -> FuseInitFlags {
        self.negotiated_flags
    }

    /// Returns the maximum write size accepted by the daemon.
    pub fn max_write(&self) -> u32 {
        self.max_write
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
}

impl FuseSession {
    pub fn lookup(&self, parent_nodeid: FuseNodeId, name: &str) -> Result<EntryOut, FuseError> {
        self.device
            .do_fuse_op(parent_nodeid, LookupOperation::new(name))
    }

    /// Releases `nlookup` accumulated lookup references for node sitting at server-side.
    pub fn forget(&self, nodeid: FuseNodeId, nlookup: u64) {
        self.device.forget(nodeid, nlookup)
    }

    pub fn getattr(
        &self,
        nodeid: FuseNodeId,
        getattr_flags: GetattrFlags,
        fh: FuseFileHandle,
    ) -> Result<FuseAttrOut, FuseError> {
        self.device.do_fuse_op(
            nodeid,
            GetattrOperation::new(GetattrIn::new(getattr_flags, fh)),
        )
    }

    /// Sets attributes according to the fields selected in [`SetattrIn`].
    pub fn setattr(
        &self,
        nodeid: FuseNodeId,
        setattr_in: SetattrIn,
    ) -> Result<FuseAttrOut, FuseError> {
        self.device
            .do_fuse_op(nodeid, SetattrOperation::new(setattr_in))
    }

    pub fn readlink(&self, nodeid: FuseNodeId) -> Result<String, FuseError> {
        self.device.do_fuse_op(nodeid, ReadlinkOperation)
    }

    pub fn mknod(
        &self,
        parent_nodeid: FuseNodeId,
        name: &str,
        mode: u32,
        rdev: u32,
    ) -> Result<EntryOut, FuseError> {
        self.device.do_fuse_op(
            parent_nodeid,
            MknodOperation::new(MknodIn::new(mode, rdev), name),
        )
    }

    pub fn mkdir(
        &self,
        parent_nodeid: FuseNodeId,
        name: &str,
        mode: u32,
    ) -> Result<EntryOut, FuseError> {
        self.device
            .do_fuse_op(parent_nodeid, MkdirOperation::new(MkdirIn::new(mode), name))
    }

    pub fn unlink(&self, parent_nodeid: FuseNodeId, name: &str) -> Result<(), FuseError> {
        self.device
            .do_fuse_op(parent_nodeid, UnlinkOperation::new(name))
    }

    pub fn rmdir(&self, parent_nodeid: FuseNodeId, name: &str) -> Result<(), FuseError> {
        self.device
            .do_fuse_op(parent_nodeid, RmdirOperation::new(name))
    }

    pub fn link(
        &self,
        old_nodeid: FuseNodeId,
        new_parent_nodeid: FuseNodeId,
        new_name: &str,
    ) -> Result<EntryOut, FuseError> {
        self.device.do_fuse_op(
            new_parent_nodeid,
            LinkOperation::new(LinkIn::new(old_nodeid), new_name),
        )
    }

    pub fn open(&self, nodeid: FuseNodeId, flags: u32) -> Result<OpenOut, FuseError> {
        self.device
            .do_fuse_op(nodeid, OpenOperation::new(OpenIn::new(flags)))
    }

    pub fn read(
        &self,
        nodeid: FuseNodeId,
        fh: FuseFileHandle,
        offset: u64,
        size: u32,
        flags: u32,
        writer: &mut VmWriter,
    ) -> Result<usize, FuseError> {
        self.device.do_fuse_op(
            nodeid,
            ReadOperation::new(ReadIn::new(fh, offset, size, flags), writer),
        )
    }

    pub fn write(
        &self,
        nodeid: FuseNodeId,
        fh: FuseFileHandle,
        offset: u64,
        flags: u32,
        write_flags: WriteFlags,
        reader: &mut VmReader,
    ) -> Result<usize, FuseError> {
        let mut total_written = 0usize;

        while reader.has_remain() {
            let write_size = reader.remain().min(self.max_write as usize);

            let mut request_reader = reader.clone();
            request_reader.limit(write_size);

            let request_offset = offset
                .checked_add(total_written as u64)
                .ok_or(FuseError::LengthOverflow)?;
            let written = self.device.do_fuse_op(
                nodeid,
                WriteOperation::new(
                    WriteIn::new(fh, request_offset, write_size as u32, flags, write_flags),
                    &mut request_reader,
                ),
            )?;

            if written > write_size {
                return Err(FuseError::MalformedResponse);
            }
            if written == 0 {
                break;
            }

            reader.skip(written);
            total_written = total_written
                .checked_add(written)
                .ok_or(FuseError::LengthOverflow)?;
            if written < write_size {
                break;
            }
        }

        Ok(total_written)
    }

    /// Releases the file or directory handle `fh` on `nodeid`.
    ///
    /// Errors are silently ignored; callers are not notified of release failures.
    pub fn release(
        &self,
        nodeid: FuseNodeId,
        fh: FuseFileHandle,
        flags: u32,
        release_flags: ReleaseFlags,
        kind: ReleaseKind,
    ) {
        if let Err(err) = self.device.do_fuse_op(
            nodeid,
            ReleaseOperation::new(ReleaseIn::new(fh, flags, release_flags), kind),
        ) {
            warn!("virtiofs release failed for inode {:?}: {:?}", nodeid, err);
        }
    }

    pub fn opendir(&self, nodeid: FuseNodeId) -> Result<OpenOut, FuseError> {
        self.device
            .do_fuse_op(nodeid, OpendirOperation::new(OpenIn::new(0)))
    }

    pub fn readdir(
        &self,
        nodeid: FuseNodeId,
        fh: FuseFileHandle,
        offset: u64,
        size: u32,
        flags: u32,
    ) -> Result<Vec<FuseDirEntry>, FuseError> {
        self.device.do_fuse_op(
            nodeid,
            ReaddirOperation::new(ReadIn::new(fh, offset, size, flags), size as usize),
        )
    }

    pub fn create(
        &self,
        parent_nodeid: FuseNodeId,
        name: &str,
        mode: u32,
    ) -> Result<(EntryOut, OpenOut), FuseError> {
        const O_RDWR: u32 = 2;

        // TODO: Pass the caller's open flags once the VFS create path carries them.
        self.device.do_fuse_op(
            parent_nodeid,
            CreateOperation::new(CreateIn::new(O_RDWR, mode), name),
        )
    }

    pub fn lseek(
        &self,
        nodeid: FuseNodeId,
        fh: FuseFileHandle,
        offset: i64,
        whence: u32,
    ) -> Result<LseekOut, FuseError> {
        self.device.do_fuse_op(
            nodeid,
            LseekOperation::new(LseekIn::new(fh, offset, whence)),
        )
    }
}

/// Monotonically increasing version tag for inode attribute updates.
///
/// Each FUSE request that may return attributes (e.g. `FUSE_GETATTR`,
/// `FUSE_LOOKUP`, `FUSE_SETATTR`) snapshots the current global version
/// before the request is sent.  When the reply arrives, the snapshot is
/// compared against the inode's committed version: if the inode version
/// is strictly greater, a newer update has already committed and the
/// stale reply is discarded.  Local metadata changes (e.g. writes that
/// extend the file size) also bump the version.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct AttrVersion(u64);
