  - Makefile
      - 新增 XFSTESTS_BACKEND ?= block。
      - 当 XFSTESTS_BACKEND=virtiofs 时，自动启用：
          - VIRTIOFS=on
          - VIRTIOFS_TAG=xfstest
          - VIRTIOFS_SCRATCH=on
          - VIRTIOFS_SCRATCH_TAG=xfsscratch
          - 默认 runlist 为 /opt/xfstests/virtiofs.list
      - make run_kernel 在 virtiofs xfstests 场景下会自动启动两个 virtiofsd：
          - 一个作为 TEST_DEV
          - 一个作为 SCRATCH_DEV
      - 测试结束后自动 kill virtiofsd，删除 socket。
  - tools/qemu_args.sh
      - virtiofs 后端不再挂 xfstests 的 block disk image。
      - 支持额外挂一个 scratch virtio-fs 设备。
      - QEMU 会自动加上两个 vhost-user-fs-pci 设备。
  - test/initramfs/Makefile
      - 只有 block 后端才构建 xfstests disk image。
      - virtiofs 后端跳过 disk image 构建。
  - test/initramfs/src/conformance/xfstests/local.config
      - block 后端继续用 ext2。
      - virtiofs 后端设置 FSTYP=virtiofs，并清空 mkfs/mount 选项。
  - test/initramfs/src/conformance/xfstests/run_xfstests.sh
      - 支持 XFSTESTS_BACKEND=block|virtiofs。
      - block 后端保持原来的 mount block device 逻辑。
      - virtiofs 后端会先 probe 两个 tag 是否能 mount，然后 unmount，让 xfstests 自己管理 mount 生命周期。
      - virtiofs 后端调用 ./check 时加 -virtiofs。
      - 加了一个只在 virtiofs 后端启用的 df wrapper，因为当前 guest 里 /proc/mounts 能看到 virtiofs，但 df -T -P 不显示 virtiofs mount，xfstests 会因此误判 filesystem type。
  - 新增 test/initramfs/src/conformance/xfstests/virtiofs.list
      - 目前只放了：

        generic/002

      - 原因是我试了更大的列表，当前 virtiofs 还有真实失败：
          - generic/001 有输出 mismatch
          - generic/006 会触发 kernel panic：BufferTooSmall
      - 所以现在先放一个能稳定通过的 smoke regression。

  验证过：

  make --no-print-directory -C test/initramfs ENABLE_CONFORMANCE_TEST=true CONFORMANCE_TEST_SUITE=xfstests XFSTESTS_BACKEND=virtiofs
  git diff --check
  make run_kernel AUTO_TEST=conformance CONFORMANCE_TEST_SUITE=xfstests XFSTESTS_BACKEND=virtiofs BOOT_PROTOCOL=linux-efi-handover64

## 让virtio-fs跑ext2相同的list
```
make run_kernel \
    AUTO_TEST=conformance \
    CONFORMANCE_TEST_SUITE=xfstests \
    XFSTESTS_BACKEND=virtiofs \
    XFSTESTS_RUNLIST=/opt/xfstests/short.list \
    BOOT_PROTOCOL=linux-efi-handover64
```

## 测例源码位置
```
• 测例源码就在 Nix 的 xfstests 包里。当前可看的稳定路径是：

  /nix/store/2zra5p2wihl72i3krb505yaxls98kzyl-xfstests-2023.05.14/lib/xfstests/tests/generic/006
  /nix/store/2zra5p2wihl72i3krb505yaxls98kzyl-xfstests-2023.05.14/lib/xfstests/tests/generic/002
```

## xfstests
- `short.list`是ext2使用的
- virtiofs在此基础上不做
	- rename
		- 035
		- 089
	- symlinks supprot
		- 005
		- 023
		- 109
		- 360
		- 401
	- mknod/mkinfo
		- 184
		- 453
	- `[not run] Filesystem virtiofs not supported in _scratch_mkfs_sized`
		- 这个是本身就不测的, virtiofs不支持mkfs
		- 224
		- 273
		- 320
		- 488
	- `O_APPNED`
		- 069
			- 能过, 但是3, 4分钟, 3000000次O_APPEND需要想host写
	- `cache write-through`路径会触发整页写回, 导致size变大
		- 这个需要后续page cache支持dirty byte range粒度的flush
		- 246
	- `st_nlink`在unlink后继续命中旧缓存
		- `cache=none`的时候能过
		- 002
	- 极大文件偏移写入测试
		- 308
			- 这个报了个error日志, 是预期行为
	- falloc
		- 568
		- 619

