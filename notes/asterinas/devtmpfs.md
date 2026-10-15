## Summary

This change aligns Asterinas devtmpfs node creation more closely with Linux.

It replaces the old path-only device-node interface with a `DevtmpfsNode`
description that carries the metadata required to accurately model Linux
defaults and per-subsystem overrides.

## Changes

* Introduce `DevtmpfsNode` for devtmpfs node metadata
* Switch device registration from `devtmpfs_path()` to `devtmpfs_node()`
* Make devtmpfs default to `root:root` with mode `0600`, matching Linux
* Allow subsystems to override devtmpfs node metadata when needed
* Preserve existing `/dev/ptmx` special handling instead of forcing it into the generic devtmpfs path
* Align `/dev/tty` mode with Linux (`0666`)
* Align `/dev/null`, `/dev/zero`, `/dev/full`, `/dev/random`, `/dev/urandom` to mode `0666`
* Update `/dev/full` regression test to check expected mode
* Add Linux source references to relevant device-node implementations

## Notes

Linux devtmpfs defaults device nodes to `root:root` with mode `0600`.
Subsystems may override metadata via `device_get_devnode()`. This PR models that behavior explicitly in the kernel device layer.
