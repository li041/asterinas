// SPDX-License-Identifier: MPL-2.0
//
// This file is partially derived from the virtiofsd project:
// https://gitlab.com/virtio-fs/virtiofsd/-/blob/main/src/fuse.rs
//
// Original copyright:
//
// Copyright 2019 The Chromium OS Authors.
// Use of this source code is governed by a BSD-style license
// that can be found in the LICENSE file.
//

//! FUSE protocol definitions shared by in-kernel clients.
//!
//! This crate provides strongly-typed layouts for FUSE request/response headers,
//! payloads, opcodes, and protocol constants.
#![no_std]
#![deny(unsafe_code)]

#[macro_use]
extern crate ostd_pod;

mod error;
mod operation;

use bitflags::bitflags;
use int_to_c_enum::TryFromInt;

pub use self::{
    error::{FuseError, FuseResult},
    operation::FuseOperation,
};

/// An opaque FUSE file handle issued by the server on `FUSE_OPEN` /
/// `FUSE_OPENDIR`. Every subsequent I/O and release request carries it so the
/// backend can locate the corresponding open-file state.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Pod)]
pub struct FuseFileHandle(pub u64);

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod)]
pub struct InHeader {
    pub len: u32,
    pub opcode: u32,
    pub unique: u64,
    pub nodeid: u64,
    pub uid: u32,
    pub gid: u32,
    pub pid: u32,
    pub total_extlen: u16, // length of extensions in 8-byte units
    pub padding: u16,
}

impl InHeader {
    pub const fn new(len: u32, opcode: u32, unique: u64, nodeid: u64) -> Self {
        Self {
            len,
            opcode,
            unique,
            nodeid,
            uid: 0,
            gid: 0,
            pid: 0,
            total_extlen: 0,
            padding: 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod)]
pub struct OutHeader {
    pub len: u32,
    pub error: i32,
    pub unique: u64,
}

impl OutHeader {
    pub const fn new(len: u32, error: i32, unique: u64) -> Self {
        Self { len, error, unique }
    }

    pub const fn empty() -> Self {
        Self::new(0, 0, 0)
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod)]
pub struct InitIn {
    pub major: u32,
    pub minor: u32,
    pub max_readahead: u32,
    pub flags: FuseInitFlags,
    // The following fields are extensions.
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
    pub major: u32,
    pub minor: u32,
    pub max_readahead: u32,
    pub flags: u32,
    pub max_background: u16,
    pub congestion_threshold: u16,
    pub max_write: u32,
    pub time_gran: u32,
    pub max_pages: u16,
    pub map_alignment: u16,
    pub flags2: u32,
    pub max_stack_depth: u32,
    pub request_timeout: u16,
    pub unused: [u16; 11],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod)]
pub struct OpenIn {
    pub flags: u32,
    pub open_flags: OpenFlags,
}

