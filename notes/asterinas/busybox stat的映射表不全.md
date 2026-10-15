-![[Pasted image 20260524151357.png]]

## xfstests检查不成功
```
• xfstests 这里不是用 statfs 直接判断 virtiofs，而是走 df 解析。

  关键路径是：

  1. check -virtiofs 把 FSTYP 设成 virtiofs。
  2. common/config 里设置：

     DF_PROG="$(type -P df)"
     DF_PROG="$DF_PROG -T -P"

  3. common/rc:init_rc() 里先检查：

     if [ -z "`_fs_type $TEST_DEV`" ]; then
         _test_mount
     fi

  4. 然后再校验：

     _check_mounted_on TEST_DEV $TEST_DEV TEST_DIR $TEST_DIR $FSTYP

  5. _check_mounted_on() 里的报错就是你看到的：

     if [ -n "$type" -a "`_fs_type $dev`" != "$type" ]; then
         echo "$devname=$dev is mounted but not a type $type filesystem"
     fi

  6. _fs_type xfstest 实际做的是：

     df -T -P | awk '$1 == "xfstest" { print $2 }'

  所以对于你的配置，xfstests 期望看到类似这一行：

  xfstest virtiofs ... /opt/xfstests/test
```

- 全局没有输出
```
 我跑了自动流程，并临时加 debug 看到了 xfstests 运行时的真实状态。

  关键输出是：

  [xfstests-debug] mount -t virtiofs xfstest /opt/xfstests/test
  [xfstests-debug] mount ret=0

  [xfstests-debug] df -T -P after mount:
  Filesystem           Type       1024-blocks    Used Available Capacity Mounted on
  /dev/vda             ext2         2097148     36016   2061132   2% /ext2

  [xfstests-debug] /proc/mounts after mount:
  ...
  xfstest /opt/xfstests/test virtiofs rw,relatime 0 0
  ...
  TEST_DEV=xfstest is mounted but not a type virtiofs filesystem

  结论很明确：probe_virtiofs 的 umount 不是原因。xfstests 自己确实执行了：

  mount -t virtiofs xfstest /opt/xfstests/test

  而且 mount 成功了，/proc/mounts 里也正确显示：

  xfstest /opt/xfstests/test virtiofs

  失败原因是 xfstests 的检查走的是：

  df -T -P | awk '$1 == "xfstest" { print $2 }'

  但在自动运行时，df -T -P 完全没有输出 xfstest 这一行，只输出了 /dev/vda ext2 /ext2。所以 _fs_type xfstest 得到空字符串，最后报：

  TEST_DEV=xfstest is mounted but not a type virtiofs filesystem
```