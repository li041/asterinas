  fuse_inode 里有每个 inode 的 attr_version：fs/fuse/fuse_i.h#148 (https://codebrowser.dev/linux/linux/fs/fuse/fuse_i.h.html#148)
  fuse_conn 里有全局 attr_version 计数器：fs/fuse/fuse_i.h#945 (https://codebrowser.dev/linux/linux/fs/fuse/fuse_i.h.html#945)
  连接初始化时全局版本从 1 开始：fs/fuse/inode.c#994 (https://codebrowser.dev/linux/linux/fs/fuse/inode.c.html#994)
  读取当前全局版本只是 atomic64_read()，不是递增：fs/fuse/fuse_i.h#1070 (https://codebrowser.dev/linux/linux/fs/fuse/fuse_i.h.html#1070)

  普通 GETATTR/LOOKUP 的流程
  以 GETATTR 为例，请求发出去之前，Linux 先读取当前全局 attr_version，把它作为这个请求的 snapshot：fs/fuse/dir.c#1484 (https://codebrowser.dev/linux/linux/fs/fuse/dir.c.html#1484)
  请求返回且 attr 合法后，把 reply 和这个 snapshot 一起传给 fuse_change_attributes()：fs/fuse/dir.c#1510 (https://codebrowser.dev/linux/linux/fs/fuse/dir.c.html#1510)

  LOOKUP/dentry revalidate 也是同样模式：请求前读 snapshot：fs/fuse/dir.c#422 (https://codebrowser.dev/linux/linux/fs/fuse/dir.c.html#422)
  确认还是同一个 inode 后，用 reply 刷新 attr：fs/fuse/dir.c#449 (https://codebrowser.dev/linux/linux/fs/fuse/dir.c.html#449)

  真正防 stale 的检查在 fuse_change_attributes_i()。它拿 fi->lock 后检查：
      return;

  也就是：如果 inode 当前版本已经大于请求发出时的 snapshot，说明请求期间已有更新提交过，这个 reply 不能再覆盖当前 attr。源码：fs/fuse/inode.c#357 (https://codebrowser.dev/linux/linux/fs/fuse/inode.c.html#357)

  如果检查通过，fuse_change_attributes_common() 才真正提交 attr。提交时它递增全局版本，并把新版本写入 inode：fs/fuse/inode.c#248 (https://codebrowser.dev/linux/linux/fs/fuse/inode.c.html#248)
  同一个函数还更新 attr timeout i_time，并写入 mode、nlink、uid、gid、blocks、atime/mtime/ctime 等字段：fs/fuse/inode.c#249 (https://codebrowser.dev/linux/linux/fs/fuse/inode.c.html#249)

  所以关键点是：

  - 请求前：读取全局版本作为 snapshot。
  - 回复提交前：检查 inode 是否已被更新。
  - 真正提交时：递增全局版本并赋给 inode。

  这和“请求前 fetch_add 分配版本号”不是一个语义。

  SETATTR 为什么特殊
  SETATTR 是有副作用的操作，比如 chmod/chown/truncate。它也在发请求前读 snapshot：fs/fuse/dir.c#2235 (https://codebrowser.dev/linux/linux/fs/fuse/dir.c.html#2235)
  但成功返回后，如果发现 fi->attr_version > attr_version，Linux 不直接丢掉 reply，而是把返回 attr 的 timeout 置零：fs/fuse/dir.c#2261 (https://codebrowser.dev/linux/linux/fs/fuse/dir.c.html#2261)

  随后它仍然调用 fuse_change_attributes_common() 应用属性：fs/fuse/dir.c#2269 (https://codebrowser.dev/linux/linux/fs/fuse/dir.c.html#2269)

  这很重要：SETATTR 已经在服务端成功了，内核不能像普通 GETATTR 一样完全丢弃结果，否则 chmod/chown/truncate 成功后本地 inode 可能不反映这次操作。Linux 的折中是：应用这次成功操作的 attr，但不给它长期缓存有效期，下一次需要 attr 时会重新向 server 拉。

  写路径如何配合
  写成功后，Linux 也会 bump inode 的 attr_version，因为本地写可能改变 size/mtime/ctime/blocks：fs/fuse/file.c#1175 (https://codebrowser.dev/linux/linux/fs/fuse/file.c.html#1175)
  如果写扩展了文件，还会更新 i_size：fs/fuse/file.c#1177 (https://codebrowser.dev/linux/linux/fs/fuse/file.c.html#1177)
  然后它把 mtime/ctime/blocks/size 相关 attr 标记为 invalid：fs/fuse/file.c#1183 (https://codebrowser.dev/linux/linux/fs/fuse/file.c.html#1183)
  这些字段集合定义为 FUSE_STATX_MODSIZE：fs/fuse/fuse_i.h#1303 (https://codebrowser.dev/linux/linux/fs/fuse/fuse_i.h.html#1303)

  这保证了：本地写不会被一个旧 GETATTR reply 覆盖，同时后续 stat 不会长期相信写前的 mtime/ctime/blocks。

  一句话总结
  Linux 用 attr_version 做的是“请求开始时拍一个全局 attr epoch 快照，回复提交时确认 inode 没有在这期间被更新过”。普通查询 reply 过期就丢；有副作用的 SETATTR 成功 reply 仍应用，但把缓存有效期清零；本地写和服务端 invalidation 会 bump 版本并失效相关 attr。这样避免 stale FUSE reply 覆盖更新后的 inode
  metadata。
