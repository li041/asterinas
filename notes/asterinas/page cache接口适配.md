- `add_dma_bufs`需要同种类型, 需要再封装一层
```
enum FsInBuf {
    Dma(FsInDmaBuf),
    Bio(BioSegment),
}
```
- 现有的设计virtio的操作统一分配`in_header`, `in_payload`, `out_header`, `out_payload`
	- read和write使用`BioSegment`作为`in_payload\out_payload`, 不能使用通用方法
		- 格外的`prepare_reaa/write_request`
			- 涉及in_buf和out_buf的分配, 创建对应的`FuseRequest`
		- `*_bio_with_transient_handle` 对 BioSegment, 给page_cache使用
			- page cache新接口需要使用BioSegment
- ext2 面向 block device，读写单位天然由 block range / BioSegment::nbytes() 表达。
	- FUSE 面向 daemon 的 byte protocol，FUSE_READ / FUSE_WRITE request body 里明确有 offset 和 size 字段, Biosegment仍然需要使用offset和size
- 原来header和payload直接分配到一个连续的dma区域, 现在read和write需要走Biosegment, 就需要使用Vec来持有`FsInBuf`和`FsOutBuf`
	- 对于其他的fuse opeation, header和payload对于In和Out一起分配
	- 对于read, 对于In的header+payload一起分配, out_header单独分配
	- 对于write, 与read相反
## BioSegmentPool
- 带D信息, 然后dma_slice分配为`dma: Arc<DmaStream<D>`, 这样底层可以直接封装为FsDmaBuf
	- 生命周期的问题, `BioSegmentInner` drop的方法
		- ext2和exfat是Bio中会持有对应的生命周期
- CachePage分配必须要和BioSegment解耦!!!