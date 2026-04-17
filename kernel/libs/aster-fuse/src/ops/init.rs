// SPDX-License-Identifier: MPL-2.0

//! `FUSE_INIT` negotiates the protocol version and capabilities for a FUSE
//! connection.
//!
//! The request body contains [`InitIn`] with the client-supported version,
//! readahead size, and feature flags. The reply body contains [`InitOut`] with
//! the server-selected version, limits, and negotiated flags.

use core::mem::size_of;

use bitflags::bitflags;
use ostd::mm::{Infallible, VmReader, VmWriter};

use crate::{FuseError, FuseOpcode, FuseOperation, FuseResult};

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod)]
pub struct InitIn {
    /// Major version of the FUSE protocol supported by the client.
    pub major: u32,
    /// Minor version of the FUSE protocol supported by the client.
    pub minor: u32,
    /// Maximum readahead size requested by the client.
    pub max_readahead: u32,
    /// Lower 32 bits of supported client capabilities.
    pub flags: FuseInitFlags,
    // The following fields are extensions.
    /// Upper 32 bits of supported client capabilities.
    pub flags2: FuseInitFlags,
    pub unused: [u32; 11],
}

impl InitIn {
    pub const fn new(
        major: u32,
        minor: u32,
        max_readahead: u32,
        flags: FuseInitFlags,
        flags2: FuseInitFlags,
    ) -> Self {
        Self {
            major,
            minor,
            max_readahead,
            flags,
            flags2,
            unused: [0; 11],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod)]
pub struct InitOut {
    /// Major version of the FUSE protocol selected by the server.
    pub major: u32,
    /// Minor version of the FUSE protocol selected by the server.
    pub minor: u32,
    /// Maximum readahead size accepted by the server.
    pub max_readahead: u32,
    /// Lower 32 bits of negotiated capabilities.
    pub flags: u32,
    /// Maximum number of background requests.
    pub max_background: u16,
    /// Background request threshold for congestion.
    pub congestion_threshold: u16,
    /// Maximum write size accepted by the server.
    pub max_write: u32,
    /// Timestamp granularity in nanoseconds.
    pub time_gran: u32,
    /// Maximum number of pages in a request.
    pub max_pages: u16,
    /// Mapping alignment requirement as a power-of-two page count.
    pub map_alignment: u16,
    /// Upper 32 bits of negotiated capabilities.
    pub flags2: u32,
    /// Maximum stack depth for passthrough operations.
    pub max_stack_depth: u32,
    /// Request timeout in seconds.
    pub request_timeout: u16,
    pub unused: [u16; 11],
}

bitflags! {
    /// FUSE capability and feature flags exchanged in `FUSE_INIT`.
    ///
    /// The client sends its supported set in [`InitIn::flags`]; the server
    /// responds with the subset it also supports in [`InitOut::flags`].
    #[repr(C)]
    #[derive(Pod)]
    pub struct FuseInitFlags: u32 {
        /// Supports asynchronous reads.
        const ASYNC_READ          = 1 << 0;
        /// Supports POSIX byte-range locks.
        const POSIX_LOCKS         = 1 << 1;
        /// Uses file-handle based operations.
        const FILE_OPS            = 1 << 2;
        /// Supports atomic `O_TRUNC` handling during open.
        const ATOMIC_O_TRUNC      = 1 << 3;
        /// Supports stable inode numbers for export.
        const EXPORT_SUPPORT      = 1 << 4;
        /// Supports writes larger than 4 KiB.
        const BIG_WRITES          = 1 << 5;
        /// Preserves mode bits instead of applying the process umask.
        const DONT_MASK           = 1 << 6;
        /// Supports splice-based writes.
        const SPLICE_WRITE        = 1 << 7;
        /// Supports splice move optimization.
        const SPLICE_MOVE         = 1 << 8;
        /// Supports splice-based reads.
        const SPLICE_READ         = 1 << 9;
        /// Supports BSD-style flock locks.
        const FLOCK_LOCKS         = 1 << 10;
        /// Supports ioctl requests on directories.
        const HAS_IOCTL_DIR       = 1 << 11;
        /// Invalidates cached file data automatically on attribute changes.
        const AUTO_INVAL_DATA     = 1 << 12;
        /// Supports `FUSE_READDIRPLUS`.
        const DO_READDIRPLUS      = 1 << 13;
        /// Allows the server to choose when to use `FUSE_READDIRPLUS`.
        const READDIRPLUS_AUTO    = 1 << 14;
        /// Supports asynchronous direct I/O.
        const ASYNC_DIO           = 1 << 15;
        /// Supports writeback caching.
        const WRITEBACK_CACHE     = 1 << 16;
        /// Allows `ENOSYS` from `FUSE_OPEN` to mean open is unsupported.
        const NO_OPEN_SUPPORT     = 1 << 17;
        /// Supports parallel directory operations.
        const PARALLEL_DIROPS     = 1 << 18;
        /// Lets the server clear privilege bits after writes and truncates.
        const HANDLE_KILLPRIV     = 1 << 19;
        /// Supports POSIX ACLs.
        const POSIX_ACL           = 1 << 20;
        /// Supports returning an error from abort handling.
        const ABORT_ERROR         = 1 << 21;
        /// Supports the `max_pages` field in [`InitOut`].
        const MAX_PAGES           = 1 << 22;
        /// Supports caching symbolic-link targets.
        const CACHE_SYMLINKS      = 1 << 23;
        /// Allows `ENOSYS` from `FUSE_OPENDIR` to mean opendir is unsupported.
        const NO_OPENDIR_SUPPORT  = 1 << 24;
        /// Supports explicit file-data invalidation.
        const EXPLICIT_INVAL_DATA = 1 << 25;
        /// Supports the `map_alignment` field in [`InitOut`].
        const MAP_ALIGNMENT       = 1 << 26;
        /// Supports submounts.
        const SUBMOUNTS           = 1 << 27;
        /// Supports the version-2 privilege-bit clearing protocol.
        const HANDLE_KILLPRIV_V2  = 1 << 28;
        /// Supports extended setxattr requests.
        const SETXATTR_EXT        = 1 << 29;
        /// Supports extended `FUSE_INIT` fields.
        const INIT_EXT            = 1 << 30;
        /// Supports security context information.
        const SECURITY_CTX        = 1 << 31;
    }
}

pub struct InitOperation {
    init_in: InitIn,
}

impl InitOperation {
    pub fn new(init_in: InitIn) -> Self {
        Self { init_in }
    }
}

impl FuseOperation for InitOperation {
    type Output = InitOut;

    fn opcode(&self) -> FuseOpcode {
        FuseOpcode::Init
    }

    fn body_len(&self) -> usize {
        size_of::<InitIn>()
    }

    fn write_body(&mut self, writer: &mut VmWriter<'_, Infallible>) -> FuseResult<()> {
        writer
            .write_val(&self.init_in)
            .map_err(|_| FuseError::BufferTooSmall)
    }

    fn out_payload_size(&self) -> Option<usize> {
        Some(size_of::<InitOut>())
    }

    fn parse_reply(
        self,
        _payload_len: usize,
        reader: &mut VmReader<'_, Infallible>,
    ) -> FuseResult<Self::Output> {
        reader.read_val().map_err(|_| FuseError::BufferTooSmall)
    }
}
