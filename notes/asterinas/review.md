- virtio/src/device/filesystem/
	- [x] config.rs
	- [ ] device.rs
		- [ ] 不好review, 先拆分下
			- [x] fuse_client.rs
				- [x] `submit_request_and_wait_reply`选队列不对
				- [x] fuse_create只返回了nodeid, 需要放回EntryOut吗?
					- [ ] 暂且这样, review完上层后再决定
			- [x] helper.rs
				- [x] unique的分配为啥+1, -1
					- 不需要, unique可以从0开始
			- [x] virtio_ops
				- [ ] 
	- [x] mod.rs
	- [x] pool.rs
		- [x] 需要重构, 先把device中准备buf/slice的抽象成一个函数在重构, 
			- 看能不能去掉PooledDmaBuf层面
				- 直接使用DmaStream
				- 或使用DmaPool
			- `add_dma_buf`只需要`T: DmaBuf`就可以
			- 而我现在准备buf使用了
				- `Slice::new`这个是对MemObj对象封装
				- 而我的`in/out_header/payload_slice`使用了`write_val`方法, 也就是需要
					- `HasSize`, `HasVmReaderWriter<Types = VmReaderWriterResult>` trait就可以
				- [x] `DmaSegment`好像自己实现了`reader`和`writer`方法, 可以直接使用这个, 而不是调用`Slice`的方法
		- [x] `sync_from/to_device`方法需要返回Result
			- 就返回`Ok(())`, 没人调
	- [x] protocol.rs
- kernel/src/fs/virtiofs
	- [x] mod.rs
	- [ ] fs.rs
		- [x] `VirtioFsType`
			- [ ] `create`方法没有考虑`FsFlags`
			- [ ] `sysnode`现在返回的是`None`, 应该创建节点在`sys/fs`下
		- [x] `VirtioFs`
			- [x] `new`方法应该返回`Result`, `getattr`失败后不应该使用默认值
			- [ ] `sync`需要实现
		- [ ] `VirtioFsInode`
			- [x] 字段需要清理, revalidate的接口会改变
				- [x] LookupState应该将会不需要, 把`entry_valid_until`放在外面就行
				- [x] 为什么现在是`cache_lock`的设计
			- [x] 现在的readdir每次都是重新打开dir, 然后readdir再释放
	- [x] 为什么`PaegCacheBackend`是异步的
	- 需要实现三个From方法, `VirtioDeviceError -> Error`, 现在是`map_virtiofs_error`函数, `Attr -> Metadata`现在是`metadata_from_attr`, `VirtioFsDirEntry.type_:u32 -> InodeType`, 现在是`inode_type_from_dirent_type`
- QEMU_ARGS得调整
	- 关于virtiofs的不能放到common args
## 5.15

 Findings

  - medium kernel/src/fs/fs_impls/virtiofs/fs/inode/vfs.rs:250: readdir_at() opens a directory handle, then invalidate_whole_page_cache()? can return
    early before FUSE_RELEASEDIR is sent. The daemon leaks the open directory handle on that path. Fix by wrapping the handle in an RAII guard or
    ensuring release runs on every exit path.
  - medium kernel/src/fs/fs_impls/virtiofs/fs/inode/vfs.rs:327 and kernel/src/fs/fs_impls/virtiofs/fs/inode/vfs.rs:346: transient handles are released
    only after direct_read_at() / direct_write_at() returns. If either path blocks indefinitely or is interrupted by future error paths before reaching
    the release call, handle cleanup depends on manual sequencing. This conflicts with the resource-management guideline. Prefer an RAII transient-
    handle guard that releases in Drop, or make VirtioFsOpenHandle itself own release semantics.

  Notes

  - Reviewed the committed branch diff main...virtio-fs-final-rebase.
  - The current worktree has uncommitted edits in kernel/comps/virtio/src/device/filesystem/device/mod.rs and kernel/comps/virtio/src/device/
    filesystem/device/virtio_ops.rs; I did not treat those as part of the branch review.