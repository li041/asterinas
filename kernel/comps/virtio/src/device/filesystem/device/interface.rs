// SPDX-License-Identifier: MPL-2.0

use super::*;

impl FileSystemDevice {
    pub fn negotiate_features(features: u64) -> u64 {
        let device_features = FileSystemFeatures::from_bits_truncate(features);
        let supported_features = FileSystemFeatures::supported_features();
        let fs_features = device_features & supported_features;
        debug!("features negotiated: {:?}", fs_features);
        fs_features.bits()
    }

    pub fn init(mut transport: Box<dyn VirtioTransport>) -> Result<(), VirtioDeviceError> {
        let config_manager = VirtioFsConfig::new_manager(transport.as_ref());
        let config = config_manager.read_config();

        let notify_supported =
            transport.read_device_features() & FileSystemFeatures::NOTIFICATION.bits() != 0;
        let special_queues_count = if notify_supported { 2 } else { 1 };

        let total_queues = transport.num_queues();
        let max_request_queues_from_transport =
            total_queues.saturating_sub(special_queues_count) as usize;
        let request_queue_count = cmp::min(
            config.num_request_queues as usize,
            max_request_queues_from_transport,
        );

        if request_queue_count == 0 {
            return Err(VirtioDeviceError::QueuesAmountDoNotMatch(
                total_queues,
                special_queues_count + config.num_request_queues as u16,
            ));
        }

        let hiprio_queue =
            FsRequestQueue::new(Self::new_queue(HIPRIO_QUEUE_INDEX, transport.as_mut())?);

        let dma_pools = FsDmaPools::new();

        let mut request_queues = Vec::with_capacity(request_queue_count);
        for idx in 0..request_queue_count {
            let queue_index = special_queues_count + idx as u16;
            request_queues.push(FsRequestQueue::new(Self::new_queue(
                queue_index,
                transport.as_mut(),
            )?));
        }

        let tag = Self::parse_tag(&config.tag);
        let device = Arc::new(Self {
            transport: SpinLock::new(transport),
            hiprio_queue,
            request_queues,
            dma_pools,
            unique_id_alloc: SyncIdAlloc::with_capacity(UNIQUE_ID_ALLOC_CAPACITY),
            tag,
            notify_supported,
        });

        let mut transport = device.transport.lock();
        transport
            .register_cfg_callback(Box::new(config_space_change))
            .unwrap();
        let device_for_hiprio_callback = device.clone();
        let hiprio_wakeup_callback = move |_: &TrapFrame| {
            device_for_hiprio_callback.handle_queue_irq(RequestQueueSelector::Hiprio);
        };
        transport
            .register_queue_callback(HIPRIO_QUEUE_INDEX, Box::new(hiprio_wakeup_callback), false)
            .unwrap();
        for idx in 0..request_queue_count {
            let queue_idx = special_queues_count + idx as u16;
            let device_for_callback = device.clone();
            let wakeup_callback = move |_: &TrapFrame| {
                device_for_callback.handle_queue_irq(RequestQueueSelector::Request(
                    (queue_idx - special_queues_count) as usize,
                ));
            };
            transport
                .register_queue_callback(queue_idx, Box::new(wakeup_callback), false)
                .unwrap();
        }
        transport.finish_init();
        drop(transport);

        device.send_fuse_init()?;

        FILESYSTEM_DEVICES
            .call_once(|| SpinLock::new(Vec::new()))
            .disable_irq()
            .lock()
            .push(device.clone());

        info!(
            "{} initialized, tag = {}, request_queues = {}, notify = {}",
            DEVICE_NAME,
            device.tag,
            device.request_queues.len(),
            device.notify_supported
        );
        info!(
            "{} test file read is deferred; call debug_read_test_file_for_all_devices() later",
            DEVICE_NAME
        );

        let _ = &device.hiprio_queue;
        let _ = &device.request_queues;

        Ok(())
    }

    pub fn tag(&self) -> &str {
        &self.tag
    }

