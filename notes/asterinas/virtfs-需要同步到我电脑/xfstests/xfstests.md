- 测试指令
```
  VNC_PORT=43 make run_kernel \
    AUTO_TEST=conformance \
    CONFORMANCE_TEST_SUITE=xfstests \
    XFSTESTS_BACKEND=virtiofs
```
- 默认使用`/opt/xfstests/virtiofs.list`, 可以指定完整测试`XFSTESTS_RUNLIST=/opt/xfstests/full.list`
• Asterinas 现在的 xfstests 支持是一条“conformance test suite”路径，目标是测 ext2 block filesystem。

  运行入口：

  `AUTO_TEST=conformance CONFORMANCE_TEST_SUITE=xfstests make run_kernel`
  
  当前流程：

  1. 顶层 Makefile:109 看到 `AUTO_TEST=conformance CONFORMANCE_TEST_SUITE=xfstests` 后，会打开 conformance test，并把这些参数塞进 kernel cmdline：
      - CONFORMANCE_TEST_SUITE=xfstests
      - XFSTESTS_RUNLIST
      - XFSTESTS_TEST_DEV
      - XFSTESTS_SCRATCH_DEV
  2. xfstests 默认参数在 Makefile:56：

     XFSTESTS_RUNLIST ?= /opt/xfstests/short.list
     XFSTESTS_DISK_SIZE ?= 12G
     XFSTESTS_TEST_DEV ?= /dev/vdc
     XFSTESTS_SCRATCH_DEV ?= /dev/vdd

  3. initramfs 构建阶段，如果 suite 是 xfstests，会额外生成两块 ext2 image。逻辑在 test/initramfs/Makefile:43 和 test/initramfs/Makefile:178：
      - xfstests_test.img
      - xfstests_scratch.img
      - 大小默认 12G
      - 都用 mkfs.ext2 初始化
  4. QEMU 参数脚本会在 suite 是 xfstests 时自动挂这两块 image，见 tools/qemu_args.sh:143 和 tools/qemu_args.sh:192。在普通 q35 模式下，它们会变成：
      - /dev/vdc：test dev
      - /dev/vdd：scratch dev
  5. xfstests 本体来自 Nix 的 pkgs.xfstests，打包逻辑在 test/initramfs/nix/conformance/xfstests.nix:26。它会把：
      - xfstests 文件复制到 /opt/xfstests
      - run_xfstests.sh
      - local.config
      - short.list / full.list / block.list
  6. VM 启动后，init 运行 /opt/run_conformance_test.sh。里面根据 CONFORMANCE_TEST_SUITE=xfstests 调用 test/initramfs/src/conformance/run_conformance_test.sh:29：

     /opt/xfstests/run_xfstests.sh -R "$XFSTESTS_RUNLIST"

  7. 当前 xfstests runner 在 test/initramfs/src/conformance/xfstests/run_xfstests.sh:13 里固定做两件事：
      - 检查 TEST_DEV / SCRATCH_DEV 是 block device
      - 用 mount -t ext2 挂到：
          - /opt/xfstests/test
          - /opt/xfstests/scratch
  8. xfstests 配置在 test/initramfs/src/conformance/xfstests/local.config:1，目前固定：

     export FSTYP=ext2
     export TEST_DIR=/opt/xfstests/test
     export SCRATCH_MNT=/opt/xfstests/scratch

  9. 最后 runner 会读取 runlist，把 block.list 加成 -E 排除列表，然后执行：

     ./check -E /opt/xfstests/block.list ...

  所以当前支持的核心是：自动构建 xfstests 到 initramfs，自动创建两块 ext2 测试盘，QEMU 自动挂盘，guest 内自动 mount 并跑指定 runlist。
  