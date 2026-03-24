// SPDX-License-Identifier: MPL-2.0

//! 9P2000.L protocol definitions.
//!
//! Reference: <https://github.com/chaos/diod/blob/master/protocol.md>

use alloc::{
    string::{String, ToString},
    vec,
    vec::Vec,
};

/// 9P protocol version string.
pub const P9_PROTO_VERSION: &str = "9P2000.L";

/// Special tag value meaning "no tag".
pub const P9_NOTAG: u16 = 0xFFFF;

/// Special FID value meaning "no fid".
pub const P9_NOFID: u32 = 0xFFFFFFFF;

/// Default maximum message size.
pub const DEFAULT_MSIZE: u32 = 8192;

/// Size of 9P message header (size[4] + type[1] + tag[2]).
pub const P9_HEADER_SIZE: usize = 7;

// 9P2000.L message types (T = request, R = response).
pub const P9_TLERROR: u8 = 6;
pub const P9_RLERROR: u8 = 7;
pub const P9_TSTATFS: u8 = 8;
pub const P9_RSTATFS: u8 = 9;
pub const P9_TLOPEN: u8 = 12;
pub const P9_RLOPEN: u8 = 13;
pub const P9_TLCREATE: u8 = 14;
pub const P9_RLCREATE: u8 = 15;
pub const P9_TSYMLINK: u8 = 16;
pub const P9_RSYMLINK: u8 = 17;
pub const P9_TMKNOD: u8 = 18;
pub const P9_RMKNOD: u8 = 19;
pub const P9_TRENAME: u8 = 20;
pub const P9_RRENAME: u8 = 21;
pub const P9_TREADLINK: u8 = 22;
pub const P9_RREADLINK: u8 = 23;
pub const P9_TGETATTR: u8 = 24;
pub const P9_RGETATTR: u8 = 25;
pub const P9_TSETATTR: u8 = 26;
pub const P9_RSETATTR: u8 = 27;
pub const P9_TXATTRWALK: u8 = 30;
pub const P9_RXATTRWALK: u8 = 31;
pub const P9_TXATTRCREATE: u8 = 32;
pub const P9_RXATTRCREATE: u8 = 33;
pub const P9_TREADDIR: u8 = 40;
pub const P9_RREADDIR: u8 = 41;
pub const P9_TFSYNC: u8 = 50;
pub const P9_RFSYNC: u8 = 51;
pub const P9_TLOCK: u8 = 52;
pub const P9_RLOCK: u8 = 53;
pub const P9_TGETLOCK: u8 = 54;
pub const P9_RGETLOCK: u8 = 55;
pub const P9_TLINK: u8 = 70;
pub const P9_RLINK: u8 = 71;
pub const P9_TMKDIR: u8 = 72;
pub const P9_RMKDIR: u8 = 73;
pub const P9_TRENAMEAT: u8 = 74;
pub const P9_RRENAMEAT: u8 = 75;
pub const P9_TUNLINKAT: u8 = 76;
pub const P9_RUNLINKAT: u8 = 77;
pub const P9_TVERSION: u8 = 100;
pub const P9_RVERSION: u8 = 101;
pub const P9_TAUTH: u8 = 102;
pub const P9_RAUTH: u8 = 103;
pub const P9_TATTACH: u8 = 104;
pub const P9_RATTACH: u8 = 105;
pub const P9_TFLUSH: u8 = 108;
pub const P9_RFLUSH: u8 = 109;
pub const P9_TWALK: u8 = 110;
pub const P9_RWALK: u8 = 111;
pub const P9_TREAD: u8 = 116;
pub const P9_RREAD: u8 = 117;
pub const P9_TWRITE: u8 = 118;
pub const P9_RWRITE: u8 = 119;
pub const P9_TCLUNK: u8 = 120;
pub const P9_RCLUNK: u8 = 121;
pub const P9_TREMOVE: u8 = 122;
pub const P9_RREMOVE: u8 = 123;

// Getattr request mask bits.
pub const P9_GETATTR_MODE: u64 = 0x00000001;
pub const P9_GETATTR_NLINK: u64 = 0x00000002;
pub const P9_GETATTR_UID: u64 = 0x00000004;
pub const P9_GETATTR_GID: u64 = 0x00000008;
pub const P9_GETATTR_RDEV: u64 = 0x00000010;
pub const P9_GETATTR_ATIME: u64 = 0x00000020;
pub const P9_GETATTR_MTIME: u64 = 0x00000040;
pub const P9_GETATTR_CTIME: u64 = 0x00000080;
pub const P9_GETATTR_INO: u64 = 0x00000100;
pub const P9_GETATTR_SIZE: u64 = 0x00000200;
pub const P9_GETATTR_BLOCKS: u64 = 0x00000400;
pub const P9_GETATTR_BTIME: u64 = 0x00000800;
pub const P9_GETATTR_GEN: u64 = 0x00001000;
pub const P9_GETATTR_DATA_VERSION: u64 = 0x00002000;
pub const P9_GETATTR_BASIC: u64 = 0x000007ff;
pub const P9_GETATTR_ALL: u64 = 0x00003fff;

