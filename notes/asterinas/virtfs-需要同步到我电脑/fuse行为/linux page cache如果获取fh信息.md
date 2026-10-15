```

  ---
  Linux FUSE 的 page cache fh 管理方式

  Linux FUSE 从不为 page cache 专门开一个 inode 级别的 fh。它分两条路：读路径用调用者自己的 fh，写回路径从 inode 维护的可写 fd 列表中取一个。

  ---
  数据结构：per-inode 的 write_files 列表

  struct fuse_inode 在 io cache union 内维护一个链表，专门记录所有可用于 writepage 的打开文件句柄：

  /* Files usable in writepage.  Protected by fi->lock */
  struct list_head write_files;

  fuse_i.h:154 — fi->write_files 定义

  对应地，struct fuse_file 上挂着 list entry：

  fuse_i.h:277 — struct list_head write_entry;（entry on inode's write_files list）

  ---
  链表如何被填充：fuse_link_write_file()

  文件 open 完成后，fuse_finish_open() 检查文件是否以写模式打开且 writeback cache 开启，如果是就调用：

  if ((file->f_mode & FMODE_WRITE) && fc->writeback_cache)
      fuse_link_write_file(file);

  file.c:233-234 — 条件链入 write_files

  static void fuse_link_write_file(struct file *file)   // file.c:203
  {
      struct fuse_inode *fi = get_fuse_inode(inode);
      struct fuse_file *ff = file->private_data;
      spin_lock(&fi->lock);
      if (list_empty(&ff->write_entry))
          list_add(&ff->write_entry, &fi->write_files);  // file.c:214
      spin_unlock(&fi->lock);
  }

  此时 ff->fh 就是 daemon 在 FUSE_OPEN 时返回的 server-side 句柄，直接挂入链表备用。

  ---
  读路径：始终持有调用者的 fh

  fuse_read_folio() 和 fuse_readahead() 都接收 struct file *file（或 rac->file），从中拿到 ff：

  // file.c:1038
  static void fuse_send_readpages(struct fuse_io_args *ia, struct file *file, ...)
  {
      struct fuse_file *ff = file->private_data;   // file.c:1041
      ...
      fuse_read_args_fill(ia, file, pos, count, FUSE_READ);

  读操作调用链上始终有 struct file *，所以 不需要 write_files，也不需要专用 fh。

  ---
  写回路径：从 write_files 取第一个可用 fh

  fuse_writepages() 是 address_space_operations.writepages 回调，没有 file 参数：

  // file.c:2279
  static int fuse_writepages(struct address_space *mapping,
                             struct writeback_control *wbc)

  它通过 iomap 路径进入 fuse_iomap_writeback_folio()，在首次处理 folio 时调用：

  if (!data->ff) {
      data->ff = fuse_write_file_get(fi);   // file.c:2211
      if (!data->ff)
          return -EIO;
  }

  fuse_write_file_get() 的实现非常简单：

  // file.c:2013
  static struct fuse_file *__fuse_write_file_get(struct fuse_inode *fi)
  {
      spin_lock(&fi->lock);
      ff = list_first_entry_or_null(&fi->write_files,
                                     struct fuse_file, write_entry);  // file.c:2018
      if (ff)
          fuse_file_get(ff);
      spin_unlock(&fi->lock);
      return ff;
  }

  即：从现有打开的可写 fd 里抢第一个，加引用计数后直接用于这轮写回。Linux 内核源码注释也承认这个依赖的脆弱性：

  /*
   * Write back dirty data/metadata now (there may not be any suitable
   * open files later for data)
   */
  static void fuse_vma_close(...)    // file.c:2323

  ---
  与 Asterinas 的对比

  ┌──────────────────────┬───────────────────────────────────────┬───────────────────────────────────────┐
  │         维度         │              Linux FUSE               │        Asterinas VirtioFsInode        │
  ├──────────────────────┼───────────────────────────────────────┼───────────────────────────────────────┤
  │ 读页面 fh 来源       │ 调用者的 file->private_data（ff->fh） │ page_cache_fh（inode 级专用 fh）      │
  ├──────────────────────┼───────────────────────────────────────┼───────────────────────────────────────┤
  │ 写回 fh 来源         │ fi->write_files 链表中取第一个        │ 同上 page_cache_fh                    │
  ├──────────────────────┼───────────────────────────────────────┼───────────────────────────────────────┤
  │ 何时打开             │ 用户 open 时（顺带）                  │ 首次 page cache I/O 时按需 FUSE_OPEN  │
  ├──────────────────────┼───────────────────────────────────────┼───────────────────────────────────────┤
  │ 何时关闭             │ 用户 close（fuse_release）时          │ inode drop 时 release_page_cache_fh() │
  ├──────────────────────┼───────────────────────────────────────┼───────────────────────────────────────┤
  │ 无打开 fd 时能否写回 │ 不能（write_files 为空则 -EIO）       │ 可以（独立 fh 不依赖用户 fd）         │
  ├──────────────────────┼───────────────────────────────────────┼───────────────────────────────────────┤
  │ 额外 FUSE_OPEN 开销  │ 无                                    │ 每 inode 一次                         │
  └──────────────────────┴───────────────────────────────────────┴───────────────────────────────────────┘

  Asterinas 的做法在语义上更干净：PageCacheBackend 完全独立于用户态的 fd 生命周期，不存在"脏页无法写回因为 fd 被关了"的问题。Linux 能这样做是因为在
  writeback_cache 模式下，可写 fd 的生命周期与 dirty page 的生命周期被 VFS 层保证足够重叠；但它自己也留了一个 fuse_vma_close 提前刷脏的逻辑作为保险。

  Sources:
  - linux/fs/fuse/fuse_i.h - torvalds/linux
  - linux/fs/fuse/file.c - torvalds/linux
  - fuse: An attempt to implement a write-back cache policy (LWN)
  - writeback-cache mode - kernel.org
```