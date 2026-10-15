  当前 size_lock 方案下，cached_read_at() 是：

  cached_read_at()
    revalidate_attr(fh)
      inner.read()          // 只短暂检查 attr TTL
      如果过期：FUSE_GETATTR，不持 inner
      commit_attr_reply()
        size_lock.write()
        inner.write()
        page_cache.resize/invalidate if needed
    size_lock.read()
    file_size = size()
    page_cache.read()

  关键是：inner 不会被持有到 page_cache.read() 里面。所以 read 的长 I/O 阶段不会占住
  metadata 锁。

  如果你用 inner 来串行化 I/O，逻辑会变成类似：

  cached_read_at()
    revalidate_attr(fh)
    inner.read()
    file_size = inner.metadata.size
    page_cache.read()       // 持 inner.read 做可能很慢的 I/O

  然后举一个并发例子：

  Thread A: cached_read_at()
    revalidate_attr(): 发现 attr 还有效，释放 inner
    inner.read()            // 如果用 inner 串行化 I/O
    page_cache.read()       // 卡在 FUSE_READ 或 page fault/backend read

  Thread B: cached_read_at() / getattr path
    revalidate_attr()
      inner.read(): 发现 attr 过期，释放 inner
      FUSE_GETATTR 返回新的 metadata
      commit_attr_reply()
        inner.write()       // 被 Thread A 的 inner.read 卡住

  这时问题是：Thread B 已经拿到了更新后的 server attributes，但无法提交，因为 Thread
  A 为了读文件数据长时间持有 inner.read()。结果是 metadata cache 的刷新被普通 data
  read 阻塞了。

  而在 size_lock 方案里，Thread A 的长 I/O 持的是 size_lock.read()，不是
  inner.read()。所以至少 revalidate_attr() 的 TTL 检查、metadata()/mode()/owner() 这
  类 metadata 读取不会被 data I/O 卡住。真正需要等的是 commit_attr_reply() 中涉及
  size/page-cache 一致性的部分。

  所以区别不是“现在完全不等”，而是：

  - 用 size_lock：等待点表达的是 size/page-cache/data I/O 一致性。
  - 用 inner：等待点扩大成 所有 metadata 读写都和 data I/O 互斥。