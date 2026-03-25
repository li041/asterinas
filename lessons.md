# Lessons for Developers (Virtio-9P + Virtio-Crypto)

## Most Valuable Takeaways

1. Implement a unified protocol path first, then add features.
   - Keep one consistent request lifecycle: build header/body, submit descriptors, wait completion, validate status.
   - Reuse the same queue semantics across subsystems (token mapping, wake-up model, timeout policy).
   - Treat on-wire layout as a contract: struct size, field order, and status width must be exact.

2. Use specification + Linux source as the dual reference.
   - Specification defines what must be true on wire.
   - Linux driver code shows operational details (scatter-gather order, control/data response shape, session lifecycle).
   - Runtime errors from QEMU are high-value protocol diagnostics and should map back to concrete struct or queue checks.

3. Separate responsibilities strictly.
   - Transport layer: wire format correctness, descriptor topology, IRQ completion path.
   - Client/session layer: request composition, session lifecycle, status-to-error mapping.
   - Integration layer: Linux-like interface behavior (mount path or ioctl surface).

## Todo list
1. Improve planning before coding.
   - Start each task with a short milestone plan (goal, scope, verification, rollback).
   - Keep the plan updated after each major code change.
2. Use skills intentionally instead of ad-hoc trial.
   - Identify which skill should be used for each task type before invoking tools.
3. Build a stable AI-assisted workflow.
4. Improve code ownership and control.