// Setattr valid bits.
pub const P9_SETATTR_MODE: u32 = 0x00000001;
pub const P9_SETATTR_UID: u32 = 0x00000002;
pub const P9_SETATTR_GID: u32 = 0x00000004;
pub const P9_SETATTR_SIZE: u32 = 0x00000008;
pub const P9_SETATTR_ATIME: u32 = 0x00000010;
pub const P9_SETATTR_MTIME: u32 = 0x00000020;
pub const P9_SETATTR_CTIME: u32 = 0x00000040;
pub const P9_SETATTR_ATIME_SET: u32 = 0x00000080;
pub const P9_SETATTR_MTIME_SET: u32 = 0x00000100;

// AT_REMOVEDIR for unlinkat.
pub const AT_REMOVEDIR: u32 = 0x200;

/// QID type bits.
pub const QID_TYPE_DIR: u8 = 0x80;
pub const QID_TYPE_APPEND: u8 = 0x40;
pub const QID_TYPE_EXCL: u8 = 0x20;
pub const QID_TYPE_MOUNT: u8 = 0x10;
pub const QID_TYPE_AUTH: u8 = 0x08;
pub const QID_TYPE_TMP: u8 = 0x04;
pub const QID_TYPE_SYMLINK: u8 = 0x02;
pub const QID_TYPE_FILE: u8 = 0x00;

/// 9P QID — unique server-side file identifier.
#[derive(Debug, Clone, Copy, Default)]
pub struct P9Qid {
    pub type_: u8,
    pub version: u32,
    pub path: u64,
}

impl P9Qid {
    pub fn decode(buf: &[u8]) -> Option<(Self, usize)> {
        if buf.len() < 13 {
            return None;
        }
        let type_ = buf[0];
        let version = u32::from_le_bytes([buf[1], buf[2], buf[3], buf[4]]);
        let path = u64::from_le_bytes([
            buf[5], buf[6], buf[7], buf[8], buf[9], buf[10], buf[11], buf[12],
        ]);
        Some((P9Qid { type_, version, path }, 13))
    }

    pub fn is_dir(&self) -> bool {
        self.type_ & QID_TYPE_DIR != 0
    }
}

/// 9P getattr response.
#[derive(Debug, Clone, Copy, Default)]
pub struct P9Attr {
    pub valid: u64,
    pub qid: P9Qid,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub nlink: u64,
    pub rdev: u64,
    pub size: u64,
    pub blksize: u64,
    pub blocks: u64,
    pub atime_sec: u64,
    pub atime_nsec: u64,
    pub mtime_sec: u64,
    pub mtime_nsec: u64,
    pub ctime_sec: u64,
    pub ctime_nsec: u64,
    pub btime_sec: u64,
    pub btime_nsec: u64,
    pub gen: u64,
    pub data_version: u64,
}

impl P9Attr {
    pub fn decode(buf: &[u8]) -> Option<(Self, usize)> {
        if buf.len() < 160 {
            return None;
        }
        let valid = read_u64_le(buf, 0);
        let (qid, _) = P9Qid::decode(&buf[8..])?;
        let mode = read_u32_le(buf, 21);
        let uid = read_u32_le(buf, 25);
        let gid = read_u32_le(buf, 29);
        let nlink = read_u64_le(buf, 33);
        let rdev = read_u64_le(buf, 41);
        let size = read_u64_le(buf, 49);
        let blksize = read_u64_le(buf, 57);
        let blocks = read_u64_le(buf, 65);
        let atime_sec = read_u64_le(buf, 73);
        let atime_nsec = read_u64_le(buf, 81);
        let mtime_sec = read_u64_le(buf, 89);
        let mtime_nsec = read_u64_le(buf, 97);
        let ctime_sec = read_u64_le(buf, 105);
        let ctime_nsec = read_u64_le(buf, 113);
        let btime_sec = read_u64_le(buf, 121);
        let btime_nsec = read_u64_le(buf, 129);
        let gen = read_u64_le(buf, 137);
        let data_version = read_u64_le(buf, 145);
        Some((
            P9Attr {
                valid,
                qid,
                mode,
                uid,
                gid,
                nlink,
                rdev,
                size,
                blksize,
                blocks,
                atime_sec,
                atime_nsec,
                mtime_sec,
                mtime_nsec,
                ctime_sec,
                ctime_nsec,
                btime_sec,
                btime_nsec,
                gen,
                data_version,
            },
            160,
        ))
    }
}