## 日志
```
Running xfstests...
FSTYP         -- virtiofs
PLATFORM      -- Linux/x86_64 (none) 5.13.0 #1 SMP Thu Jan  1 00:00:00 UTC 1970
MKFS_OPTIONS  -- xfsscratch
MOUNT_OPTIONS -- xfsscratch /opt/xfstests/scratch

generic/001       - output mismatch (see /opt/xfstests/results//generic/001.out.bad)
    --- tests/generic/001.out
    +++ /opt/xfstests/results//generic/001.out.bad
    @@ -1,9 +1,49 @@
     QA output created by 001
     cleanup
     setup ....................................
    -iter 1 chain ... check ....................................
    -iter 2 chain ... check ....................................
    -iter 3 chain ... check ....................................
    -iter 4 chain ... check ....................................
    ...
    (Run 'diff -u /opt/xfstests/tests/generic/001.out /opt/xfstests/results//generic/001.out.bad'  to see the entire diff)
generic/002        2s
generic/005       [not run] Require symlinks support
generic/006        5s
generic/007        9s
generic/013        1s
generic/014        3s
generic/023       [not run] Require symlinks support
generic/028        6s
generic/069        297s
generic/089       - output mismatch (see /opt/xfstests/results//generic/089.out.bad)
    --- tests/generic/089.out
    +++ /opt/xfstests/results//generic/089.out.bad
    @@ -1,19 +1,30472 @@
     QA output created by 089
    +can't rename t_mtab.tmp to t_mtab: Not a directory
    +can't rename t_mtab.tmp to t_mtab: Not a directory
    +can't rename t_mtab.tmp to t_mtab: Not a directory
    +can't rename t_mtab.tmp to t_mtab: Not a directory
    +can't rename t_mtab.tmp to t_mtab: Not a directory
    +can't rename t_mtab.tmp to t_mtab: Not a directory
    ...
    (Run 'diff -u /opt/xfstests/tests/generic/089.out /opt/xfstests/results//generic/089.out.bad'  to see the entire diff)
generic/109       [not run] Require symlinks support
generic/124        9s
generic/126        1s
generic/132        15s
generic/141        1s
generic/184       [not run] virtiofs does not support mknod/mkfifo
generic/221        2s
generic/224       [not run] Filesystem virtiofs not supported in _scratch_mkfs_sized
generic/236        2s
generic/246        0s
generic/248        0s
generic/249        2s
generic/273       [not run] Filesystem virtiofs not supported in _scratch_mkfs_sized
generic/308        1s
generic/309       [failed, exit status 2]- output mismatch (see /opt/xfstests/results//generic/309.out.bad)
    --- tests/generic/309.out
    +++ /opt/xfstests/results//generic/309.out.bad
    @@ -1,2 +1,5 @@
     QA output created by 309
     Silence is golden
    +mv: cannot move '/opt/xfstests/test/testfile.309' to '/opt/xfstests/test/testdir_309/testfile': Not a directory
    +mtime not updated
    +ctime not updated
    ...
    (Run 'diff -u /opt/xfstests/tests/generic/309.out /opt/xfstests/results//generic/309.out.bad'  to see the entire diff)
generic/313        5s
generic/320       [not run] Filesystem virtiofs not supported in _scratch_mkfs_sized
generic/339        1s
generic/340        31s
generic/346       - output mismatch (see /opt/xfstests/results//generic/346.out.bad)
    --- tests/generic/346.out
    +++ /opt/xfstests/results//generic/346.out.bad
    @@ -4,7 +4,263 @@
     INFO: sz = 1048576
     INFO: thread 0 created
     INFO: thread 1 created
    -INFO: 0 error(s) detected
    +ERROR: thread 0, offset 00000400, 00000000 != 7ffffec7c6c0
    +ERROR: thread 0, offset 00001400, 00000000 != 7ffffec7c6c0
    +ERROR: thread 0, offset 00002400, 00000000 != 7ffffec7c6c0
    ...
    (Run 'diff -u /opt/xfstests/tests/generic/346.out /opt/xfstests/results//generic/346.out.bad'  to see the entire diff)
generic/360       [not run] Require symlinks support
generic/401       [not run] Require symlinks support
generic/406       - output mismatch (see /opt/xfstests/results//generic/406.out.bad)
    --- tests/generic/406.out
    +++ /opt/xfstests/results//generic/406.out.bad
    @@ -1,2 +1,4 @@
     QA output created by 406
    +/bin/sh: can't open 'df': No such file or directory
    +./common/rc: line 2842: [: -lt: unary operator expected
     Silence is golden
    ...
    (Run 'diff -u /opt/xfstests/tests/generic/406.out /opt/xfstests/results//generic/406.out.bad'  to see the entire diff)
generic/428       - output mismatch (see /opt/xfstests/results//generic/428.out.bad)
    --- tests/generic/428.out
    +++ /opt/xfstests/results//generic/428.out.bad
    @@ -1,2 +1,3 @@
     QA output created by 428
    +strncmp mismatch: 'HELLO WORLD!' vs ''
     Silence is golden
    ...
    (Run 'diff -u /opt/xfstests/tests/generic/428.out /opt/xfstests/results//generic/428.out.bad'  to see the entire diff)
generic/437        6s
generic/438        17s
generic/443        1s
generic/453       [not run] virtiofs does not allow unrestricted byte streams for names
generic/488       [not run] Filesystem virtiofs not supported in _scratch_mkfs_sized
generic/532        1s
generic/568       [not run] xfs_io falloc  failed (old kernel/wrong fs?)
generic/609        1s
generic/615        117s
generic/619       [not run] xfs_io falloc  failed (old kernel/wrong fs?)
generic/696        1s
Ran: generic/001 generic/002 generic/005 generic/006 generic/007 generic/013 generic/014 generic/023 generic/028 generic/069 generic/089 generic/109 generic/124 generic/126 generic/132 generic/141 generic/184 generic/221 generic/224 generic/236 generic/246 generic/248 generic/249 generic/273 generic/308 generic/309 generic/313 generic/320 generic/339 generic/340 generic/346 generic/360 generic/401 generic/406 generic/428 generic/437 generic/438 generic/443 generic/453 generic/488 generic/532 generic/568 generic/609 generic/615 generic/619 generic/696
Not run: generic/005 generic/023 generic/109 generic/184 generic/224 generic/273 generic/320 generic/360 generic/401 generic/453 generic/488 generic/568 generic/619
Failures: generic/001 generic/089 generic/309 generic/346 generic/406 generic/428
Failed 6 of 46 tests

Error: xfstests failed.
Conformance test failed
make: *** [Makefile:297: run_kernel] Error 1
```

