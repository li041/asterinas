- 目前还有两major和一minor没有解决

>  **A daemon that actually daemonizes escapes supervision entirely**
- 这个`manifest.md`对deamons增加了必须前台挂着的描述, 目前virtio-fs的相应sh最后是exec支持virtiofsd, 不会逃离

>  **The one-second `STARTUP_DELAY` is the only synchronization before QEMU connects**
- 不知道qemu.with_deamons如何检查ready, 对于virtiofsd来说过程中会创建的socket path, 但是其他deamons不一定, 在不引入特定daemon的假设时, 目前采取等待`STARTUP_DELAY`
	- 现在加了个FIXME说明这个问题, 设计上没有改变

> `wait_until_guest_kernel_shutdown` is a nested `fn` declared in the middle of the already-long `run_qemu_and_wait` 

- Intentionally keep this to minimize the tructual changes to `bundle/mod.rs`. See https://github.com/asterinas/asterinas/pull/3780#discussion_r4013872242 for details.