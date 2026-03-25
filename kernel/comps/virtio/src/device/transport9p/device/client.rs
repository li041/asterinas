// SPDX-License-Identifier: MPL-2.0

//! 9P2000.L protocol client operations.

use super::*;

impl Transport9PDevice {
    /// Tversion/Rversion — negotiate protocol version and msize.
    pub(crate) fn p9_version(&self) -> Result<(), VirtioDeviceError> {
        let msize = DEFAULT_MSIZE;
        let mut body = Vec::new();
        body.extend_from_slice(&msize.to_le_bytes());
        encode_string(&mut body, P9_PROTO_VERSION);

        let request = build_message(P9_TVERSION, P9_NOTAG, &body);
        let response = self.send_9p_request_early(&request, msize as usize)?;
        let resp_body = Self::check_9p_response(&response, P9_RVERSION)?;

        if resp_body.len() < 4 {
            return Err(VirtioDeviceError::QueueUnknownError);
        }
        let server_msize = read_u32_le(resp_body, 0);
        let negotiated_msize = cmp::min(msize, server_msize);
        self.msize.store(negotiated_msize, Ordering::Relaxed);

        let (version, _) =
            decode_string(&resp_body[4..]).ok_or(VirtioDeviceError::QueueUnknownError)?;

        info!(
            "{} version negotiated: version={}, msize={}",
            DEVICE_NAME, version, negotiated_msize
        );

        Ok(())
    }

    /// Tattach/Rattach — attach to the filesystem root.
    /// Returns (qid of root).
    pub fn p9_attach(
        &self,
        fid: u32,
        afid: u32,
        uname: &str,
        aname: &str,
        n_uname: u32,
    ) -> Result<P9Qid, VirtioDeviceError> {
        let mut body = Vec::new();
        body.extend_from_slice(&fid.to_le_bytes());
        body.extend_from_slice(&afid.to_le_bytes());
        encode_string(&mut body, uname);
        encode_string(&mut body, aname);
        body.extend_from_slice(&n_uname.to_le_bytes());

        let tag = self.alloc_tag_id();
        let request = build_message(P9_TATTACH, tag, &body);
        let msize = self.msize() as usize;
        let response = self.send_9p_request(&request, msize)?;
        let resp_body = Self::check_9p_response(&response, P9_RATTACH)?;

        let (qid, _) = P9Qid::decode(resp_body).ok_or(VirtioDeviceError::QueueUnknownError)?;

        Ok(qid)
    }

    /// Twalk/Rwalk — walk to a new path from fid, returning (newfid, list of qids).
    pub fn p9_walk(
        &self,
        fid: u32,
        newfid: u32,
        names: &[&str],
    ) -> Result<Vec<P9Qid>, VirtioDeviceError> {
        let mut body = Vec::new();
        body.extend_from_slice(&fid.to_le_bytes());
        body.extend_from_slice(&newfid.to_le_bytes());
        let nwname = names.len() as u16;
        body.extend_from_slice(&nwname.to_le_bytes());
        for name in names {
            encode_string(&mut body, name);
        }

        let tag = self.alloc_tag_id();
        let request = build_message(P9_TWALK, tag, &body);
        let msize = self.msize() as usize;
        let response = self.send_9p_request(&request, msize)?;
        let resp_body = Self::check_9p_response(&response, P9_RWALK)?;

        if resp_body.len() < 2 {
            return Err(VirtioDeviceError::QueueUnknownError);
        }
        let nwqid = read_u16_le(resp_body, 0) as usize;
        let mut qids = Vec::with_capacity(nwqid);
        let mut pos = 2;
        for _ in 0..nwqid {
            let (qid, consumed) =
                P9Qid::decode(&resp_body[pos..]).ok_or(VirtioDeviceError::QueueUnknownError)?;
            qids.push(qid);
            pos += consumed;
        }

        Ok(qids)
    }

