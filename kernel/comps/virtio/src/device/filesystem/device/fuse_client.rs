// SPDX-License-Identifier: MPL-2.0

use super::*;

impl FileSystemDevice {
    pub(crate) fn fuse_init(&self) -> Result<(), VirtioDeviceError> {
        let unique = self.alloc_unique();

        let in_header = InHeader::new(
            (size_of::<InHeader>() + size_of::<InitIn>()) as u32,
            FUSE_OPCODE_INIT,
            unique,
            0,
        );
        let init_in = InitIn::new(FUSE_KERNEL_VERSION, FUSE_KERNEL_MINOR_VERSION, 0, 0, 0);

        let (in_header_slice, in_payload_slice, out_header_slice, out_payload_slice) =
            self.prepare_request_slices(in_header, init_in, size_of::<InitOut>());

        self.submit_request_and_wait_early(
            0,
            unique,
            &[&in_header_slice, &in_payload_slice],
            &[&out_header_slice, &out_payload_slice],
        )?;

        self.read_reply_header(&out_header_slice, unique, "FUSE_INIT", false)?;

        out_payload_slice
            .mem_obj()
            .sync_from_device(out_payload_slice.offset().clone())
            .unwrap();
        let init_out: InitOut = out_payload_slice.read_val(0).unwrap();

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
        let unique = self.alloc_unique();

        let in_header = InHeader::new(
            (size_of::<InHeader>() + name.len() + 1) as u32,
            FUSE_OPCODE_LOOKUP,
            unique,
            parent_nodeid,
        );

        let in_header_slice = self.prepare_in_header_buf(in_header);
        let in_name_buf = self.alloc_to_device_buf(name.len() + 1);
        let out_header_slice = self.prepare_out_header_buf();
        let out_payload_buf = self.alloc_from_device_buf(size_of::<EntryOut>());

        let in_name_slice = Slice::new(in_name_buf.clone(), 0..(name.len() + 1));
        self.write_cstr_to_buf(&in_name_buf, name);
        in_name_slice
            .mem_obj()
            .sync_to_device(in_name_slice.offset().clone())
            .unwrap();

        let out_payload_slice = Slice::new(out_payload_buf.clone(), 0..size_of::<EntryOut>());
        let queue_index = self.select_request_queue_for_node(parent_nodeid);

        self.submit_request_and_wait(
            queue_index,
            unique,
            &[&in_header_slice, &in_name_slice],
            &[&out_header_slice, &out_payload_slice],
        )?;

        self.read_reply_header(&out_header_slice, unique, "FUSE_LOOKUP", true)?;

        out_payload_slice
            .mem_obj()
            .sync_from_device(out_payload_slice.offset().clone())
            .unwrap();
        let entry_out: EntryOut = out_payload_slice.read_val(0).unwrap();
        Ok(entry_out)
    }

    pub fn create_dir(
        &self,
        parent_nodeid: u64,
        name: &str,
        mode: u32,
    ) -> Result<u64, VirtioDeviceError> {
        let unique = self.alloc_unique();
        let in_header = InHeader::new(
            (size_of::<InHeader>() + size_of::<MkdirIn>() + name.len() + 1) as u32,
            FUSE_OPCODE_MKDIR,
            unique,
            parent_nodeid,
        );
        let mkdir_in = MkdirIn::new(mode);

        let (in_header_slice, in_payload_slice, out_header_slice, out_payload_slice) =
            self.prepare_request_slices(in_header, mkdir_in, size_of::<EntryOut>());
        let in_name_buf = self.alloc_to_device_buf(name.len() + 1);

        let in_name_slice = Slice::new(in_name_buf.clone(), 0..(name.len() + 1));
        self.write_cstr_to_buf(&in_name_buf, name);
        in_name_slice
            .mem_obj()
            .sync_to_device(in_name_slice.offset().clone())
            .unwrap();
        let queue_index = self.select_request_queue_for_node(parent_nodeid);

        self.submit_request_and_wait(
            queue_index,
            unique,
            &[&in_header_slice, &in_payload_slice, &in_name_slice],
            &[&out_header_slice, &out_payload_slice],
        )?;
        self.read_reply_header(&out_header_slice, unique, "FUSE_MKDIR", true)?;

        out_payload_slice
            .mem_obj()
            .sync_from_device(out_payload_slice.offset().clone())
            .unwrap();
        let entry_out: EntryOut = out_payload_slice.read_val(0).unwrap();
        Ok(entry_out.nodeid)
    }