    pub fn lookup(&self, parent_nodeid: u64, name: &str) -> Result<u64, VirtioDeviceError> {
        Ok(self.lookup_entry(parent_nodeid, name)?.nodeid)
    }

    pub fn lookup_entry(
        &self,
        parent_nodeid: u64,
        name: &str,
    ) -> Result<EntryOut, VirtioDeviceError> {
        self.lookup_entry_inner(parent_nodeid, name)
    }

    pub fn forget_lookup(&self, nodeid: u64, nlookup: u64) -> Result<(), VirtioDeviceError> {
        if nodeid == FUSE_ROOT_ID || nlookup == 0 {
            return Ok(());
        }
        self.send_fuse_forget(nodeid, nlookup)
    }

    pub fn getattr_node(&self, nodeid: u64) -> Result<FuseAttrOut, VirtioDeviceError> {
        self.getattr(nodeid)
    }

    pub fn setattr_size(&self, nodeid: u64, size: u64) -> Result<FuseAttrOut, VirtioDeviceError> {
        self.setattr_size_inner(nodeid, size)
    }

    pub fn read_file_at(
        &self,
        nodeid: u64,
        offset: u64,
        size: u32,
    ) -> Result<Vec<u8>, VirtioDeviceError> {
        let file_handle = self.open_file(nodeid, O_RDONLY)?;
        let result = self.read_file_with_fh(nodeid, file_handle, offset, size);
        let _ = self.release_file(nodeid, file_handle, O_RDONLY);
        result
    }

    pub fn write_file_at(
        &self,
        nodeid: u64,
        offset: u64,
        data: &[u8],
    ) -> Result<usize, VirtioDeviceError> {
        let file_handle = self.open_file(nodeid, O_WRONLY)?;
        let result = self.write_file_with_fh(nodeid, file_handle, offset, data);
        let _ = self.release_file(nodeid, file_handle, O_WRONLY);
        result
    }

    pub fn read_file_with_fh(
        &self,
        nodeid: u64,
        fh: u64,
        offset: u64,
        size: u32,
    ) -> Result<Vec<u8>, VirtioDeviceError> {
        self.read_file(nodeid, fh, offset, size)
    }

