# Reproduce: Shared Directory Mount and Basic File/Directory Operations

This document reproduces the shared-directory workflow aligned with virtio-9p bring-up progress and virtio-fs-style layering.

## Goal

- Mount shared directory inside guest with 9p.
- Verify basic file and directory operations.

## Host Preparation

1. Prepare a host-side shared directory:

```bash
mkdir -p /tmp/9p_shared
echo "hello-from-host" > /tmp/9p_shared/host_file.txt
```

2. Build and run kernel with shared directory enabled:

```bash
cd /root/asterinas
ENABLE_9P=1 VIRTIO9P_SHARED_DIR=/tmp/9p_shared make run_kernel
```

## Guest Verification Steps

Inside guest shell:

1. Create mount point and mount shared directory (tag should match runtime config):

```bash
mkdir -p /mnt/shared_dir
mount -t 9p -o trans=virtio my9p /mnt/shared_dir
```

2. Verify read path:

```bash
cat /mnt/shared_dir/host_file.txt
```

3. Verify write + directory operations:

```bash
echo "hello-from-guest" > /mnt/shared_dir/guest_file.txt
mkdir -p /mnt/shared_dir/guest_dir
ls -al /mnt/shared_dir
```

4. Verify round-trip on host:

```bash
cat /tmp/9p_shared/guest_file.txt
ls -al /tmp/9p_shared
```

## Expected Outcome

- Mount succeeds without kernel panic.
- `cat` and `ls` work on mounted path.
- New file and directory created in guest are visible on host.