    pub fn fuse_unlink(&self, parent_nodeid: u64, name: &str) -> Result<(), VirtioDeviceError> {
        let unique = self.alloc_unique();
        let in_header = InHeader::new(
            (size_of::<InHeader>() + name.len() + 1) as u32,
            FUSE_OPCODE_UNLINK,
            unique,
            parent_nodeid,
        );

        let in_header_slice = self.prepare_in_header_buf(in_header);
        let in_name_buf = self.alloc_to_device_buf(name.len() + 1);
        let out_header_slice = self.prepare_out_header_buf();

        let in_name_slice = Slice::new(in_name_buf.clone(), 0..(name.len() + 1));
        self.write_cstr_to_buf(&in_name_buf, name);
        in_name_slice
            .mem_obj()
            .sync_to_device(in_name_slice.offset().clone())
            .unwrap();
        let queue_index = self.select_request_queue_for_node(parent_nodeid);
        self.submit_request_and_wait(
            queue_index,
            unique,
            &[&in_header_slice, &in_name_slice],
            &[&out_header_slice],
        )?;
        self.read_reply_header(&out_header_slice, unique, "FUSE_UNLINK", true)?;
        Ok(())
    }

    pub fn fuse_rmdir(&self, parent_nodeid: u64, name: &str) -> Result<(), VirtioDeviceError> {
        let unique = self.alloc_unique();
        let in_header = InHeader::new(
            (size_of::<InHeader>() + name.len() + 1) as u32,
            FUSE_OPCODE_RMDIR,
            unique,
            parent_nodeid,
        );

        let in_header_slice = self.prepare_in_header_buf(in_header);
        let in_name_buf = self.alloc_to_device_buf(name.len() + 1);
        let out_header_slice = self.prepare_out_header_buf();

        let in_name_slice = Slice::new(in_name_buf.clone(), 0..(name.len() + 1));
        self.write_cstr_to_buf(&in_name_buf, name);
        in_name_slice
            .mem_obj()
            .sync_to_device(in_name_slice.offset().clone())
            .unwrap();
        let queue_index = self.select_request_queue_for_node(parent_nodeid);
        self.submit_request_and_wait(
            queue_index,
            unique,
            &[&in_header_slice, &in_name_slice],
            &[&out_header_slice],
        )?;
        self.read_reply_header(&out_header_slice, unique, "FUSE_RMDIR", true)?;
        Ok(())
    }

    pub fn create_file_with_flags(
        &self,
        parent_nodeid: u64,
        name: &str,
        mode: u32,
    ) -> Result<(u64, FuseOpenOut), VirtioDeviceError> {
        let unique = self.alloc_unique();
        let in_header = InHeader::new(
            (size_of::<InHeader>() + size_of::<CreateIn>() + name.len() + 1) as u32,
            FUSE_OPCODE_CREATE,
            unique,
            parent_nodeid,
        );
        let create_in = CreateIn::new(O_RDWR, mode);

        let out_payload_size = size_of::<EntryOut>() + size_of::<FuseOpenOut>();
        let (in_header_slice, in_payload_slice, out_header_slice, out_payload_slice) =
            self.prepare_request_slices(in_header, create_in, out_payload_size);
        let in_name_buf = self.alloc_to_device_buf(name.len() + 1);

        let in_name_slice = Slice::new(in_name_buf.clone(), 0..(name.len() + 1));
        self.write_cstr_to_buf(&in_name_buf, name);
        in_name_slice
            .mem_obj()
            .sync_to_device(in_name_slice.offset().clone())
            .unwrap();
        let queue_index = self.select_request_queue_for_node(parent_nodeid);

        self.submit_request_and_wait(
            queue_index,
            unique,
            &[&in_header_slice, &in_payload_slice, &in_name_slice],
            &[&out_header_slice, &out_payload_slice],
        )?;
        self.read_reply_header(&out_header_slice, unique, "FUSE_CREATE", true)?;

        out_payload_slice
            .mem_obj()
            .sync_from_device(out_payload_slice.offset().clone())
            .unwrap();
        let entry_out: EntryOut = out_payload_slice.read_val(0).unwrap();
        let open_out: FuseOpenOut = out_payload_slice.read_val(size_of::<EntryOut>()).unwrap();
        Ok((entry_out.nodeid, open_out))
    }

