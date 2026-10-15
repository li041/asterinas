
## Changes in this PR
- Fix `pseudo_inode.c`, `pseudo_mount.c` and `pidfd.c` testcases to pass under linux
- add `mode` argument to `AnonInodeFS::shared_inode`, becasue although diffent fds may share the same `anon_inode`, but they may have different mode, e.g. `epoll_fd` inode mode is 0o444.
- add `PidfdFs` and `PidfdInode` to support below trait:
	- return `EPERM` when chmodat pid file
	- pid file's mode is 0o700
	- Besides, linux has this virtual fs.

This may not work. `AnonInode` init once, and share the same inode.
```
pub fn shared_inode() -> &'static Arc<dyn Inode> {
    static SHARED_INODE: Once<Arc<dyn Inode>> = Once::new();
    SHARED_INODE.call_once(|| {
	    ...
    })
}
```
- send_signal
To be honest, I don't find a good way to do so.
`kill` syscall populate siginfo in `do_sys_kill`, and then pass it to `kill_*`. 
```
pub fn do_sys_kill(filter: ProcessFilter, sig_num: Option<SigNum>, ctx: &Context) -> Result<()> {
    let signal = sig_num.map(|sig_num| {
        let pid = ctx.process.pid();
        let uid = ctx.posix_thread.credentials().ruid();
        UserSignal::new(sig_num, UserSignalKind::Kill, pid, uid)
    });
```
It may be a good choice that if `signal` argument is null, then use default one. However, it means only check perm but do not enqueue signal in asterinas.
```
fn kill_process<S: Signal + Clone + Copy>(
    process: &Process,
    signal: Option<S>,
    ctx: &Context,
) -> Result<()> {
	...
    check_signal_perm(target_main_thread.as_posix_thread().unwrap(), ctx, signum)?;
    ...
    if let Some(signal) = signal {
        process.enqueue_signal(signal);
    }
}
```

### 1. **`pidfd_send_signal` 和 `pidfd_open` 的区别**

- **`pidfd_open`**：  
    `pidfd_open` 是用来创建一个 **进程级别的** `pidfd`（PID 文件描述符），它仅用于与进程进行交互。具体来说，`pidfd_open` 只能用于打开**整个进程**，因此它可以用来控制进程的生命周期，发送进程级别的信号等操作。
    
    - 它的作用是**针对进程**，因为进程的 PID 是唯一标识一个进程的，所以 `pidfd_open` 只能针对整个进程。
        
    - 使用方式：
        
        `int pidfd = pidfd_open(pid, 0);`
        
- **`pidfd_send_signal`**：  
    `pidfd_send_signal` 是用来向进程或线程发送信号的接口。它可以通过 `pidfd` 来指定目标，且可以发送信号给**整个进程**或者**特定线程**。如果你通过 `pidfd` 打开了一个进程的文件描述符，它会默认向进程发送信号；如果你通过 `pidfd` 打开了一个线程的文件描述符（通常是通过 `/proc/<pid>/task/<tid>` 目录获取的），则信号会被发送到特定的线程。
    
    - 通过 `pidfd` 打开的是**进程**的文件描述符时，发送的是进程级信号。
        
    - 通过 `pidfd` 打开的是**线程**的文件描述符时，发送的是线程级信号。
        
    - 使用方式：
        
        `pidfd_send_signal(pidfd, sig, 0);`
        

### 2. **`/proc/<pid>` 和 `/proc/<pid>/task/<tid>` 的角色**

- **`/proc/<pid>`**：  
    这个目录表示整个进程的信息，进程的资源（如内存、打开的文件、信号等）是由进程内所有线程共享的。所以，通过 `/proc/<pid>`，你获取的是与进程相关的所有信息，且对进程发送信号时也是基于整个进程的 PID。
    