    pub fn write_file_with_fh(
        &self,
        nodeid: u64,
        fh: u64,
        offset: u64,
        data: &[u8],
    ) -> Result<usize, VirtioDeviceError> {
        self.write_file(nodeid, fh, offset, data)
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

        self.submit_request_and_wait(
            0,
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

    pub fn unlink_file(&self, parent_nodeid: u64, name: &str) -> Result<(), VirtioDeviceError> {
        self.remove_entry(parent_nodeid, name, FUSE_OPCODE_UNLINK, "FUSE_UNLINK")
    }

    pub fn remove_dir(&self, parent_nodeid: u64, name: &str) -> Result<(), VirtioDeviceError> {
        self.remove_entry(parent_nodeid, name, FUSE_OPCODE_RMDIR, "FUSE_RMDIR")
    }

    pub fn create_file(
        &self,
        parent_nodeid: u64,
        name: &str,
        mode: u32,
    ) -> Result<(u64, u64), VirtioDeviceError> {
        let (nodeid, open_out) = self.create_file_with_flags(parent_nodeid, name, mode)?;
        Ok((nodeid, open_out.fh))
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

        self.submit_request_and_wait(
            0,
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

    pub fn readdir(
        &self,
        nodeid: u64,
        offset: u64,
    ) -> Result<Vec<VirtioFsDirEntry>, VirtioDeviceError> {
        let dir_handle = self.open_dir(nodeid)?;
        let entries = self.read_dir_entries(nodeid, dir_handle, offset, FUSE_READDIR_BUF_SIZE)?;
        let _ = self.release_dir(nodeid, dir_handle);
        Ok(entries)
    }

    fn send_fuse_init(&self) -> Result<(), VirtioDeviceError> {
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

    fn lookup_entry_inner(
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

        self.submit_request_and_wait(
            0,
            unique,
            &[&in_header_slice, &in_name_slice],
            &[&out_header_slice, &out_payload_slice],
        )?;

        out_header_slice
            .mem_obj()
            .sync_from_device(out_header_slice.offset().clone())
            .unwrap();
        let out_header: OutHeader = out_header_slice.read_val(0).unwrap();
        if out_header.unique != unique || out_header.error != 0 {
            if out_header.error == FUSE_ERR_ENOENT {
                debug!(
                    "{} FUSE_LOOKUP miss: parent={}, name={}, unique={}, error={}, out_len={}",
                    DEVICE_NAME,
                    parent_nodeid,
                    name,
                    out_header.unique,
                    out_header.error,
                    out_header.len
                );
            } else {
                warn!(
                    "{} FUSE_LOOKUP failed: parent={}, name={}, unique={}, error={}, out_len={}",
                    DEVICE_NAME,
                    parent_nodeid,
                    name,
                    out_header.unique,
                    out_header.error,
                    out_header.len
                );
            }
            if out_header.unique == unique && out_header.error != 0 {
                return Err(VirtioDeviceError::FileSystemError(out_header.error));
            }
            return Err(VirtioDeviceError::QueueUnknownError);
        }

        out_payload_slice
            .mem_obj()
            .sync_from_device(out_payload_slice.offset().clone())
            .unwrap();
        let entry_out: EntryOut = out_payload_slice.read_val(0).unwrap();
        Ok(entry_out)
    }

    fn getattr(&self, nodeid: u64) -> Result<FuseAttrOut, VirtioDeviceError> {
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
        self.submit_request_and_wait(
            0,
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

    fn setattr_size_inner(&self, nodeid: u64, size: u64) -> Result<FuseAttrOut, VirtioDeviceError> {
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
        self.submit_request_and_wait(
            0,
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

    fn open_dir(&self, nodeid: u64) -> Result<u64, VirtioDeviceError> {
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
        self.submit_request_and_wait(
            0,
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

    fn read_dir_entries(
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

        {
            let mut queue = self.request_queues[0].queue.lock();
            let token = queue.add_dma_buf(
                &[&in_header_slice, &in_payload_slice],
                &[&out_header_slice, &out_payload_slice],
            )?;
            self.register_pending_request(0, token, unique);
            if queue.should_notify() {
                queue.notify();
            }
        };

        self.wait_for_unique(0, unique)?;

        out_header_slice
            .mem_obj()
            .sync_from_device(out_header_slice.offset().clone())
            .unwrap();
        let out_header: OutHeader = out_header_slice.read_val(0).unwrap();
        if out_header.unique != unique || out_header.error != 0 {
            warn!(
                "{} FUSE_READDIR failed: unique={}, error={}, out_len={}",
                DEVICE_NAME, out_header.unique, out_header.error, out_header.len
            );
            return Err(VirtioDeviceError::QueueUnknownError);
        }

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

    fn release_dir(&self, nodeid: u64, fh: u64) -> Result<(), VirtioDeviceError> {
        let unique = self.alloc_unique();
        let in_header = InHeader::new(
            (size_of::<InHeader>() + size_of::<ReleaseIn>()) as u32,
            FUSE_OPCODE_RELEASEDIR,
            unique,
            nodeid,
        );
        let release_in = ReleaseIn::new(fh, 0);

        let in_header_buf = self.alloc_to_device_buf(size_of::<InHeader>());
        let in_payload_slice = self.prepare_in_payload_buf(release_in);
        let out_header_buf = self.alloc_from_device_buf(size_of::<OutHeader>());

        let in_header_slice = Slice::new(in_header_buf.clone(), 0..size_of::<InHeader>());
        in_header_slice.write_val(0, &in_header).unwrap();
        in_header_slice
            .mem_obj()
            .sync_to_device(in_header_slice.offset().clone())
            .unwrap();

        let out_header_slice = Slice::new(out_header_buf.clone(), 0..size_of::<OutHeader>());

        {
            let mut queue = self.request_queues[0].queue.lock();
            let token =
                queue.add_dma_buf(&[&in_header_slice, &in_payload_slice], &[&out_header_slice])?;
            self.register_pending_request(0, token, unique);
            if queue.should_notify() {
                queue.notify();
            }
        };

        self.wait_for_unique(0, unique)?;

        out_header_slice
            .mem_obj()
            .sync_from_device(out_header_slice.offset().clone())
            .unwrap();
        let out_header: OutHeader = out_header_slice.read_val(0).unwrap();
        if out_header.unique != unique || out_header.error != 0 {
            warn!(
                "{} FUSE_RELEASEDIR failed: unique={}, error={}, out_len={}",
                DEVICE_NAME, out_header.unique, out_header.error, out_header.len
            );
            return Err(VirtioDeviceError::QueueUnknownError);
        }
        Ok(())
    }

    pub fn open_file(&self, nodeid: u64, flags: u32) -> Result<u64, VirtioDeviceError> {
        let open_out = self.open_file_with_flags(nodeid, flags)?;
        Ok(open_out.fh)
    }

    pub fn open_file_with_flags(
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

        {
            let mut queue = self.request_queues[0].queue.lock();
            let token = queue.add_dma_buf(
                &[&in_header_slice, &in_payload_slice],
                &[&out_header_slice, &out_payload_slice],
            )?;
            self.register_pending_request(0, token, unique);
            if queue.should_notify() {
                queue.notify();
            }
        };

        self.wait_for_unique(0, unique)?;

        out_header_slice
            .mem_obj()
            .sync_from_device(out_header_slice.offset().clone())
            .unwrap();
        let out_header: OutHeader = out_header_slice.read_val(0).unwrap();
        if out_header.unique != unique || out_header.error != 0 {
            warn!(
                "{} FUSE_OPEN failed: unique={}, error={}, out_len={}",
                DEVICE_NAME, out_header.unique, out_header.error, out_header.len
            );
            return Err(VirtioDeviceError::QueueUnknownError);
        }

        out_payload_slice
            .mem_obj()
            .sync_from_device(out_payload_slice.offset().clone())
            .unwrap();
        let open_out: FuseOpenOut = out_payload_slice.read_val(0).unwrap();
        Ok(open_out)
    }

    fn remove_entry(
        &self,
        parent_nodeid: u64,
        name: &str,
        opcode: u32,
        op_name: &str,
    ) -> Result<(), VirtioDeviceError> {
        let unique = self.alloc_unique();
        let in_header = InHeader::new(
            (size_of::<InHeader>() + name.len() + 1) as u32,
            opcode,
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
        self.submit_request_and_wait(
            0,
            unique,
            &[&in_header_slice, &in_name_slice],
            &[&out_header_slice],
        )?;
        self.read_reply_header(&out_header_slice, unique, op_name, true)
            .map_err(|err| {
                warn!(
                    "{} {} failed: parent={}, name={}, unique={} (request)",
                    DEVICE_NAME, op_name, parent_nodeid, name, unique
                );
                err
            })?;

        Ok(())
    }

    pub fn release_file(&self, nodeid: u64, fh: u64, flags: u32) -> Result<(), VirtioDeviceError> {
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

        self.submit_request_and_wait(
            0,
            unique,
            &[&in_header_slice, &in_payload_slice],
            &[&out_header_slice],
        )?;
        self.read_reply_header(&out_header_slice, unique, "FUSE_RELEASE", true)?;

        Ok(())
    }

    fn read_file(
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
        self.submit_request_and_wait(
            0,
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

    fn write_file(
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

        self.submit_request_and_wait(
            0,
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

    fn send_fuse_forget(&self, nodeid: u64, nlookup: u64) -> Result<(), VirtioDeviceError> {
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

        self.wait_for_unique_on(RequestQueueSelector::Hiprio, unique)
    }
}
