- 测试
```sh
#本地测 ext2：
  bash test/initramfs/src/benchmark/bench_linux_and_aster.sh fio/seq_write_bw/ext2
  bash test/initramfs/src/benchmark/bench_linux_and_aster.sh fio/seq_read_bw/ext2

#no-IOMMU ext2：

bash test/initramfs/src/benchmark/bench_linux_and_aster.sh fio/seq_write_bw/ext2_no_iommu
bash test/initramfs/src/benchmark/bench_linux_and_aster.sh fio/seq_read_bw/ext2_no_iommu

#本地测 virtio-fs：

bash test/initramfs/src/benchmark/bench_linux_and_aster.sh fio/seq_write_bw/virtiofs
bash test/initramfs/src/benchmark/bench_linux_and_aster.sh fio/seq_read_bw/virtiofs
```

***
# FIO Benchmark

FIO benchmark 目前按照「workload + 文件系统」的方式组织。

目录结构如下：

```text
test/initramfs/src/benchmark/fio/
├── common/
│   └── run.sh
├── fs/
│   ├── ext2/
│   │   ├── config.sh
│   │   └── prepare.sh
│   └── virtiofs/
│       ├── config.sh
│       └── prepare.sh
├── seq_read_bw/
│   ├── ext2/
│   ├── ext2_no_iommu/
│   └── virtiofs/
└── seq_write_bw/
    ├── ext2/
    ├── ext2_no_iommu/
    └── virtiofs/
```

所有 workload 共用 `fio/common/run.sh`。

运行时通过两个变量指定测试内容：

```bash
FIO_FS_TYPE=ext2|virtiofs
FIO_WORKLOAD=read|write
```

`run.sh` 会加载对应文件系统的配置和准备脚本：

```text
fio/fs/ext2/config.sh
fio/fs/ext2/prepare.sh

fio/fs/virtiofs/config.sh
fio/fs/virtiofs/prepare.sh
```

## fio测试
- https://deepwiki.com/ceph/fio/5.3-output-formats
- 输出带单位
	- https://github.com/ceph/fio/blob/fd70e3619c00bc9f7b2f80cadf3fdb348cbacf51/lib/num2str.c#21
	- benchmark脚本没解析![[Pasted image 20260702191657.png]]
- read/write都是direct的

| FS       | Workload | Linux (MB/s) | Asterinas (MB/s) | Asterinas vs Linux |
| :------- | :------- | -----------: | ---------------: | -----------------: |
| ext2     | write    |      2508.19 |          4905.24 |            +95.57% |
| ext2     | read     |      5658.12 |          5616.17 |             -0.74% |
| virtiofs | write    |      5316.28 |          5485.10 |             +3.18% |
| virtiofs | read     |     10705.20 |          4127.20 |            -61.45% |

- read路径相较于linux慢很多
	- direct_io中间会有分配1Mib的dma buf, 存在中间拷贝
- linux
	- linux似乎是直接写到用户提供的buffer中, 没有中间拷贝
	- 对于`cached read/write`走的是page cache路径`filemap_read`, 对于`direct_io`应该是从用户抽页
	- [iov_iter_extract_pages](https://raw.githubusercontent.com/torvalds/linux/v6.16/fs/fuse/file.c#L1496-L1499](https://codebrowser.dev/linux/linux/lib/iov_iter.c.html#iov_iter_extract_pages)
		- 将一段抽象的`iov_iter`转换为对应的page list
			- linux bio也走这个
			- 对`user buffer`会调用`get_user_pages`![[Pasted image 20260702173621.png]]
			- 会pin page, 必须固定在物理内存中(要被DMA直接使用), transfer结束后由caller unpin
			- `FOLL_NOFAULT`可以禁止page fault, page不存在直接失败, 正常是会分配然后pin住
		- 为什么ext2的性能没受影响
			- 也没有直接从`user addr`建BioSegment
			- 而是`BioSegment::alloc`, 这个是从全局的block I/O pool分片, virtio-fs的pool最大粒度到4Kib, 导致每次都重新分配, 性能影响更大

```
fuse_direct_io
	-> fuse_get_user_pages
		-> iov_iter_extract_pages
			-> iov_iter_extract_user_pages (用户提供的地址直接转换到DMA使用的页)
```

- [FUSE_IO_PRIV_SYNC](https://codebrowser.dev/linux/linux/fs/fuse/fuse_i.h.html#394)

## direct_io
- ext2
	- `kernel/src/fs/fs_impls/ext2/inode/file.rs#351L`
```
Biosegment::alloc
fs.read_blocks(..., bio_segement.clone())
bio_segmnet.reader().read_fallible(writer)
```
- BioSegmentInner
```
dma_slice: Slice<Arc<DmaStream>>,
direction: BioDirection,
from_pool: bool
```
- FuseReplyBuf/FuseRequestBuf
	- `Arc<Slice<FsDmaStorage<DmaDirection>>`
## direct_io加了userdma后
- 比例略微提升, 41%![[Pasted image 20260702192015.png]]
	- 差异
		- virtqueue indirect descriptor, 有descriptor table
		- direct_read_at()前需要调用`revalidate_attr`
			- #todo 这个之前是缺少short read
- direct_read_at前revalidate_attr的原因![[Pasted image 20260706105756.png]]
## short read/write
- linux通过描述中的`args->out_argvar`判断最后一个out arg是否允许变短
```
ReadOperation     -> VariablePayload(requested_size)
ReaddirOperation  -> VariablePayload(requested_size)
ReadlinkOperation -> VariablePayload(MAX_READLINK_LEN)
WriteOperation    -> FixedPayload(size_of::<WriteReply>())
GetattrOperation  -> FixedPayload(size_of::<FuseAttrReply>())
# 还有没支持的GETXATTR, LISTXAAT也支持短读
```
## 支持cached测试
- 增加参数传递
	- `bash test/initramfs/src/benchmark/bench_linux_and_aster.sh fio/seq_write_bw/ext2`
		- 让`bench_linux_and_aster.sh`构造
		- `bench_result.yaml`中提供variants
- 测试结果
- cached read
	-  当前 page cache 逐个 4 KiB page 调用 read_fallible 复制到用户空间，这就是当前通用 cached-read 上限。
- Direct read 每次分配Imib的问题
- cached write 
	- write_through的策略问题
```
  写 page cache
  -> flush_range
  -> 拆成 4 KiB FUSE_WRITE
  -> 等待整批完成
```