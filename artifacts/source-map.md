# Virtio-9P + Virtio-Crypto Source Map

This map links architecture responsibilities to concrete paths.

## Device and Protocol Layer

- `kernel/comps/virtio/src/device/transport9p/device/client.rs`
  - 9P request/response client ops (`Tversion`, `Tattach`, `Twalk`, `Tread`, `Twrite`, etc.).
- `kernel/comps/virtio/src/device/transport9p/protocol/`
  - 9P protocol constants, message structures, and decode/encode helpers.
- `kernel/comps/virtio/src/device/mod.rs`
  - transport module registration (`transport9p`).

## VFS Integration Layer

- `kernel/src/fs/virtio9p/fs.rs`
  - FS type registration (`name() == "9p"`), mount source(tag) parsing, root attach/getattr/statfs.
- `kernel/src/fs/virtio9p/fs/inode.rs`
  - inode-level behavior for metadata and file operations.
- `kernel/src/fs/virtio9p/fid.rs`
  - fid lifecycle management and allocation policy.
- `kernel/src/fs/mod.rs`
  - virtio9p initialization entry (`virtio9p::init()`).

## Runtime and Reproduction Configuration

- `tools/qemu_args.sh`
  - `ENABLE_9P`, `VIRTIO9P_TAG`, `VIRTIO9P_SHARED_DIR` and QEMU `virtio-9p` args.

## Validation Artifacts

- `qemu.log`
  - smoke pass marker: `Virtio 9p test passed.`

## Virtio-Crypto Device and Protocol Layer

- `kernel/comps/virtio/src/device/crypto/protocol.rs`
  - virtio-crypto on-wire headers, cipher op constants, status structs.
- `kernel/comps/virtio/src/device/crypto/config.rs`
  - `virtio_crypto_config` mapping and device capability fields.
- `kernel/comps/virtio/src/device/crypto/device/mod.rs`
  - queue lifecycle, request submission/wakeup, data/control path orchestration.
- `kernel/comps/virtio/src/device/crypto/device/client.rs`
  - session create/destroy and encrypt/decrypt request construction.
- `kernel/comps/virtio/src/device/crypto/device/virtio_ops.rs`
  - device init, callback registration, negotiated feature handling.

## Cryptodev Compatibility Layer

- `kernel/src/device/misc/crypto.rs`
  - `/dev/crypto` char device registration and cryptodev ioctl dispatch.
  - current session/algorithm abstraction (`CRIOGET`, `CIOCGSESSION`, `CIOCCRYPT`, `CIOCFSESSION`, `CIOCGSESSINFO`).
- `kernel/src/crypto/virtio.rs`
  - kernel-facing wrapper on top of virtio-crypto device API.

## Runtime Configuration for Crypto

- `tools/qemu_args.sh`
  - `ENABLE_VIRTIO_CRYPTO`, queue/device args for normal and microvm paths.