## claude分析

#### 问题分类

| 类别                | 涉及测例          | 推测根因                                 |
| ----------------- | ------------- | ------------------------------------ |
| **rename 语义异常**   | 001, 089, 309 | virtiofs 对 `rename` 系统调用的目录/文件类型判断有误 |
| **mmap 数据一致性**    | 346           | virtiofs mmap 写入后数据未正确回写或页缓存未同步      |
| **xattr / 字符串写入** | 428           | virtiofs 扩展属性或普通写入后读取为空              |
| **待进一步分析**        | 001           | 需查看完整 diff 确认具体失败步骤                  |

---

#### 不适用（Not Run）说明

共 13 个测例未运行，原因集中在：

- **需要 symlink 支持**（005/023/109/360/401）
- **virtiofs 不支持 mknod/mkfifo**（184/453）
- **virtiofs 不支持 `_scratch_mkfs_sized`**（224/273/320/488）
- **内核/文件系统不支持 `falloc`**（568/619）

## 新增测试后

## 没有跑的测试

|原因|数量|测试项|
|---|---|---|
|virtiofs 不适合/不支持该文件系统类型|1|ext4/042|
|缺少 symlink 支持|5|generic/005, generic/023, generic/109, generic/360, generic/401|
|virtiofs 不支持 _scratch_mkfs_sized|7|generic/102, generic/224, generic/273, generic/275, generic/320, generic/488|
|virtiofs 不支持 mknod/mkfifo|1|generic/184|
|virtiofs 的 atime mount option 无效|1|generic/192|
|xfs_io falloc 不可用或 filesystem 不支持 fallocate|4|generic/214, generic/371, generic/568, generic/619|
|virtiofs 不允许 unrestricted byte streams for names|1|generic/453|
|需要有效 block disk scratch 设备|1|generic/466|

## 失败的测试

| 具体原因                      | 数量  | 测试项         | 现象/判断                                                                                                                                                                                                   |
| ------------------------- | --- | ----------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| mmap 写入扩展区域后数据丢失          | 1   | generic/030 | 期望 offset 0x4e6400 开始是 0x59（即 Y），实际是全 0x00。此位置正好是 5017K，对应测试里 truncate up + mremap + mwrite 写入新扩展区 [5017K, 5020K)，说明 mmap 写入新扩展范围的数据没有正确保留/写回。                                                          |
| O_APPEND 测试的检查输出缺失        | 1   | generic/069 | 期望出现 `*** checking file with 1/20/300/...` integers，实际缺失。该测试并发创建 append_writer，再读取 pids 文件逐个校验。当前输出像检查循环没有正常跑起来，常见原因是 pids 文件为空/不可读/内容未按预期写入；需要完整 .out.bad 才能确认是否还有 append_reader 报错。                   |
| fstest mmap workload 中途失败 | 1   | generic/074 | 输出停在 `fstest.1 : -l loops -s 10485760 -b 8192 -m` 后，后续 fstest.2 到 fstest.4 都没跑。说明 fstest.1 的 mmap 写 10MiB、block size 8192 的 workload 失败，或其后的 _check_test_fs 失败。                                         |
| rename/mtab 并发元数据测试输出不一致  | 1   | generic/089 | 日志只显示 "Files differ"，无具体差异，无法判断是少了 completed N iterations、多了临时 mtab 文件还是 t_mtab 程序报错。需执行 `diff -u /opt/xfstests/tests/generic/089.out /opt/xfstests/results/generic/089.out.bad` 进一步排查。这个是`O_APPEND`的问题 |
| 覆盖 rename 后目录时间戳未更新       | 1   | generic/309 | 明确多出 mtime not updated 和 ctime not updated。用例测试 `mv file dir/existing_file` 覆盖目录内已有文件时，父目录 mtime/ctime 是否更新；当前两个时间戳均未变。                                                                                 |
|                           |     |             |                                                                                                                                                                                                         |