    pub fn fuse_getattr(&self, nodeid: u64) -> Result<FuseAttrOut, VirtioDeviceError> {
        let unique = self.alloc_unique();
        let in_header = InHeader::new(
            (size_of::<InHeader>() + size_of::<FuseGetattrIn>()) as u32,
            FUSE_OPCODE_GETATTR,
            unique,
            nodeid,
        );
        let getattr_in = FuseGetattrIn::new(0);

        let (in_header_slice, in_payload_slice, out_header_slice, out_payload_slice) =
            self.prepare_request_slices(in_header, getattr_in, size_of::<FuseAttrOut>());
        let queue_index = self.select_request_queue_for_node(nodeid);
        self.submit_request_and_wait(
            queue_index,
            unique,
            &[&in_header_slice, &in_payload_slice],
            &[&out_header_slice, &out_payload_slice],
        )?;
        self.read_reply_header(&out_header_slice, unique, "FUSE_GETATTR", false)?;

        out_payload_slice
            .mem_obj()
            .sync_from_device(out_payload_slice.offset().clone())
            .unwrap();
        Ok(out_payload_slice.read_val(0).unwrap())
    }

    pub fn fuse_setattr(&self, nodeid: u64, size: u64) -> Result<FuseAttrOut, VirtioDeviceError> {
        let unique = self.alloc_unique();
        let in_header = InHeader::new(
            (size_of::<InHeader>() + size_of::<SetattrIn>()) as u32,
            FUSE_OPCODE_SETATTR,
            unique,
            nodeid,
        );
        let setattr_in = SetattrIn::new_size(size);

        let (in_header_slice, in_payload_slice, out_header_slice, out_payload_slice) =
            self.prepare_request_slices(in_header, setattr_in, size_of::<FuseAttrOut>());
        let queue_index = self.select_request_queue_for_node(nodeid);
        self.submit_request_and_wait(
            queue_index,
            unique,
            &[&in_header_slice, &in_payload_slice],
            &[&out_header_slice, &out_payload_slice],
        )?;
        self.read_reply_header(&out_header_slice, unique, "FUSE_SETATTR", false)?;

        out_payload_slice
            .mem_obj()
            .sync_from_device(out_payload_slice.offset().clone())
            .unwrap();
        Ok(out_payload_slice.read_val(0).unwrap())
    }

    pub fn fuse_opendir(&self, nodeid: u64) -> Result<u64, VirtioDeviceError> {
        let unique = self.alloc_unique();
        let in_header = InHeader::new(
            (size_of::<InHeader>() + size_of::<OpenIn>()) as u32,
            FUSE_OPCODE_OPENDIR,
            unique,
            nodeid,
        );
        let open_in = OpenIn::new(0);

        let (in_header_slice, in_payload_slice, out_header_slice, out_payload_slice) =
            self.prepare_request_slices(in_header, open_in, size_of::<FuseOpenOut>());
        let queue_index = self.select_request_queue_for_node(nodeid);
        self.submit_request_and_wait(
            queue_index,
            unique,
            &[&in_header_slice, &in_payload_slice],
            &[&out_header_slice, &out_payload_slice],
        )?;
        self.read_reply_header(&out_header_slice, unique, "FUSE_OPENDIR", false)?;

        out_payload_slice
            .mem_obj()
            .sync_from_device(out_payload_slice.offset().clone())
            .unwrap();
        let open_out: FuseOpenOut = out_payload_slice.read_val(0).unwrap();
        Ok(open_out.fh)
    }

    pub fn fuse_readdir(
        &self,
        nodeid: u64,
        fh: u64,
        offset: u64,
        size: u32,
    ) -> Result<Vec<VirtioFsDirEntry>, VirtioDeviceError> {
        let unique = self.alloc_unique();
        let in_header = InHeader::new(
            (size_of::<InHeader>() + size_of::<ReadIn>()) as u32,
            FUSE_OPCODE_READDIR,
            unique,
            nodeid,
        );
        let read_in = ReadIn::new(fh, offset, size);

        let out_payload_size = size as usize;
        let (in_header_slice, in_payload_slice, out_header_slice, out_payload_slice) =
            self.prepare_request_slices(in_header, read_in, out_payload_size);
        let queue_index = self.select_request_queue_for_node(nodeid);

        self.submit_request_and_wait(
            queue_index,
            unique,
            &[&in_header_slice, &in_payload_slice],
            &[&out_header_slice, &out_payload_slice],
        )?;

        let out_header =
            self.read_reply_header(&out_header_slice, unique, "FUSE_READDIR", false)?;

        let payload_len = (out_header.len as usize).saturating_sub(size_of::<OutHeader>());
        let payload_len = cmp::min(payload_len, out_payload_size);
        out_payload_slice
            .mem_obj()
            .sync_from_device(out_payload_slice.offset().clone())
            .unwrap();

        let mut payload = vec![0u8; payload_len];
        let mut payload_reader = out_payload_slice.reader().unwrap();
        payload_reader.limit(payload_len);
        payload_reader.read(&mut VmWriter::from(payload.as_mut_slice()));

        let mut entries = Vec::new();
        let mut pos = 0usize;
        while pos + size_of::<Dirent>() <= payload_len {
            let header: Dirent = out_payload_slice.read_val(pos).unwrap();
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
            let aligned = (dirent_len + 7) & !7;
            pos += aligned;
        }

        Ok(entries)
    }

