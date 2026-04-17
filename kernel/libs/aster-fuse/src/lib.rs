// SPDX-License-Identifier: MPL-2.0 AND BSD-3-Clause
//
// This file is partially derived from the virtiofsd project:
// https://gitlab.com/virtio-fs/virtiofsd/-/blob/main/src/fuse.rs
//
// Original source:
// Copyright 2019 The Chromium OS Authors. All rights reserved.
// Licensed under the BSD-3-Clause license.
//
// Modifications made by The Asterinas Authors are licensed under MPL-2.0.
// Copyright 2026-present The Asterinas Authors.

//! Provides FUSE protocol definitions shared by in-kernel clients.
//!
//! This crate contains the transport-independent on-wire pieces of the FUSE
//! protocol: request and reply headers, payload layouts, opcodes, flags, and
//! common constants.
//!
//! The main entry points are:
//!
//! - [`FuseOperation`], which describes one typed FUSE request/reply pair.
//! - [`FuseError`] and [`FuseResult`], which report encoding and decoding
//!   failures.
//! - POD-compatible protocol structs such as [`InHeader`] and [`OutHeader`].
//! - Per-operation request and reply structs under [`mod@ops`].
#![no_std]
#![deny(unsafe_code)]

extern crate alloc;
#[macro_use]
extern crate ostd_pod;

mod error;
mod operation;
pub mod ops;

use alloc::string::String;
use core::sync::atomic::AtomicU64;

use int_to_c_enum::TryFromInt;

pub use self::{
    error::{FuseError, FuseResult},
    operation::FuseOperation,
    ops::{
        create::{CreateIn, CreateOperation},
        forget::{ForgetIn, ForgetOperation},
        getattr::{FuseAttrOut, GetattrFlags, GetattrIn, GetattrOperation},
        init::{FuseInitFlags, InitIn, InitOperation, InitOut},
        link::{LinkIn, LinkOperation},
        lookup::LookupOperation,
        lseek::{LseekIn, LseekOperation, LseekOut},
        mkdir::{MkdirIn, MkdirOperation},
        mknod::{MknodIn, MknodOperation},
        open::{FuseOpenFlags, OpenIn, OpenOperation, OpenOut, OpendirOperation},
        read::{ReadIn, ReadOperation},
        readdir::ReaddirOperation,
        readlink::ReadlinkOperation,
        release::{ReleaseFlags, ReleaseIn, ReleaseKind, ReleaseOperation},
        rmdir::RmdirOperation,
        setattr::{SetattrIn, SetattrOperation, SetattrValid},
        unlink::UnlinkOperation,
        write::{WriteFlags, WriteIn, WriteOperation, WriteOut},
    },
};

/// The root inode ID used by the FUSE protocol.
pub const FUSE_ROOT_ID: FuseNodeId = FuseNodeId::new(1);

/// The major FUSE protocol version supported by this crate.
pub const FUSE_KERNEL_VERSION: u32 = 7;

/// The minor FUSE protocol version supported by this crate.
pub const FUSE_KERNEL_MINOR_VERSION: u32 = 38;

/// Minimum `max_write` value enforced by the client.
///
/// Even if the daemon reports a smaller `max_write` in `FUSE_INIT`, the client
/// uses at least one page (4096 bytes) per write request.
pub const MIN_MAX_WRITE: u32 = 4096;

/// An opaque FUSE file handle issued by the server.
///
/// The server returns this handle in `FUSE_OPEN` and `FUSE_OPENDIR` replies.
/// Subsequent I/O and release requests carry it so the backend can locate the
/// corresponding open-file state.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Pod, Default)]
pub struct FuseFileHandle(u64);

impl FuseFileHandle {
    pub const fn new(fh: u64) -> Self {
        Self(fh)
    }
}

/// A FUSE inode identifier (`nodeid`) exchanged on the protocol.
///
/// A `FuseNodeId` identifies one server-side inode object. It appears in FUSE
/// request headers and in lookup-like replies such as [`EntryOut`].
///
/// For a cached VFS inode, this value is an immutable identity binding: if
/// lookup revalidation later resolves the same pathname to a different
/// `FuseNodeId`, the old cached inode must be treated as stale and replaced
/// instead of being retargeted in place.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Pod)]
pub struct FuseNodeId(u64);

impl FuseNodeId {
    pub const fn new(nodeid: u64) -> Self {
        Self(nodeid)
    }

    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

/// Client-side mirror of the server's nlookup for an inode.
///
/// In the FUSE protocol, every reply that contains an [`EntryOut`] increments
/// the server-side nlookup by one. This includes `FUSE_LOOKUP`, `FUSE_CREATE`,
/// `FUSE_MKDIR`, `FUSE_MKNOD`, and `FUSE_LINK`. This type tracks the same
/// count on the client side so that the accumulated value can be sent back
/// via `FUSE_FORGET` when the inode is dropped.
#[repr(transparent)]
pub struct LookupCount(core::sync::atomic::AtomicU64);

impl Default for LookupCount {
    fn default() -> Self {
        Self(AtomicU64::new(1))
    }
}

impl LookupCount {
    /// Increases the lookup count by one.
    pub fn increase(&self) {
        self.0.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    }

