# aster-fuse

`aster-fuse` provides shared FUSE protocol definitions for Asterinas kernel
crates.

It contains POD-compatible request and reply layouts, typed opcodes and flags,
protocol constants, and the `FuseOperation` trait used by FUSE clients such as
virtio-fs.

## Scope

`aster-fuse` focuses on protocol representation:

- It models FUSE headers, payload structs, enums, and bitflags.
- It keeps those types `Pod` where the wire layout requires direct byte I/O.
- It leaves transport-specific request submission and buffer management to
  higher-level crates.

## Main Entry Points

- `src/lib.rs` defines protocol structs, enums, flags, and constants.
- `src/operation.rs` defines `FuseOperation`, which describes one typed FUSE
  request and reply pair.
- `src/error.rs` defines `FuseError` and `FuseResult`.
