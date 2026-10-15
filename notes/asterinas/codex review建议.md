- [ ] FuseOpcode的使用, 有enum和consts, 可以考虑删除consts
- [ ] entry timeout应该在dentry那一层
- [ ] 关于语义: high kernel/src/fs/vfs/path/dentry.rs:286, kernel/src/fs/vfs/path/resolver.rs:431, kernel/src/fs/fs_impls/virtiofs/fs/inode.rs:150: cached-child revalidation turns a stale dentry into a hard lookup error
    than surfacing the stale-cache condition directly, see /root/linux/fs/fuse/dir.c:384. Smallest safe fix: distinguish “stale cached child” from a real lookup failure and fall back to a fresh lookup after
    evicting the cached child.

- rootfs问题
	- https://github.com/StevenJiang1110/asterinas/pull/56/changes/6fced3356bfd026d29314cb82fc6cd5791577a86

-   我建议的落地顺序
	  - 最小修复先做 BootRootSpec，把默认无参路径修正为 ramfs + initramfs，不要再用 “显式 rootfs=ramfs 才解包” 这个条件。
	  - 然后做 FsType::create() 的结构化参数重构，把 virtiofs 的 source/tag 语义从 syscall 特判里拿出来。
	  - 如果你想更接近 Linux，再进一步把启动参数也统一成 root= 和 rootfstype=，把现在的 rootfs=virtiofs virtiofs_tag=... 当兼容别名，而不是主接口。
- review的问题: 
	- codex resume 019d4ca2-769c-7011-95ea-8f1e3d9ae6a7
- fallocate的问题
	- codex resume 019d4c28-7428-7860-b466-bb10a4c64101
- prctl的问题
	- codex resume 019d4d43-3ee0-7450-9e74-f6210e208a90
```
问题在 waitpid() 能在父进程刚进入 zombie 时就把它 reap 掉，但这时它的 exit_process() 还可能没完成对子孙进程的
  reparent。这样 orphan 随后退出时，send_child_death_signal() 可能还会拿到旧的 zombie 父进程，SIGCHLD 就被丢了，
  所以你会看到 PPID/wait() 都对，但 sigchild_recv 少一次。

  我在 kernel/src/process/exit.rs:144 改了收件人解析逻辑：如果当前父进程已经 zombie，就先等它的 children reparent
  过程结束，再重新读取父进程；如果仍然拿不到活父进程，就退回到 subreaper/init 解析路径。顺手把
  find_reaper_process() 的 upgrade() 从 unwrap() 改成了 Option 返回，避免并发退出时直接 panic。另加了一个本地回归
  用例 test/initramfs/src/apps/process/prctl/subreaper_sigchld.c:31，并接到 test/initramfs/src/apps/process/
  run_test.sh:35，它复现的就是 LTP prctl03 这条 “subreaper + orphan SIGCHLD” 时序。

  验证过 cargo osdk check -p aster-kernel 和新回归程序的本地编译；新程序在宿主 Linux 上也能跑通。还没直接重跑你那
  条 make run_kernel AUTO_TEST=syscall ... 的 QEMU/LTP 组合。
```