    pub fn fuse_releasedir(&self, nodeid: u64, fh: u64) -> Result<(), VirtioDeviceError> {
        let unique = self.alloc_unique();
        let in_header = InHeader::new(
            (size_of::<InHeader>() + size_of::<ReleaseIn>()) as u32,
            FUSE_OPCODE_RELEASEDIR,
            unique,
            nodeid,
        );
        let release_in = ReleaseIn::new(fh, 0);

        let in_header_slice = self.prepare_in_header_buf(in_header);
        let in_payload_slice = self.prepare_in_payload_buf(release_in);
        let out_header_slice = self.prepare_out_header_buf();
        let queue_index = self.select_request_queue_for_node(nodeid);

        self.submit_request_and_wait(
            queue_index,
            unique,
            &[&in_header_slice, &in_payload_slice],
            &[&out_header_slice],
        )?;

        self.read_reply_header(&out_header_slice, unique, "FUSE_RELEASEDIR", false)?;
        Ok(())
    }

    pub fn fuse_open(
        &self,
        nodeid: u64,
        flags: u32,
    ) -> Result<FuseOpenOut, VirtioDeviceError> {
        let unique = self.alloc_unique();
        let in_header = InHeader::new(
            (size_of::<InHeader>() + size_of::<OpenIn>()) as u32,
            FUSE_OPCODE_OPEN,
            unique,
            nodeid,
        );
        let open_in = OpenIn::new(flags);

        let (in_header_slice, in_payload_slice, out_header_slice, out_payload_slice) =
            self.prepare_request_slices(in_header, open_in, size_of::<FuseOpenOut>());
        let queue_index = self.select_request_queue_for_node(nodeid);

        self.submit_request_and_wait(
            queue_index,
            unique,
            &[&in_header_slice, &in_payload_slice],
            &[&out_header_slice, &out_payload_slice],
        )?;

        self.read_reply_header(&out_header_slice, unique, "FUSE_OPEN", false)?;

        out_payload_slice
            .mem_obj()
            .sync_from_device(out_payload_slice.offset().clone())
            .unwrap();
        let open_out: FuseOpenOut = out_payload_slice.read_val(0).unwrap();
        Ok(open_out)
    }

    pub fn fuse_release(&self, nodeid: u64, fh: u64, flags: u32) -> Result<(), VirtioDeviceError> {
        let unique = self.alloc_unique();
        let in_header = InHeader::new(
            (size_of::<InHeader>() + size_of::<ReleaseIn>()) as u32,
            FUSE_OPCODE_RELEASE,
            unique,
            nodeid,
        );
        let release_in = ReleaseIn::new(fh, flags);

        let in_header_slice = self.prepare_in_header_buf(in_header);
        let out_header_slice = self.prepare_out_header_buf();

        let in_payload_slice = self.prepare_in_payload_buf(release_in);
        let queue_index = self.select_request_queue_for_node(nodeid);

        self.submit_request_and_wait(
            queue_index,
            unique,
            &[&in_header_slice, &in_payload_slice],
            &[&out_header_slice],
        )?;
        self.read_reply_header(&out_header_slice, unique, "FUSE_RELEASE", true)?;

        Ok(())
    }