impl OpenIn {
    pub const fn new(flags: u32) -> Self {
        Self {
            flags,
            open_flags: OpenFlags::empty(),
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod)]
pub struct ReleaseIn {
    pub fh: FuseFileHandle,
    pub flags: u32,
    pub release_flags: u32,
    pub lock_owner: u64,
}

impl ReleaseIn {
    pub const fn new(fh: FuseFileHandle, flags: u32) -> Self {
        Self {
            fh,
            flags,
            release_flags: 0,
            lock_owner: 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod)]
pub struct LseekIn {
    pub fh: FuseFileHandle,
    pub offset: i64,
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

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod)]
pub struct OpenOut {
    pub fh: FuseFileHandle,
    pub open_flags: OpenFlags,
    pub padding: u32,
}

bitflags! {
    #[repr(C)]
    #[derive(Pod)]
    pub struct OpenFlags: u32 {
        const FOPEN_DIRECT_IO = 1 << 0;
        const FOPEN_KEEP_CACHE = 1 << 1;
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod)]
pub struct GetattrIn {
    pub getattr_flags: u32,
    pub dummy: u32,
    pub fh: FuseFileHandle,
}

bitflags! {
    #[repr(C)]
    #[derive(Pod)]
    pub struct SetattrValid: u32 {
        const FATTR_MODE = 1 << 0;
        const FATTR_UID = 1 << 1;
        const FATTR_GID = 1 << 2;
        const FATTR_SIZE = 1 << 3;
        const FATTR_ATIME = 1 << 4;
        const FATTR_MTIME = 1 << 5;
        const FATTR_FH = 1 << 6;
        const FATTR_ATIME_NOW = 1 << 7;
        const FATTR_MTIME_NOW = 1 << 8;
        const FATTR_LOCKOWNER = 1 << 9;
        const FATTR_CTIME = 1 << 10;
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod)]
pub struct SetattrIn {
    pub valid: SetattrValid,
    pub padding: u32,
    pub fh: FuseFileHandle,
    pub size: u64,
    pub lock_owner: u64,
    pub atime: u64,
    pub mtime: u64,
    pub ctime: u64,
    pub atimensec: u32,
    pub mtimensec: u32,
    pub ctimensec: u32,
    pub mode: u32,
    pub unused4: u32,
    pub uid: u32,
    pub gid: u32,
    pub unused5: u32,
}

impl Default for SetattrIn {
    fn default() -> Self {
        Self {
            valid: SetattrValid::empty(),
            padding: 0,
            fh: FuseFileHandle(0),
            size: 0,
            lock_owner: 0,
            atime: 0,
            mtime: 0,
            ctime: 0,
            atimensec: 0,
            mtimensec: 0,
            ctimensec: 0,
            mode: 0,
            unused4: 0,
            uid: 0,
            gid: 0,
            unused5: 0,
        }
    }
}

impl GetattrIn {
    pub const fn new(fh: FuseFileHandle) -> Self {
        Self {
            getattr_flags: 0,
            dummy: 0,
            fh,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod)]
pub struct FuseAttrOut {
    pub attr_valid: u64,
    pub attr_valid_nsec: u32,
    pub dummy: u32,
    pub attr: Attr,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod)]
pub struct ReadIn {
    pub fh: FuseFileHandle,
    pub offset: u64,
    pub size: u32,
    pub read_flags: u32,
    pub lock_owner: u64,
    pub flags: u32,
    pub padding: u32,
}

impl ReadIn {
    pub const fn new(fh: FuseFileHandle, offset: u64, size: u32) -> Self {
        Self {
            fh,
            offset,
            size,
            read_flags: 0,
            lock_owner: 0,
            flags: 0,
            padding: 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod)]
pub struct WriteIn {
    pub fh: FuseFileHandle,
    pub offset: u64,
    pub size: u32,
    pub write_flags: u32,
    pub lock_owner: u64,
    pub flags: u32,
    pub padding: u32,
}

impl WriteIn {
    pub const fn new(fh: FuseFileHandle, offset: u64, size: u32) -> Self {
        Self {
            fh,
            offset,
            size,
            write_flags: 0,
            lock_owner: 0,
            flags: 0,
            padding: 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod)]
pub struct WriteOut {
    pub size: u32,
    pub padding: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod)]
pub struct CreateIn {
    pub flags: u32,
    pub mode: u32,
    pub umask: u32,
    pub open_flags: OpenFlags,
}

impl CreateIn {
    pub const fn new(flags: u32, mode: u32) -> Self {
        Self {
            flags,
            mode,
            umask: 0,
            open_flags: OpenFlags::empty(),
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod)]
pub struct MkdirIn {
    pub mode: u32,
    pub umask: u32,
}

impl MkdirIn {
    pub const fn new(mode: u32) -> Self {
        Self { mode, umask: 0 }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod)]
pub struct MknodIn {
    pub mode: u32,
    pub rdev: u32,
    pub umask: u32,
    pub padding: u32,
}

impl MknodIn {
    pub const fn new(mode: u32, rdev: u32) -> Self {
        Self {
            mode,
            rdev,
            umask: 0,
            padding: 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod)]
pub struct ForgetIn {
    pub nlookup: u64,
}

impl ForgetIn {
    pub const fn new(nlookup: u64) -> Self {
        Self { nlookup }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod)]
pub struct Attr {
    pub ino: u64,
    pub size: u64,
    pub blocks: u64,
    pub atime: u64,
    pub mtime: u64,
    pub ctime: u64,
    pub atimensec: u32,
    pub mtimensec: u32,
    pub ctimensec: u32,
    pub mode: u32,
    pub nlink: u32,
    pub uid: u32,
    pub gid: u32,
    pub rdev: u32,
    pub blksize: u32,
    pub padding: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod)]
pub struct EntryOut {
    pub nodeid: u64,
    pub generation: u64,
    pub entry_valid: u64,
    pub attr_valid: u64,
    pub entry_valid_nsec: u32,
    pub attr_valid_nsec: u32,
    pub attr: Attr,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod)]
pub struct Dirent {
    pub ino: u64,
    pub off: u64,
    pub namelen: u32,
    pub typ: u32,
}

/// POSIX `d_type` values carried in [`Dirent::typ`].
///
/// See <https://www.man7.org/linux/man-pages/man3/readdir.3.html>.
#[expect(non_camel_case_types)]
#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, TryFromInt)]
pub enum DirentType {
    DT_UNKNOWN = 0,
    DT_FIFO = 1,
    DT_CHR = 2,
    DT_DIR = 4,
    DT_BLK = 6,
    DT_REG = 8,
    DT_LNK = 10,
    DT_SOCK = 12,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, TryFromInt)]
pub enum FuseOpcode {
    Lookup = 1,
    Forget = 2,
    Getattr = 3,
    Setattr = 4,
    Readlink = 5,
    Symlink = 6,
    Mknod = 8,
    Mkdir = 9,
    Unlink = 10,
    Rmdir = 11,
    Rename = 12,
    Link = 13,
    Open = 14,
    Read = 15,
    Write = 16,
    Statfs = 17,
    Release = 18,
    Fsync = 20,
    Setxattr = 21,
    Getxattr = 22,
    Listxattr = 23,
    Removexattr = 24,
    Flush = 25,
    Init = 26,
    Opendir = 27,
    Readdir = 28,
    Releasedir = 29,
    Fsyncdir = 30,
    Getlk = 31,
    Setlk = 32,
    Setlkw = 33,
    Access = 34,
    Create = 35,
    Interrupt = 36,
    Bmap = 37,
    Destroy = 38,
    Ioctl = 39,
    Poll = 40,
    NotifyReply = 41,
    BatchForget = 42,
    Fallocate = 43,
    Readdirplus = 44,
    Rename2 = 45,
    Lseek = 46,
    CopyFileRange = 47,
    SetupMapping = 48,
    RemoveMapping = 49,
    SyncFs = 50,
    Tmpfile = 51,
}

impl From<FuseOpcode> for u32 {
    fn from(opcode: FuseOpcode) -> u32 {
        opcode as u32
    }
}

pub const FUSE_ROOT_ID: u64 = 1;

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod)]
pub struct LinkIn {
    pub oldnodeid: u64,
}

