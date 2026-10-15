- `seals`能串行化
	- write_at
	- resize
	- fallcate
	- set_mode
	- seals的相关setter和getter
- `read_at`是继承的ramInode
- ramInode的`metadata`锁才能保护`size`
- 现在的锁顺序是`seals -> page_cache -> metadata`
	- 但是这里拿`inode.size`没有保护, 应该在ramfs inode locks拿了之后再检查seals
	- page_cache的锁保护I/O, e.g. `read_at`, metadata的锁保护影响元数据修改的操作

## Memfd
- seals
	- write
		1. 检查是否上写锁
		2. 检查size相关的grow和shrink
			- 如果不让grow, 但是页内还是能写的