# aster-fuse

`aster-fuse` provides shared FUSE protocol definitions for Asterinas kernel
crates. It contains POD-compatible request/response structures, opcode enums,
flags, and protocol constants used by FUSE clients such as virtio-fs.

Keeping these definitions in a dedicated crate avoids tying protocol types to a
specific transport implementation and makes reuse across multiple modules
straightforward.
