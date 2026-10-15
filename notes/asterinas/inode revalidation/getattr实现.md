  VFS 路径

  stat/statx/fstat 最终走 `vfs_getattr()`，然后` vfs_getattr_nosec()`(不做安全检查) 如果 inode 有 `i_op->getattr` 就调用它；否则使用默认实现 `generic_fillattr()` 从VFS层缓存的 inode 字段填充。
  - 调用 filesystem getattr：fs/stat.c#210-216
    https://codebrowser.dev/linux/linux/fs/stat.c.html#210
  - fallback 到 generic_fillattr()：fs/stat.c#217-218
    https://codebrowser.dev/linux/linux/fs/stat.c.html#217
  - Linux 注释也明确说 remote fs 可以用 statx sync flags 强制更新属性：fs/stat.c#243-250
    https://codebrowser.dev/linux/linux/fs/stat.c.html#243