    /// Tlopen/Rlopen — open a file by fid, returning (qid, iounit).
    pub fn p9_lopen(&self, fid: u32, flags: u32) -> Result<(P9Qid, u32), VirtioDeviceError> {
        let mut body = Vec::new();
        body.extend_from_slice(&fid.to_le_bytes());
        body.extend_from_slice(&flags.to_le_bytes());

        let tag = self.alloc_tag_id();
        let request = build_message(P9_TLOPEN, tag, &body);
        let msize = self.msize() as usize;
        let response = self.send_9p_request(&request, msize)?;
        let resp_body = Self::check_9p_response(&response, P9_RLOPEN)?;

        if resp_body.len() < 17 {
            return Err(VirtioDeviceError::QueueUnknownError);
        }
        let (qid, _) = P9Qid::decode(resp_body).ok_or(VirtioDeviceError::QueueUnknownError)?;
        let iounit = read_u32_le(resp_body, 13);

        Ok((qid, iounit))
    }

    /// Tlcreate/Rlcreate — create a file, returning (qid, iounit).
    pub fn p9_lcreate(
        &self,
        fid: u32,
        name: &str,
        flags: u32,
        mode: u32,
        gid: u32,
    ) -> Result<(P9Qid, u32), VirtioDeviceError> {
        let mut body = Vec::new();
        body.extend_from_slice(&fid.to_le_bytes());
        encode_string(&mut body, name);
        body.extend_from_slice(&flags.to_le_bytes());
        body.extend_from_slice(&mode.to_le_bytes());
        body.extend_from_slice(&gid.to_le_bytes());

        let tag = self.alloc_tag_id();
        let request = build_message(P9_TLCREATE, tag, &body);
        let msize = self.msize() as usize;
        let response = self.send_9p_request(&request, msize)?;
        let resp_body = Self::check_9p_response(&response, P9_RLCREATE)?;

        if resp_body.len() < 17 {
            return Err(VirtioDeviceError::QueueUnknownError);
        }
        let (qid, _) = P9Qid::decode(resp_body).ok_or(VirtioDeviceError::QueueUnknownError)?;
        let iounit = read_u32_le(resp_body, 13);

        Ok((qid, iounit))
    }

    /// Tgetattr/Rgetattr.
    pub fn p9_getattr(&self, fid: u32, request_mask: u64) -> Result<P9Attr, VirtioDeviceError> {
        let mut body = Vec::new();
        body.extend_from_slice(&fid.to_le_bytes());
        body.extend_from_slice(&request_mask.to_le_bytes());

        let tag = self.alloc_tag_id();
        let request = build_message(P9_TGETATTR, tag, &body);
        let msize = self.msize() as usize;
        let response = self.send_9p_request(&request, msize)?;
        let resp_body = Self::check_9p_response(&response, P9_RGETATTR)?;

        let (attr, _) = P9Attr::decode(resp_body).ok_or(VirtioDeviceError::QueueUnknownError)?;

        Ok(attr)
    }

    /// Tsetattr/Rsetattr.
    pub fn p9_setattr(
        &self,
        fid: u32,
        valid: u32,
        mode: u32,
        uid: u32,
        gid: u32,
        size: u64,
        atime_sec: u64,
        atime_nsec: u64,
        mtime_sec: u64,
        mtime_nsec: u64,
    ) -> Result<(), VirtioDeviceError> {
        let mut body = Vec::new();
        body.extend_from_slice(&fid.to_le_bytes());
        body.extend_from_slice(&valid.to_le_bytes());
        body.extend_from_slice(&mode.to_le_bytes());
        body.extend_from_slice(&uid.to_le_bytes());
        body.extend_from_slice(&gid.to_le_bytes());
        body.extend_from_slice(&size.to_le_bytes());
        body.extend_from_slice(&atime_sec.to_le_bytes());
        body.extend_from_slice(&atime_nsec.to_le_bytes());
        body.extend_from_slice(&mtime_sec.to_le_bytes());
        body.extend_from_slice(&mtime_nsec.to_le_bytes());

        let tag = self.alloc_tag_id();
        let request = build_message(P9_TSETATTR, tag, &body);
        let msize = self.msize() as usize;
        let response = self.send_9p_request(&request, msize)?;
        let _ = Self::check_9p_response(&response, P9_RSETATTR)?;

        Ok(())
    }

