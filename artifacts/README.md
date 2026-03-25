# Artifacts Index (Problem / Architecture / Reproduce / Lessons)

This directory contains reproducible artifacts for the virtio-9p + virtio-crypto bring-up work.

## 1. Problem

- Primary goals:
	- Initial virtio-9p support with shared-directory mount and basic file/directory operations.
	- Initial virtio-crypto support with `/dev/crypto` cipher path (AES-CBC baseline).
- Progress alignment:
	- 9p implementation strategy synchronized with virtio-fs layering and queue patterns.

## 2. Architecture

- Code path map: [source-map.md](source-map.md)
- Reused modules and methods across virtio-fs/9p and virtio-crypto:
	- [kernel/comps/virtio/src/device/filesystem/pool.rs](kernel/comps/virtio/src/device/filesystem/pool.rs) and [kernel/comps/virtio/src/device/crypto/pool.rs](kernel/comps/virtio/src/device/crypto/pool.rs)
		- Same DMA pool method: size-class pool + stream fallback, split ToDevice/FromDevice buffers, shared sync_to_device/sync_from_device pattern.
	- [kernel/comps/virtio/src/device/filesystem/device/virtio_ops.rs](kernel/comps/virtio/src/device/filesystem/device/virtio_ops.rs) and [kernel/comps/virtio/src/device/crypto/device/virtio_ops.rs](kernel/comps/virtio/src/device/crypto/device/virtio_ops.rs)
		- Same async interrupt-driver method: queue callback registration, IRQ callback only marks completion + wakes waiter, request path blocks by Waiter/Waker until completion.
	- [kernel/comps/virtio/src/queue.rs](kernel/comps/virtio/src/queue.rs)
		- Same descriptor lifecycle method in both tracks: add_dma_buf submission, token-based used-ring completion, should_notify/notify behavior.
	- [kernel/comps/virtio/src/id_alloc.rs](kernel/comps/virtio/src/id_alloc.rs)
		- Same id lifecycle method: SyncIdAlloc for request/session allocation and deallocation after completion.
	- [kernel/comps/virtio/src/device/mod.rs](kernel/comps/virtio/src/device/mod.rs) and [kernel/comps/virtio/src/lib.rs](kernel/comps/virtio/src/lib.rs)
		- Same integration method: module export + centralized virtio device dispatch.
	- [kernel/src/fs/virtio9p/fs.rs](kernel/src/fs/virtio9p/fs.rs), [kernel/src/fs/virtiofs/fs.rs](kernel/src/fs/virtiofs/fs.rs), and [kernel/src/device/misc/mod.rs](kernel/src/device/misc/mod.rs)
		- Same upper-layer integration method: keep transport implementation in virtio component layer, then adapt into Linux-like kernel interface (mount path or device node path).

- Non-identical parts in this work:
	- Added `/dev/crypto` ioctl compatibility bridge specific to cryptodev tests in [kernel/src/device/misc/crypto.rs](kernel/src/device/misc/crypto.rs).
	- Added virtio-crypto on-wire adaptation and queue routing fixes in [kernel/comps/virtio/src/device/crypto/protocol.rs](kernel/comps/virtio/src/device/crypto/protocol.rs) and [kernel/comps/virtio/src/device/crypto/device/mod.rs](kernel/comps/virtio/src/device/crypto/device/mod.rs).

## 3. Reproduce

- Shared directory mount + FS operations:
	- [reproduce_shared_dir_mount.md](reproduce_shared_dir_mount.md)
- Virtio-crypto + cryptodev test path:
	- [reproduce_virtio_crypto.md](reproduce_virtio_crypto.md)

## 4. Lessons and Experiment Context

- Interaction and tool history: [../experiment.md](../experiment.md)
- Practical lessons for developers: [../lessons.md](../lessons.md)
