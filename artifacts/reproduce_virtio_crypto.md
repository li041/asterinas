# Reproduce: Virtio-Crypto + /dev/crypto Cipher Path

This document reproduces the initial virtio-crypto enablement with cryptodev userspace tests.

## Goal

- Boot Asterinas with virtio-crypto enabled.
- Validate `/dev/crypto` basic cipher path.
- Confirm AES-CBC path works in initial integration scope.

## Host Preparation

1. Build userspace tests (host side source path):

```bash
cd /root/linux/cryptodev-linux/tests
make
```

2. Copy test binaries into shared directory used by guest:

```bash
cp ./cipher /tmp/9p_shared/
cp ./speed /tmp/9p_shared/
```

## Boot with Virtio-Crypto

```bash
cd /root/asterinas
ENABLE_9P=1 VIRTIO9P_SHARED_DIR=/tmp/9p_shared ENABLE_VIRTIO_CRYPTO=1 make run_kernel
```

## Guest Verification Steps

Inside guest shell:

1. Confirm device node exists:

```bash
ls -l /dev/crypto
```

2. Run cipher functional test:

```bash
mkdir -p /mnt/shared_dir
mount -t 9p -o trans=virtio my9p /mnt/shared_dir
/mnt/shared_dir/cipher
```

3. Run speed benchmark smoke:

```bash
/mnt/shared_dir/speed
```

## Expected Outcome (Current Scope)

- `/dev/crypto` can be opened.
- `CRIOGET`, `CIOCGSESSION`, `CIOCCRYPT`, `CIOCGSESSINFO`, `CIOCFSESSION` path works for initial cipher flow.
- AES-CBC path executes without QEMU virtio-crypto protocol errors.

## Current Supported Algorithm Scope

- `CRYPTO_AES_CBC` (virtio backend)
- `CRYPTO_NULL` (software passthrough in cryptodev compatibility layer)

## Troubleshooting

- If QEMU reports unsupported opcode or header size errors, verify protocol structs match Linux `virtio_crypto.h` layout.
- If `CIOCFSESSION` fails, check destroy-session response handling (`inhdr.status`, 1 byte) and queue routing.
- If `CRYPTO_NULL` fails in speed test, confirm cryptodev algorithm dispatch maps cipher id 16.