    pub fn fuse_read(
        &self,
        nodeid: u64,
        fh: u64,
        offset: u64,
        size: u32,
    ) -> Result<Vec<u8>, VirtioDeviceError> {
        let unique = self.alloc_unique();
        let in_header = InHeader::new(
            (size_of::<InHeader>() + size_of::<ReadIn>()) as u32,
            FUSE_OPCODE_READ,
            unique,
            nodeid,
        );
        let read_in = ReadIn::new(fh, offset, size);

        let out_payload_size = size as usize;
        let (in_header_slice, in_payload_slice, out_header_slice, out_payload_slice) =
            self.prepare_request_slices(in_header, read_in, out_payload_size);
        let queue_index = self.select_request_queue_for_node(nodeid);
        self.submit_request_and_wait(
            queue_index,
            unique,
            &[&in_header_slice, &in_payload_slice],
            &[&out_header_slice, &out_payload_slice],
        )?;
        let out_header = self.read_reply_header(&out_header_slice, unique, "FUSE_READ", false)?;

        let payload_len = (out_header.len as usize).saturating_sub(size_of::<OutHeader>());
        let payload_len = cmp::min(payload_len, out_payload_size);
        out_payload_slice
            .mem_obj()
            .sync_from_device(out_payload_slice.offset().clone())
            .unwrap();

        let mut content = vec![0u8; payload_len];
        let mut reader = out_payload_slice.reader().unwrap();
        reader.limit(payload_len);
        reader.read(&mut VmWriter::from(content.as_mut_slice()));
        Ok(content)
    }

    pub fn fuse_write(
        &self,
        nodeid: u64,
        fh: u64,
        offset: u64,
        data: &[u8],
    ) -> Result<usize, VirtioDeviceError> {
        let unique = self.alloc_unique();
        let in_header = InHeader::new(
            (size_of::<InHeader>() + size_of::<WriteIn>() + data.len()) as u32,
            FUSE_OPCODE_WRITE,
            unique,
            nodeid,
        );
        let write_in = WriteIn::new(fh, offset, data.len() as u32);

        let (in_header_slice, in_payload_slice, out_header_slice, out_payload_slice) =
            self.prepare_request_slices(in_header, write_in, size_of::<WriteOut>());
        let in_data_buf = self.alloc_to_device_buf(data.len());

        let in_data_slice = Slice::new(in_data_buf.clone(), 0..data.len());
        {
            let mut writer = in_data_buf.writer().unwrap();
            let mut reader = VmReader::from(data);
            let _ = writer.write(&mut reader);
        }
        in_data_slice
            .mem_obj()
            .sync_to_device(in_data_slice.offset().clone())
            .unwrap();
        let queue_index = self.select_request_queue_for_node(nodeid);

        self.submit_request_and_wait(
            queue_index,
            unique,
            &[&in_header_slice, &in_payload_slice, &in_data_slice],
            &[&out_header_slice, &out_payload_slice],
        )?;
        self.read_reply_header(&out_header_slice, unique, "FUSE_WRITE", false)?;

        out_payload_slice
            .mem_obj()
            .sync_from_device(out_payload_slice.offset().clone())
            .unwrap();
        let write_out: WriteOut = out_payload_slice.read_val(0).unwrap();
        Ok(write_out.size as usize)
    }

    pub fn fuse_forget(&self, nodeid: u64, nlookup: u64) -> Result<(), VirtioDeviceError> {
        if nodeid == FUSE_ROOT_ID || nlookup == 0 {
            return Ok(());
        }
        let unique = self.alloc_unique();
        let in_header = InHeader::new(
            (size_of::<InHeader>() + size_of::<ForgetIn>()) as u32,
            FUSE_OPCODE_FORGET,
            unique,
            nodeid,
        );
        let forget_in = ForgetIn::new(nlookup);

        let in_header_buf = self.alloc_to_device_buf(size_of::<InHeader>());
        let in_payload_slice = self.prepare_in_payload_buf(forget_in);

        let in_header_slice = Slice::new(in_header_buf.clone(), 0..size_of::<InHeader>());
        in_header_slice.write_val(0, &in_header).unwrap();
        in_header_slice
            .mem_obj()
            .sync_to_device(in_header_slice.offset().clone())
            .unwrap();

        {
            let mut queue = self.hiprio_queue.queue.lock();
            let token = queue.add_dma_buf(&[&in_header_slice, &in_payload_slice], &[])?;
            self.register_pending_request_on_queue(RequestQueueSelector::Hiprio, token, unique);
            if queue.should_notify() {
                queue.notify();
            }
        }

        self.wait_for_unique_on(RequestQueueSelector::Hiprio, unique as usize)
    }
}