    /// Returns the current lookup count.
    pub fn get(&self) -> u64 {
        self.0.load(core::sync::atomic::Ordering::Relaxed)
    }
}

/// The common header of a FUSE request.
///
/// Every request sent to a FUSE server starts with this header. The payload
/// that follows is determined by [`InHeader::opcode`].
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod)]
pub struct InHeader {
    /// Total request length in bytes, including this header.
    pub len: u32,
    /// Operation code identifying the request payload format.
    pub opcode: u32,
    /// Request identifier copied into the matching [`OutHeader`].
    pub unique: u64,
    /// Target inode of the request.
    pub nodeid: FuseNodeId,
    /// User ID of the requesting process.
    pub uid: u32,
    /// Group ID of the requesting process.
    pub gid: u32,
    /// Process ID of the requesting process.
    pub pid: u32,
    /// Total length of extension headers that follow this header.
    pub total_extlen: u16,
    pub padding: u16,
}

impl InHeader {
    /// Creates an `InHeader` with the provided core fields.
    pub const fn new(len: u32, opcode: u32, unique: u64, nodeid: FuseNodeId) -> Self {
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

/// The common header of a FUSE reply.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod)]
pub struct OutHeader {
    /// Total reply length in bytes, including this header.
    pub len: u32,
    /// Operation result as `0` on success or a negated errno on failure.
    pub error: i32,
    /// Request identifier copied from the matching [`InHeader`].
    pub unique: u64,
}

impl OutHeader {
    pub const fn new(len: u32, error: i32, unique: u64) -> Self {
        Self { len, error, unique }
    }

    /// Returns an empty [`OutHeader`].
    pub const fn empty() -> Self {
        Self::new(0, 0, 0)
    }
}

/// FUSE inode attributes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod)]
pub struct Attr {
    /// Inode number.
    pub ino: u64,
    /// File size in bytes.
    pub size: u64,
    /// Number of allocated blocks.
    pub blocks: u64,
    /// Last access time in seconds since the Unix epoch.
    pub atime: u64,
    /// Last modification time in seconds since the Unix epoch.
    pub mtime: u64,
    /// Last status-change time in seconds since the Unix epoch.
    pub ctime: u64,
    /// Nanosecond component of [`Attr::atime`].
    pub atimensec: u32,
    /// Nanosecond component of [`Attr::mtime`].
    pub mtimensec: u32,
    /// Nanosecond component of [`Attr::ctime`].
    pub ctimensec: u32,
    /// File type and permission bits.
    pub mode: u32,
    /// Number of hard links.
    pub nlink: u32,
    /// Owner user ID.
    pub uid: u32,
    /// Owner group ID.
    pub gid: u32,
    /// Device number for special files.
    pub rdev: u32,
    /// Preferred block size for I/O.
    pub blksize: u32,
    pub padding: u32,
}

/// The reply payload for lookup-like FUSE operations.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod)]
pub struct EntryOut {
    /// Node ID assigned to the resolved inode.
    pub nodeid: FuseNodeId,
    /// Generation number used to distinguish reused inode numbers.
    pub generation: u64,
    /// Entry-cache timeout in seconds.
    pub entry_valid: u64,
    /// Attribute-cache timeout in seconds.
    pub attr_valid: u64,
    /// Nanosecond component of [`EntryOut::entry_valid`].
    pub entry_valid_nsec: u32,
    /// Nanosecond component of [`EntryOut::attr_valid`].
    pub attr_valid_nsec: u32,
    /// Attributes of the resolved inode.
    pub attr: Attr,
}

/// A raw directory entry in a `FUSE_READDIR` reply.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod)]
pub struct Dirent {
    /// Inode number of the directory entry.
    pub ino: u64,
    /// Offset cookie for continuing directory iteration.
    pub off: u64,
    /// Length of the entry name in bytes.
    pub namelen: u32,
    /// POSIX directory entry type.
    pub typ: u32,
}

/// A directory entry decoded from a `FUSE_READDIR` reply.
#[derive(Debug, Clone)]
pub struct FuseDirEntry {
    ino: u64,
    offset: u64,
    type_: DirentType,
    name: String,
}

impl FuseDirEntry {
    pub fn new(ino: u64, offset: u64, type_: DirentType, name: String) -> Self {
        Self {
            ino,
            offset,
            type_,
            name,
        }
    }

    /// Returns the inode number of this directory entry.
    pub fn ino(&self) -> u64 {
        self.ino
    }

    /// Returns the offset cookie for continuing directory iteration.
    pub fn offset(&self) -> u64 {
        self.offset
    }

    /// Returns the POSIX directory entry type.
    pub fn type_(&self) -> DirentType {
        self.type_
    }

    /// Returns the entry name.
    pub fn name(&self) -> &str {
        &self.name
    }
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
