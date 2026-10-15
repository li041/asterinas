# Inode Getter 调用点与锁保护分析

| Getter                        | 主要调用点                                                                                                                                                              | 使用上下文 / 锁保护                                                                                                                                                                 |
| ----------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `size()`                      | Inode::seek_end 默认实现；InodeHandle O_APPEND；ramfs read/fallocate；memfd seal checks；virtiofs read/write/seek_end；overlayfs resize/copy-up；VFS file-backed seek/append | InodeHandle::write/write_at 在更新 file offset 时持有 offset: Mutex，但这不保护 inode size。ramfs/virtiofs/overlayfs 多靠实现内部锁；memfd 在 seals.lock() 下读 size，用于 seal 策略，不是 inode metadata 锁 |
| `mode()`                      | exec setuid/setgid；ELF AT_SECURE；overlayfs copy-up create；exfat rename add_entry; memfd exec seal checks；systree metadata default                                  | 通常无调用点锁。memfd 在 seals.lock() 下调用；overlayfs build_upper_recursively_if_needed 持有 upper: SpinLock 时读 lower mode                                                               |
| `owner()`                     | exec setuid；overlayfs copy-up metadata                                                                                                                             | 无显式调用点锁；依赖具体 FS getter                                                                                                                                                      |
| `group()`                     | exec setgid；overlayfs copy-up metadata                                                                                                                             | 无显式调用点锁；依赖具体 FS getter                                                                                                                                                      |
| `atime()`/`mtime()`/`ctime()` | utimens 保留 UTIME_OMIT 值；overlayfs copy-up metadata                                                                                                                 | utimens 无 inode 锁。overlayfs copy-up 在 copy-up 流程内调用，外层可能仍持有 overlay upper lock                                                                                              |
| `seek_end()`                  | InodeHandle::seek fallback inode seek；virtiofs per-open file seek_end 先 revalidate attr 再读 size                                                                    | InodeHandle::seek 通过 do_seek_util 持有 offset mutex 更新文件位置；inode seek_end() 本身不受该锁保护                                                                                          |

- immutable attrs
	- `ino`
	- `type_`
	- `fs`
	- `extension`
	- `revalidation_policy`
- mutable字段
	- 其中有问题的
		- `mode`
			- exec: 先使用快照做一次快速判断, 看到`suid/sgid`会拿inode锁再reload`i_mode/i_uid/i_gid`![[Pasted image 20260629132357.png]]
			- chown也加锁
				- https://codebrowser.dev/linux/linux/fs/open.c.html#639
				- ![[Pasted image 20260629132636.png]]
		- `size`
			- InodeHandle的`write/write_at`没有保护inode size
		- `seek_end`
			- 本质是size()
- 目前inode上提供的attr都是快照, 没有对应的锁保护, 本身不提供关于操作的原子性保障
	- 对于`check-and-set`类型的元数据修改, 应该放到inode的同步边界, 原有的getter方法应该去掉, 改为把相关的参数传给inode, 让inode检查并修改