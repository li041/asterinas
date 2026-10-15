## pid_getattr
  - `/proc/<pid>`相关 base inode operations 挂 `.getattr = pid_getattr：
    https://codebrowser.dev/linux/linux/fs/proc/base.c.html#3450

  - `/proc/<tid>` 相关 base inode operations 也挂 `.getattr = pid_getattr`：fs/proc/base.c#3776-3779
    https://codebrowser.dev/linux/linux/fs/proc/base.c.html#3776
	  - `pid_getattr()` 先 `generic_fillattr()`，然后根据当前 task/hidepid 动态改 uid/gid，权限不满足时返回ENOENT
	    https://codebrowser.dev/linux/linux/fs/proc/base.c.html#2018

## proc_task_getattr

  - `/proc/<pid>/task` 用 `proc_task_getattr()` 动态加线程数到 nlink：fs/proc/base.c#3955-3968
    https://codebrowser.dev/linux/linux/fs/proc/base.c.html#3955

## proc_fd_getattr
  - `/proc/<pid>/fd` 用 `proc_fd_getattr()` 动态计算 fd 目录 size，失败也可返回错误：fs/proc/fd.c#351-367
    https://codebrowser.dev/linux/linux/fs/proc/fd.c.html#351