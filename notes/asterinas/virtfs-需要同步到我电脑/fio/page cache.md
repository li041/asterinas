# Linux 页缓存 I/O 路径与状态管理

## Buffered read

普通文件的缓存读大致走下面这条路径：

```c
read/readv
  -> file->f_op->read_iter
  -> generic_file_read_iter
  -> filemap_read
  -> filemap_get_pages
  -> copy_folio_to_iter
```

入口在 `mm/filemap.c:2957`。

`filemap_read()` 本身不直接做磁盘 I/O。它先通过 `filemap_get_pages()` 从 `mapping->i_pages` 查找目标范围内的 folio。

这里有三种情况：

1. **Cache hit**：folio 已经 Uptodate，直接复制内容到用户 buffer。

2. **Cache 中存在 folio，但未 Uptodate**：通常意味着 folio 正在被预读或之前的读入未完成。当前线程等待 folio 解锁后重新检查 Uptodate。

3. **Cache miss**：内核先调用 `page_cache_sync_ra()` 尝试同步预读。预读后如果仍然没有目标 folio，才由 `filemap_create_folio()` 分配并插入新 folio，再调用文件系统的 `read_folio()`。

这部分代码集中在 `mm/filemap.c:2668`。

### 新 folio 的读入流程

新 folio 插入 page cache 时已处于 Locked 状态：

```
不存在
  -> 分配 folio
  -> 插入 mapping->i_pages
  -> Locked + !Uptodate
```

文件系统拿到锁定的 folio 后提交读 I/O。I/O 完成时调用：

```c
folio_end_read(folio, success);
```

- **成功**：设置 Uptodate，清除 Locked，唤醒等待者
- **失败**：仅清除 Locked，不设置 Uptodate

实现位于 `mm/filemap.c:1517`。

### 读完成状态判断

读完成不是通过单独的“读取完成状态”表示，而是通过两个条件判断：

| 状态 | 含义 |
|------|------|
| Locked | 仍在初始化或进行 I/O |
| Uptodate | 内容可以被读取 |

等待者被唤醒后，如果发现 folio 仍然不是 Uptodate，则视为 `-EIO`（参见 `mm/filemap.c:2492`）。

---

## Buffered write

普通缓存写的主要路径：

```c
write/writev
  -> generic_file_write_iter
  -> __generic_file_write_iter
  -> generic_perform_write
  -> write_begin
  -> copy_folio_from_iter_atomic
  -> write_end
```

核心循环在 `mm/filemap.c:4297`。

### write_begin()

`write_begin()` 的任务是返回一个已锁定、可写入的 folio。常用获取参数：

```c
FGP_WRITEBEGIN = FGP_LOCK | FGP_WRITE | FGP_CREAT | FGP_STABLE;
```

行为：
- folio 不存在则创建
- 正在 writeback 时，根据文件系统和体系结构要求等待内容稳定

### 部分页写入

如果一次写操作只覆盖 folio 的一部分，**未覆盖区域必须保留原有内容**。因此 `write_begin()` 可能需要：
- 先读取对应磁盘块，或
- 对新分配区域进行清零

只有完整覆盖某个块或 folio 时，才可能省略旧数据读取。

### write_end()

以 iomap 为例（`fs/iomap/buffered-io.c:1030`）：

1. 设置已写范围为 Uptodate
2. 设置已写范围为 Dirty
3. 设置 folio 的 Dirty
4. 更新文件大小
5. 解锁 folio

### Dirty 标记

标记 folio 为 Dirty 不只是设置 `PG_dirty`，`filemap_dirty_folio()` 还会：
- 设置 `PAGECACHE_TAG_DIRTY`
- 将 inode 标记为 `I_DIRTY_PAGES`

参见 `mm/page-writeback.c:2714`。

### Buffered write 返回时的状态

数据通常尚未写入存储，典型状态：

```
Uptodate + Dirty + !Writeback
```

---

## Writeback

Writeback 可能由以下方式触发：
- 后台回写线程
- 内存回收
- `fsync()`
- 同步写

### 大体流程

```
扫描 PAGECACHE_TAG_DIRTY
  -> 锁住 folio
  -> folio_clear_dirty_for_io
  -> folio_start_writeback
  -> 提交写 I/O
  -> 解锁 folio
  -> I/O 完成
  -> folio_end_writeback
```

通用入口 `filemap_fdatawrite_range()` 最终调用文件系统的 `writepages()`，见 `mm/filemap.c:371`。

### 状态转换中的短暂不一致

回写开始前，内核调用 `folio_clear_dirty_for_io()` 清除 folio 的 Dirty。此过程中有一个**有意保留的短暂不一致**：

- folio 的 `PG_dirty` 已清除
- XArray 的 `DIRTY` tag 暂时还在

> 目的：避免并发的同步回写扫描漏掉这个正在进行状态转换的 folio。

相关说明在 `mm/page-writeback.c:2858`。

### folio_start_writeback()

设置：
- `PG_writeback`
- `PAGECACHE_TAG_WRITEBACK`

同时清理旧的 Dirty/TOWRITE tag，见 `mm/page-writeback.c:2993`。

### I/O 完成

`folio_end_writeback()` 清除 Writeback 状态并唤醒等待者，见 `mm/filemap.c:1675`。

### Dirty 与 Writeback 共存

**Dirty 和 Writeback 并不互斥**。folio 正在回写时，用户仍可能通过 mmap 或其他路径再次修改它：

```
Uptodate + Dirty + Writeback
```

当前写 I/O 完成后只清除 Writeback，新产生的 Dirty 会保留，后续还需再执行一次 writeback。

---

## 页状态变化

```
不在 page cache
    |
    | 分配并插入 XArray
    v
Locked (!Uptodate)
    |
    | read_folio / readahead
    v
Locked + 正在读 I/O
    |
    | folio_end_read(true)
    v
Uptodate + Clean
    |
    | buffered write / mmap write
    v
Uptodate + Dirty
    |
    | clear_dirty_for_io + start_writeback
    v
Uptodate + Writeback
    |
    | folio_end_writeback
    v
Uptodate + Clean
    |
    | reclaim / truncate / invalidate
    v
不在 page cache
```

### 读失败路径

```
Locked + !Uptodate
    -> folio_end_read(false)
    -> !Locked + !Uptodate
```

后续访问可能重新读取，也可能直接回收这个 folio。

### 错误处理

当前主线**没有通用的 PG_error 状态**：
- **读错误**：表现为 folio 解锁后仍然 `!Uptodate`
- **异步写错误**：记录到 `address_space::wb_err`，由 `fsync()` 或 `filemap_write_and_wait_range()` 向用户报告

---

## Uptodate 的含义

**Uptodate 只表示内存中的每个字节至少和后端存储一样新**：

| 状态                           | 含义                |
| ---------------------------- | ----------------- |
| Uptodate + Clean             | 内存和存储一致           |
| Uptodate + Dirty             | 内存比存储新            |
| Uptodate + Writeback         | 内存数据正在写向存储        |
| Uptodate + Dirty + Writeback | 旧数据正在回写，同时又产生了新修改 |