    /// Tread/Rread — read data from an open fid.
    pub fn p9_read(&self, fid: u32, offset: u64, count: u32) -> Result<Vec<u8>, VirtioDeviceError> {
        let mut body = Vec::new();
        body.extend_from_slice(&fid.to_le_bytes());
        body.extend_from_slice(&offset.to_le_bytes());
        body.extend_from_slice(&count.to_le_bytes());

        let tag = self.alloc_tag_id();
        let request = build_message(P9_TREAD, tag, &body);
        let msize = self.msize() as usize;
        let response = self.send_9p_request(&request, msize)?;
        let resp_body = Self::check_9p_response(&response, P9_RREAD)?;

        if resp_body.len() < 4 {
            return Err(VirtioDeviceError::QueueUnknownError);
        }
        let data_len = read_u32_le(resp_body, 0) as usize;
        if resp_body.len() < 4 + data_len {
            return Err(VirtioDeviceError::QueueUnknownError);
        }

        Ok(resp_body[4..4 + data_len].to_vec())
    }

    /// Twrite/Rwrite — write data to an open fid, returns bytes written.
    pub fn p9_write(&self, fid: u32, offset: u64, data: &[u8]) -> Result<u32, VirtioDeviceError> {
        let mut body = Vec::new();
        body.extend_from_slice(&fid.to_le_bytes());
        body.extend_from_slice(&offset.to_le_bytes());
        let count = data.len() as u32;
        body.extend_from_slice(&count.to_le_bytes());
        body.extend_from_slice(data);

        let tag = self.alloc_tag_id();
        let request = build_message(P9_TWRITE, tag, &body);
        let msize = self.msize() as usize;
        let response = self.send_9p_request(&request, msize)?;
        let resp_body = Self::check_9p_response(&response, P9_RWRITE)?;

        if resp_body.len() < 4 {
            return Err(VirtioDeviceError::QueueUnknownError);
        }
        let written = read_u32_le(resp_body, 0);

        Ok(written)
    }

    /// Tclunk/Rclunk — release a fid.
    pub fn p9_clunk(&self, fid: u32) -> Result<(), VirtioDeviceError> {
        let mut body = Vec::new();
        body.extend_from_slice(&fid.to_le_bytes());

        let tag = self.alloc_tag_id();
        let request = build_message(P9_TCLUNK, tag, &body);
        let msize = self.msize() as usize;
        let response = self.send_9p_request(&request, msize)?;
        let _ = Self::check_9p_response(&response, P9_RCLUNK)?;

        Ok(())
    }

    /// Treaddir/Rreaddir — read directory entries from an open directory fid.
    pub fn p9_readdir(
        &self,
        fid: u32,
        offset: u64,
        count: u32,
    ) -> Result<Vec<P9DirEntry>, VirtioDeviceError> {
        let mut body = Vec::new();
        body.extend_from_slice(&fid.to_le_bytes());
        body.extend_from_slice(&offset.to_le_bytes());
        body.extend_from_slice(&count.to_le_bytes());

        let tag = self.alloc_tag_id();
        let request = build_message(P9_TREADDIR, tag, &body);
        let msize = self.msize() as usize;
        let response = self.send_9p_request(&request, msize)?;
        let resp_body = Self::check_9p_response(&response, P9_RREADDIR)?;

        if resp_body.len() < 4 {
            return Err(VirtioDeviceError::QueueUnknownError);
        }
        let data_len = read_u32_le(resp_body, 0) as usize;
        if resp_body.len() < 4 + data_len {
            return Err(VirtioDeviceError::QueueUnknownError);
        }

        let entries = parse_readdir_entries(&resp_body[4..4 + data_len]);
        Ok(entries)
    }

