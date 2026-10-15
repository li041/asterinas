在 virtiofsd cache=auto 下，Linux 对元数据的策略不是“每次操作前都刷新”，而是“在少
  数明确的入口刷新，其他地方靠 timeout / invalid mask / dentry revalidate”。

  可以把它理解成 4 类触发点。

  1. 显式 statx/getattr 时按 timeout 和 invalid mask 决定是否刷新

  这是 Linux 最标准的元数据刷新入口。

  fuse_getattr() 最后走 fuse_update_get_attr()：
  fs/fuse/dir.c#2374 (https://codebrowser.dev/linux/linux/fs/fuse/dir.c.html#2374)
  fs/fuse/dir.c#2397 (https://codebrowser.dev/linux/linux/fs/fuse/dir.c.html#2397)

  fuse_update_get_attr() 会综合这些条件决定 sync：

  - AT_STATX_FORCE_SYNC 强制刷新
  - AT_STATX_DONT_SYNC 不刷新
  - 请求字段命中了 inval_mask 且不在本地可信 cache_mask 里时刷新
  - 否则看 attr timeout 是否过期

  关键判断在：
  fs/fuse/dir.c#1523 (https://codebrowser.dev/linux/linux/fs/fuse/dir.c.html#1523)
  fs/fuse/dir.c#1540 (https://codebrowser.dev/linux/linux/fs/fuse/dir.c.html#1540)
  fs/fuse/dir.c#1544 (https://codebrowser.dev/linux/linux/fs/fuse/dir.c.html#1544)
  fs/fuse/dir.c#1547 (https://codebrowser.dev/linux/linux/fs/fuse/dir.c.html#1547)

  这说明 Linux 的“刷新元数据”主要收口在 getattr/statx 路径，而不是散落在所有 inode
  方法里。

  2. 读路径在 auto invalidate 模式下会主动刷新 size 相关属性

  FUSE 的 cached read 路径里写得很直接：

  - 在 auto invalidate mode 下，读前总是更新 attributes
  - 否则只在本次读取可能越过当前 EOF 时更新，以确保 i_size 最新

  代码在：
  fs/fuse/file.c#1092
  (https://codebrowser.dev/linux/linux/fs/fuse/file.c.html#1092)
  fs/fuse/file.c#1097
  (https://codebrowser.dev/linux/linux/fs/fuse/file.c.html#1097)
  fs/fuse/file.c#1100
  (https://codebrowser.dev/linux/linux/fs/fuse/file.c.html#1100)

  也就是说，cache=auto 这类策略下，Linux 确实会在“读数据”这个入口主动拉一次属性，但
  重点是 STATX_SIZE，不是把所有 metadata getter 都变成 refresh。

  3. dentry revalidate 时会重新 lookup，并在命中同一个 inode 时刷新属性和 timeout

  这是目录项一致性的入口，不是普通 type check。

  Linux 的注释写得很清楚：
  如果 dentry timeout 过期，就 redo lookup；
  如果 lookup 结果还是同一个 inode，就 refresh attributes、timeouts，并把 dentry 重
  新标记有效。

  代码在：
  fs/fuse/dir.c#375 (https://codebrowser.dev/linux/linux/fs/fuse/dir.c.html#375)
  fs/fuse/dir.c#378 (https://codebrowser.dev/linux/linux/fs/fuse/dir.c.html#378)
  fs/fuse/dir.c#382 (https://codebrowser.dev/linux/linux/fs/fuse/dir.c.html#382)

  所以 Linux 的“目录相关元数据刷新”主要挂在 dentry revalidate 上，不是每次 lookup/
  create/unlink/rmdir/readdir 前都先 getattr 一次。

  4. 修改类操作通常先把属性标脏/失效，留给后续 getattr 路径刷新

  Linux 很多地方不是“操作后立刻重新拉全量 metadata”，而是先 invalidation。

  几个典型点：

  - 基础规则：fuse_invalidate_attr() 把 basic stats 标成 stale，等下次 ->getattr()
    再从 userspace 取。
    fs/fuse/dir.c#309 (https://codebrowser.dev/linux/linux/fs/fuse/dir.c.html#309)
    fs/fuse/dir.c#313 (https://codebrowser.dev/linux/linux/fs/fuse/dir.c.html#313)
  - 目录内容变化时，父目录属性失效：
    fs/fuse/dir.c#318 (https://codebrowser.dev/linux/linux/fs/fuse/dir.c.html#318)
    fs/fuse/dir.c#320 (https://codebrowser.dev/linux/linux/fs/fuse/dir.c.html#320)
  - unlink/rmdir/rename 这类会让 dentry entry cache 失效，必要时也更新/失效 ctime：
    fs/fuse/dir.c#334 (https://codebrowser.dev/linux/linux/fs/fuse/dir.c.html#334)
    fs/fuse/dir.c#342 (https://codebrowser.dev/linux/linux/fs/fuse/dir.c.html#342)
    fs/fuse/dir.c#1163
    (https://codebrowser.dev/linux/linux/fs/fuse/dir.c.html#1163)
    fs/fuse/dir.c#1188
    (https://codebrowser.dev/linux/linux/fs/fuse/dir.c.html#1188)
    fs/fuse/dir.c#1232
    (https://codebrowser.dev/linux/linux/fs/fuse/dir.c.html#1232)
  - atime 也不是同步拉远端，而是单独按字段失效：
    fs/fuse/dir.c#324 (https://codebrowser.dev/linux/linux/fs/fuse/dir.c.html#324)
    fs/fuse/dir.c#328 (https://codebrowser.dev/linux/linux/fs/fuse/dir.c.html#328)

  和你当前 virtiofs 的直接对应

  所以如果参考 Linux cache=auto，比较像的约束是：
    revalidate_attr()
  - 把元数据刷新主要收口到：
      - 显式属性读取路径
      - 读数据前的 size 校验路径
      - dentry revalidate
  - 写/目录修改后优先做 attr invalidation，而不是局部手工维护一半 metadata

  换句话说，Linux 是：

  - type check 本身通常不 refresh
  - getattr/statx、read、dentry revalidate 才是 metadata 刷新的主入口
  - 修改操作更多是“标失效”，不是“立即全量同步”