/// 9P statfs response.
#[derive(Debug, Clone, Copy, Default)]
pub struct P9StatFs {
    pub fs_type: u32,
    pub bsize: u32,
    pub blocks: u64,
    pub bfree: u64,
    pub bavail: u64,
    pub files: u64,
    pub ffree: u64,
    pub fsid: u64,
    pub namelen: u32,
}

impl P9StatFs {
    pub fn decode(buf: &[u8]) -> Option<Self> {
        if buf.len() < 60 {
            return None;
        }
        Some(P9StatFs {
            fs_type: read_u32_le(buf, 0),
            bsize: read_u32_le(buf, 4),
            blocks: read_u64_le(buf, 8),
            bfree: read_u64_le(buf, 16),
            bavail: read_u64_le(buf, 24),
            files: read_u64_le(buf, 32),
            ffree: read_u64_le(buf, 40),
            fsid: read_u64_le(buf, 48),
            namelen: read_u32_le(buf, 56),
        })
    }
}

/// 9P directory entry from Rreaddir.
#[derive(Debug, Clone)]
pub struct P9DirEntry {
    pub qid: P9Qid,
    pub offset: u64,
    pub type_: u8,
    pub name: String,
}

/// Helper: encode a 9P string (len[2] + bytes) into a buffer.
pub fn encode_string(buf: &mut Vec<u8>, s: &str) {
    let len = s.len() as u16;
    buf.extend_from_slice(&len.to_le_bytes());
    buf.extend_from_slice(s.as_bytes());
}

/// Helper: decode a 9P string from a buffer, returning (string, bytes_consumed).
pub fn decode_string(buf: &[u8]) -> Option<(String, usize)> {
    if buf.len() < 2 {
        return None;
    }
    let len = u16::from_le_bytes([buf[0], buf[1]]) as usize;
    if buf.len() < 2 + len {
        return None;
    }
    let s = core::str::from_utf8(&buf[2..2 + len])
        .ok()?
        .to_string();
    Some((s, 2 + len))
}

/// Build a 9P message with header (size[4] + type[1] + tag[2]) + body.
pub fn build_message(msg_type: u8, tag: u16, body: &[u8]) -> Vec<u8> {
    let size = (P9_HEADER_SIZE + body.len()) as u32;
    let mut msg = Vec::with_capacity(size as usize);
    msg.extend_from_slice(&size.to_le_bytes());
    msg.push(msg_type);
    msg.extend_from_slice(&tag.to_le_bytes());
    msg.extend_from_slice(body);
    msg
}

/// Parse a 9P response header: returns (size, type, tag).
pub fn parse_header(buf: &[u8]) -> Option<(u32, u8, u16)> {
    if buf.len() < P9_HEADER_SIZE {
        return None;
    }
    let size = read_u32_le(buf, 0);
    let msg_type = buf[4];
    let tag = u16::from_le_bytes([buf[5], buf[6]]);
    Some((size, msg_type, tag))
}

/// Check if a response is an Rlerror. Returns Some(errno) if it is.
pub fn check_rlerror(buf: &[u8]) -> Option<u32> {
    let (_, msg_type, _) = parse_header(buf)?;
    if msg_type == P9_RLERROR && buf.len() >= P9_HEADER_SIZE + 4 {
        Some(read_u32_le(buf, P9_HEADER_SIZE))
    } else {
        None
    }
}

/// Parse directory entries from Rreaddir payload.
pub fn parse_readdir_entries(data: &[u8]) -> Vec<P9DirEntry> {
    let mut entries = Vec::new();
    let mut pos = 0;

    while pos < data.len() {
        // Each entry: qid[13] + offset[8] + type[1] + name[s]
        if pos + 13 + 8 + 1 + 2 > data.len() {
            break;
        }
        let (qid, _) = match P9Qid::decode(&data[pos..]) {
            Some(q) => q,
            None => break,
        };
        pos += 13;
        let offset = read_u64_le(data, pos);
        pos += 8;
        let type_ = data[pos];
        pos += 1;
        let (name, consumed) = match decode_string(&data[pos..]) {
            Some(s) => s,
            None => break,
        };
        pos += consumed;

        entries.push(P9DirEntry {
            qid,
            offset,
            type_,
            name,
        });
    }

    entries
}

// Inline LE read helpers.
#[inline]
pub fn read_u16_le(buf: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([buf[offset], buf[offset + 1]])
}

#[inline]
pub fn read_u32_le(buf: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        buf[offset],
        buf[offset + 1],
        buf[offset + 2],
        buf[offset + 3],
    ])
}

#[inline]
pub fn read_u64_le(buf: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes([
        buf[offset],
        buf[offset + 1],
        buf[offset + 2],
        buf[offset + 3],
        buf[offset + 4],
        buf[offset + 5],
        buf[offset + 6],
        buf[offset + 7],
    ])
}