    /// Tmkdir/Rmkdir — create a directory, returns qid.
    pub fn p9_mkdir(
        &self,
        dfid: u32,
        name: &str,
        mode: u32,
        gid: u32,
    ) -> Result<P9Qid, VirtioDeviceError> {
        let mut body = Vec::new();
        body.extend_from_slice(&dfid.to_le_bytes());
        encode_string(&mut body, name);
        body.extend_from_slice(&mode.to_le_bytes());
        body.extend_from_slice(&gid.to_le_bytes());

        let tag = self.alloc_tag_id();
        let request = build_message(P9_TMKDIR, tag, &body);
        let msize = self.msize() as usize;
        let response = self.send_9p_request(&request, msize)?;
        let resp_body = Self::check_9p_response(&response, P9_RMKDIR)?;

        let (qid, _) = P9Qid::decode(resp_body).ok_or(VirtioDeviceError::QueueUnknownError)?;

        Ok(qid)
    }

    /// Tunlinkat/Runlinkat — remove a directory entry.
    pub fn p9_unlinkat(
        &self,
        dirfid: u32,
        name: &str,
        flags: u32,
    ) -> Result<(), VirtioDeviceError> {
        let mut body = Vec::new();
        body.extend_from_slice(&dirfid.to_le_bytes());
        encode_string(&mut body, name);
        body.extend_from_slice(&flags.to_le_bytes());

        let tag = self.alloc_tag_id();
        let request = build_message(P9_TUNLINKAT, tag, &body);
        let msize = self.msize() as usize;
        let response = self.send_9p_request(&request, msize)?;
        let _ = Self::check_9p_response(&response, P9_RUNLINKAT)?;

        Ok(())
    }

    /// Trenameat/Rrenameat — rename a directory entry.
    pub fn p9_renameat(
        &self,
        olddirfid: u32,
        oldname: &str,
        newdirfid: u32,
        newname: &str,
    ) -> Result<(), VirtioDeviceError> {
        let mut body = Vec::new();
        body.extend_from_slice(&olddirfid.to_le_bytes());
        encode_string(&mut body, oldname);
        body.extend_from_slice(&newdirfid.to_le_bytes());
        encode_string(&mut body, newname);

        let tag = self.alloc_tag_id();
        let request = build_message(P9_TRENAMEAT, tag, &body);
        let msize = self.msize() as usize;
        let response = self.send_9p_request(&request, msize)?;
        let _ = Self::check_9p_response(&response, P9_RRENAMEAT)?;

        Ok(())
    }

    /// Tlink/Rlink — create a hard link.
    pub fn p9_link(&self, dfid: u32, fid: u32, name: &str) -> Result<(), VirtioDeviceError> {
        let mut body = Vec::new();
        body.extend_from_slice(&dfid.to_le_bytes());
        body.extend_from_slice(&fid.to_le_bytes());
        encode_string(&mut body, name);

        let tag = self.alloc_tag_id();
        let request = build_message(P9_TLINK, tag, &body);
        let msize = self.msize() as usize;
        let response = self.send_9p_request(&request, msize)?;
        let _ = Self::check_9p_response(&response, P9_RLINK)?;

        Ok(())
    }

    /// Tsymlink/Rsymlink — create a symbolic link.
    pub fn p9_symlink(
        &self,
        dfid: u32,
        name: &str,
        symtgt: &str,
        gid: u32,
    ) -> Result<P9Qid, VirtioDeviceError> {
        let mut body = Vec::new();
        body.extend_from_slice(&dfid.to_le_bytes());
        encode_string(&mut body, name);
        encode_string(&mut body, symtgt);
        body.extend_from_slice(&gid.to_le_bytes());

        let tag = self.alloc_tag_id();
        let request = build_message(P9_TSYMLINK, tag, &body);
        let msize = self.msize() as usize;
        let response = self.send_9p_request(&request, msize)?;
        let resp_body = Self::check_9p_response(&response, P9_RSYMLINK)?;

        let (qid, _) = P9Qid::decode(resp_body).ok_or(VirtioDeviceError::QueueUnknownError)?;

        Ok(qid)
    }

