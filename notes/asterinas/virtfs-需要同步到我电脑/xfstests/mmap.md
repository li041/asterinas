- host修改后, 已有mmap什么时候看到新数据
	- #区别 mmap的普通load不会因为attr timeout自动发GETATTR
	- 目前virtiofsd也不支持host主动notify
- guest再次open
	- open-to-close consistency
	- `cache=auto`下会因为不带`FOPEN_KEEP_CACHE`而失效inode page cache, 后续已有mmap会重新`fault`看到新内容
		-  `close-to-open` 语义在 mmap 存在时仍然成立于“重新 open”这个边界
- dirty mmap和host修改的冲突
	- 取决于完成顺序, 谁后写谁覆盖(linux也不保证合并, 能发现冲突)
- https://github.com/asterinas/asterinas/pull/3399
	- 目前`VmMapping`持有FileLike, 可以保证文件描述表中最后一个fd执行`close(fd)`但是有映射的情况下对应的文件句柄仍然存在, 不会因为`Drop`发送`FUSE_RELEASE`, 已经可以实现
- 特殊的flag处理
```rust
FOPEN_DIRECT_IO + MAP_PRIVATE：允许；
FOPEN_DIRECT_IO + MAP_SHARED 且没有 FUSE_DIRECT_IO_ALLOW_MMAP：ENODEV。
```
- 目前策略是更严格, 对于`direct io`采取都拒绝
	- `mmapable`目前没有对应flags参数