impl LinkIn {
    pub const fn new(oldnodeid: u64) -> Self {
        Self { oldnodeid }
    }
}

pub const FUSE_KERNEL_VERSION: u32 = 7;
pub const FUSE_KERNEL_MINOR_VERSION: u32 = 38;

bitflags! {
    /// FUSE capability and feature flags exchanged in `FUSE_INIT`.
    ///
    /// The client sends its supported set in [`InitIn::flags`]; the server
    /// responds with the subset it also supports in [`InitOut::flags`].
    #[repr(C)]
    #[derive(Pod)]
    pub struct FuseInitFlags: u32 {
        const ASYNC_READ          = 1 << 0;
        const POSIX_LOCKS         = 1 << 1;
        const FILE_OPS            = 1 << 2;
        const ATOMIC_O_TRUNC      = 1 << 3;
        const EXPORT_SUPPORT      = 1 << 4;
        const BIG_WRITES          = 1 << 5;
        const DONT_MASK           = 1 << 6;
        const SPLICE_WRITE        = 1 << 7;
        const SPLICE_MOVE         = 1 << 8;
        const SPLICE_READ         = 1 << 9;
        const FLOCK_LOCKS         = 1 << 10;
        const HAS_IOCTL_DIR       = 1 << 11;
        const AUTO_INVAL_DATA     = 1 << 12;
        const DO_READDIRPLUS      = 1 << 13;
        const READDIRPLUS_AUTO    = 1 << 14;
        const ASYNC_DIO           = 1 << 15;
        const WRITEBACK_CACHE     = 1 << 16;
        const NO_OPEN_SUPPORT     = 1 << 17;
        const PARALLEL_DIROPS     = 1 << 18;
        const HANDLE_KILLPRIV     = 1 << 19;
        const POSIX_ACL           = 1 << 20;
        const ABORT_ERROR         = 1 << 21;
        const MAX_PAGES           = 1 << 22;
        const CACHE_SYMLINKS      = 1 << 23;
        const NO_OPENDIR_SUPPORT  = 1 << 24;
        const EXPLICIT_INVAL_DATA = 1 << 25;
        const MAP_ALIGNMENT       = 1 << 26;
        const SUBMOUNTS           = 1 << 27;
        const HANDLE_KILLPRIV_V2  = 1 << 28;
        const SETXATTR_EXT        = 1 << 29;
        const INIT_EXT            = 1 << 30;
        const SECURITY_CTX        = 1 << 31;
    }
}