    /// Treadlink/Rreadlink — read a symbolic link target.
    pub fn p9_readlink(&self, fid: u32) -> Result<String, VirtioDeviceError> {
        let mut body = Vec::new();
        body.extend_from_slice(&fid.to_le_bytes());

        let tag = self.alloc_tag_id();
        let request = build_message(P9_TREADLINK, tag, &body);
        let msize = self.msize() as usize;
        let response = self.send_9p_request(&request, msize)?;
        let resp_body = Self::check_9p_response(&response, P9_RREADLINK)?;

        let (target, _) = decode_string(resp_body).ok_or(VirtioDeviceError::QueueUnknownError)?;

        Ok(target)
    }

    /// Tmknod/Rmknod — create a special file.
    pub fn p9_mknod(
        &self,
        dfid: u32,
        name: &str,
        mode: u32,
        major: u32,
        minor: u32,
        gid: u32,
    ) -> Result<P9Qid, VirtioDeviceError> {
        let mut body = Vec::new();
        body.extend_from_slice(&dfid.to_le_bytes());
        encode_string(&mut body, name);
        body.extend_from_slice(&mode.to_le_bytes());
        body.extend_from_slice(&major.to_le_bytes());
        body.extend_from_slice(&minor.to_le_bytes());
        body.extend_from_slice(&gid.to_le_bytes());

        let tag = self.alloc_tag_id();
        let request = build_message(P9_TMKNOD, tag, &body);
        let msize = self.msize() as usize;
        let response = self.send_9p_request(&request, msize)?;
        let resp_body = Self::check_9p_response(&response, P9_RMKNOD)?;

        let (qid, _) = P9Qid::decode(resp_body).ok_or(VirtioDeviceError::QueueUnknownError)?;

        Ok(qid)
    }

    /// Tstatfs/Rstatfs — get filesystem statistics.
    pub fn p9_statfs(&self, fid: u32) -> Result<P9StatFs, VirtioDeviceError> {
        let mut body = Vec::new();
        body.extend_from_slice(&fid.to_le_bytes());

        let tag = self.alloc_tag_id();
        let request = build_message(P9_TSTATFS, tag, &body);
        let msize = self.msize() as usize;
        let response = self.send_9p_request(&request, msize)?;
        let resp_body = Self::check_9p_response(&response, P9_RSTATFS)?;

        P9StatFs::decode(resp_body).ok_or(VirtioDeviceError::QueueUnknownError)
    }

    /// Tfsync/Rfsync — sync a file to disk.
    pub fn p9_fsync(&self, fid: u32, datasync: u32) -> Result<(), VirtioDeviceError> {
        let mut body = Vec::new();
        body.extend_from_slice(&fid.to_le_bytes());
        body.extend_from_slice(&datasync.to_le_bytes());

        let tag = self.alloc_tag_id();
        let request = build_message(P9_TFSYNC, tag, &body);
        let msize = self.msize() as usize;
        let response = self.send_9p_request(&request, msize)?;
        let _ = Self::check_9p_response(&response, P9_RFSYNC)?;

        Ok(())
    }

    /// Tflush/Rflush — flush a pending request.
    pub fn p9_flush(&self, oldtag: u16) -> Result<(), VirtioDeviceError> {
        let mut body = Vec::new();
        body.extend_from_slice(&oldtag.to_le_bytes());

        let tag = self.alloc_tag_id();
        let request = build_message(P9_TFLUSH, tag, &body);
        let msize = self.msize() as usize;
        let response = self.send_9p_request(&request, msize)?;
        let _ = Self::check_9p_response(&response, P9_RFLUSH)?;

        Ok(())
    }
}