- **`/proc/<pid>/task/<tid>`**：  
    每个线程都有一个独立的 `/proc/<pid>/task/<tid>` 目录。每个线程在其 `task/<tid>` 子目录下有独立的信息，例如线程的栈、调度状态、信号信息等。因此，使用 `/proc/<pid>/task/<tid>` 可以指向**特定线程**，从而实现对单个线程的操作，例如调度、信号发送等。
    
    - 通过 `pidfd_open` 打开 `/proc/<pid>/task/<tid>` 目录下的 `tid`，会返回**该线程的 `pidfd`**。这是基于 **线程** 来控制信号的。

## vsock

## virtio-fs
```
/home/sutao/linux-td/**qemu**/build/**qemu**-system-x86_64 -name sandbox-4ca4a92ed4df875d774c5b8dd4edfdd8f4829e5d196bc565bf8ef94dc4aab435,debug-threads=on -uuid 8a74ecd7-3c40-464d-9273-4828a3591268 -machine q35,accel=kvm,nvdimm=on -cpu host,pmu=off -qmp unix:fd=3,server=on,wait=off -m 2048M,slots=10,maxmem=258094M -device pci-bridge,bus=pcie.0,id=pci-bridge-0,chassis_nr=1,shpc=off,addr=2,io-reserve=4k,mem-reserve=1m,pref64-reserve=1m -device virtio-serial-pci,disable-modern=false,id=serial0 -device virtconsole,chardev=charconsole0,id=console0 -chardev socket,id=charconsole0,path=/run/vc/vm/4ca4a92ed4df875d774c5b8dd4edfdd8f4829e5d196bc565bf8ef94dc4aab435/console.sock,server=on,wait=off -device nvdimm,id=nv0,memdev=mem0,unarmed=on -object memory-backend-file,id=mem0,mem-path=/opt/kata/share/kata-containers/kata-ubuntu-noble-confidential.image,size=268435456,readonly=on -device virtio-scsi-pci,id=scsi0,disable-modern=false -object rng-random,id=rng0,filename=/dev/urandom -device virtio-rng-pci,rng=rng0 -device vhost-vsock-pci,disable-modern=false,vhostfd=4,id=vsock-3636178767,guest-cid=3636178767 -netdev tap,id=network-0,vhost=on,vhostfds=5,fds=6 -device driver=virtio-net-pci,netdev=network-0,mac=2e:de:af:61:11:db,disable-modern=false,mq=on,vectors=4 -rtc base=utc,driftfix=slew,clock=host -global kvm-pit.lost_tick_policy=discard -vga none -no-user-config -nodefaults -nographic --no-reboot -object memory-backend-ram,id=dimm1,size=2048M -numa node,memdev=dimm1 -kernel /opt/kata/share/kata-containers/vmlinuz-6.16.7-172-confidential -append tsc=reliable no_timer_check rcupdate.rcu_expedited=1 i8042.direct=1 i8042.dumbkbd=1 i8042.nopnp=1 i8042.noaux=1 noreplace-smp reboot=k cryptomgr.notests net.ifnames=0 pci=lastbus=0 root=/dev/pmem0p1 rootflags=dax,data=ordered,errors=remount-ro ro rootfstype=ext4 console=hvc0 console=hvc1 debug systemd.show_status=true systemd.log_level=debug panic=1 nr_cpus=1 selinux=0 systemd.unit=kata-containers.target systemd.mask=systemd-networkd.service systemd.mask=systemd-networkd.socket scsi_mod.scan=none agent.log=debug agent.debug_console agent.debug_console_vport=1026 rootfs_verity.scheme=dm-verity rootfs_verity.hash=c86b0e4e614d20df867de83665f7fdad7aaba397197d0daadcf00b98ed547958 -pidfile /run/vc/vm/4ca4a92ed4df875d774c5b8dd4edfdd8f4829e5d196bc565bf8ef94dc4aab435/pid -smp 1,cores=1,threads=1,sockets=1,maxcpus=1 -serial chardev:mux -chardev stdio,id=mux,mux=on,logfile=/home/sutao/k8s/log/**qemu**.log
```