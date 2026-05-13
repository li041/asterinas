// SPDX-License-Identifier: MPL-2.0

//! FUSE session connection for `virtiofs`.
//!
//! [`FuseSession`] is mount-scoped FUSE session. It performs
//! `FUSE_INIT` negotiation, and exposes typed request helpers.

use alloc::sync::Arc;
use core::sync::atomic::{AtomicU64, Ordering};

use aster_fuse::{
    FUSE_KERNEL_MINOR_VERSION, FUSE_KERNEL_VERSION, FUSE_ROOT_ID, FuseCompleteFn, FuseError,
    FuseFileHandle, FuseNodeId, FuseOperation, FuseStatus, FuseStatusError, MIN_MAX_WRITE,
    ops::{
        forget::{ForgetIn, ForgetOperation},
        init::{FuseInitFlags, FuseInitFlags2, InitIn, InitOperation},
        read::{ReadIn, ReadOperation},
        release::{ReleaseFlags, ReleaseIn, ReleaseKind, ReleaseOperation},
        write::{WriteIn, WriteOperation},
    },
};
use ostd::{info, warn};

use super::{super::DEVICE_NAME, FileSystemDevice, FuseReadBuf, FuseWaiter, FuseWriteBuf};
use crate::device::filesystem::pool::FuseDataBuf;

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
    fn wait_for_submitted_request(waiter: &FuseWaiter) -> Result<(), FuseError> {
        match waiter.wait() {
            FuseStatus::Complete | FuseStatus::Error(FuseStatusError::RemoteError) => Ok(()),
            FuseStatus::Error(FuseStatusError::Timeout) => Err(FuseError::Timeout),
            FuseStatus::Error(FuseStatusError::MalformedResponse) | FuseStatus::Pending => {
                Err(FuseError::MalformedResponse)
            }
        }
    }

    /// Creates a new FUSE session by performing `FUSE_INIT` negotiation with
    /// the daemon.
    pub fn new(device: Arc<FileSystemDevice>) -> Result<Arc<Self>, FuseError> {
        let requested_flags = Self::init_flags();
        let init_out = Self::do_fuse_op_on_device(
            &device,
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
        let session = Arc::new(Self {
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
            session.negotiated_flags.bits(),
            init_out.flags2().bits(),
            session.max_write,
            session.max_readahead,
            session.time_gran,
            session.max_pages,
            session.map_alignment,
        );

        Ok(session)
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

    fn do_fuse_op_on_device<Op: FuseOperation>(
        device: &Arc<FileSystemDevice>,
        nodeid: FuseNodeId,
        mut operation: Op,
    ) -> Result<Op::Output, FuseError> {
        let waiter = device.submit_fuse_op(nodeid, &mut operation, None, None)?;
        Self::wait_for_submitted_request(&waiter)?;
        waiter.read_reply(operation)
    }

    /// Sends one FUSE operation and waits for the typed reply.
    pub fn do_fuse_op<Op: FuseOperation>(
        &self,
        nodeid: FuseNodeId,
        operation: Op,
    ) -> Result<Op::Output, FuseError> {
        Self::do_fuse_op_on_device(&self.device, nodeid, operation)
    }

    /// Allocates a buffer for `FUSE_READ` data.
    pub fn alloc_read_buf(&self, size: usize) -> Result<FuseReadBuf, FuseError> {
        self.device
            .from_device_pool
            .alloc_fs_buf(size)
            .map_err(FuseError::ResourceAlloc)
    }

    /// Allocates a buffer for `FUSE_WRITE` data.
    pub fn alloc_write_buf(&self, size: usize) -> Result<FuseWriteBuf, FuseError> {
        self.device
            .to_device_pool
            .alloc_fs_buf(size)
            .map_err(FuseError::ResourceAlloc)
    }
}

impl FuseSession {
    /// Sends a `FUSE_FORGET` request on the high-priority queue.
    ///
    /// `FUSE_FORGET` is a no-reply request. The backend must not send a
    /// response, so this method only submits the request and never waits for
    /// completion. Local prepare or enqueue failures are logged and otherwise
    /// ignored because callers cannot observe a protocol-level error for
    /// `FUSE_FORGET`.
    pub fn forget(&self, nodeid: FuseNodeId, nlookup: u64) {
        if nodeid == FUSE_ROOT_ID || nlookup == 0 {
            return;
        }

        let mut operation = ForgetOperation::new(ForgetIn::new(nlookup));
        let request = match self
            .device
            .prepare_request(nodeid, &mut operation, None, None)
        {
            Ok(request) => Arc::new(request),
            Err(err) => {
                warn!(
                    "virtiofs forget failed to prepare inode {:?} with nlookup {}: {:?}",
                    nodeid, nlookup, err
                );
                return;
            }
        };
        self.device.submit(&self.device.hiprio_queue, request);
    }

    pub fn read(
        &self,
        nodeid: FuseNodeId,
        fh: FuseFileHandle,
        offset: u64,
        size: u32,
        flags: u32,
        data_buf: FuseReadBuf,
    ) -> Result<usize, FuseError> {
        let read_in = ReadIn::new(fh, offset, size, flags);
        let len = read_in.size() as usize;
        let mut operation = ReadOperation::new(read_in);
        let waiter = self.device.submit_fuse_op(
            nodeid,
            &mut operation,
            Some(FuseDataBuf::Read(data_buf)),
            None,
        )?;
        Self::wait_for_submitted_request(&waiter)?;

        let read_len = waiter.read_reply(operation)?;
        if read_len > len {
            return Err(FuseError::MalformedResponse);
        }

        Ok(read_len)
    }

    pub fn read_async(
        &self,
        nodeid: FuseNodeId,
        read_in: ReadIn,
        data_buf: FuseReadBuf,
        complete_fn: Option<FuseCompleteFn>,
    ) -> Result<FuseWaiter, FuseError> {
        let mut operation = ReadOperation::new(read_in);
        self.device.submit_fuse_op(
            nodeid,
            &mut operation,
            Some(FuseDataBuf::Read(data_buf)),
            complete_fn,
        )
    }

    pub fn write(
        &self,
        nodeid: FuseNodeId,
        write_in: WriteIn,
        data_buf: FuseWriteBuf,
    ) -> Result<usize, FuseError> {
        let write_size = write_in.size() as usize;
        let mut operation = WriteOperation::new(write_in);
        let waiter = self.device.submit_fuse_op(
            nodeid,
            &mut operation,
            Some(FuseDataBuf::Write(data_buf)),
            None,
        )?;
        Self::wait_for_submitted_request(&waiter)?;

        let write_out = waiter.read_reply(operation)?;
        if write_out.size() > write_size {
            return Err(FuseError::MalformedResponse);
        }

        Ok(write_out.size())
    }

    pub fn write_async(
        &self,
        nodeid: FuseNodeId,
        write_in: WriteIn,
        data_buf: FuseWriteBuf,
        complete_fn: Option<FuseCompleteFn>,
    ) -> Result<FuseWaiter, FuseError> {
        let mut operation = WriteOperation::new(write_in);
        self.device.submit_fuse_op(
            nodeid,
            &mut operation,
            Some(FuseDataBuf::Write(data_buf)),
            complete_fn,
        )
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
        if let Err(err) = self.do_fuse_op(
            nodeid,
            ReleaseOperation::new(ReleaseIn::new(fh, flags, release_flags), kind),
        ) {
            warn!("virtiofs release failed for inode {:?}: {:?}", nodeid, err);
        }
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
