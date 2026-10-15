## Root Cause

The original `BioSegmentPool` bug was caused by updating `min_free` only by searching **after the newly allocated range**:

```rust
manager.min_free = manager.occupied[end..]
    .iter()
    .position(|i| !i)
    .map(|pos| end + pos)
    .unwrap_or(self.total_blocks);
```

This loses any free holes that still exist **before** the allocated range.

### Example

Suppose the bitmap is:

```text
index:     0 1 2 3
occupied:  0 1 0 0
min_free = 0

2 blocks


```

Now allocate **2 contiguous blocks**:

1. `[0, 2)` is not available because block `1` is occupied.
2. The allocator eventually allocates `[2, 4)`.
3. Block `0` is still free.
4. However, the old implementation starts searching from `end = 4`, so `min_free` becomes `capacity`.
5. Subsequent allocations incorrectly believe the pool is full, and block `0` is never considered again.

As a result, this is **not** an overlap or use-after-free bug. Instead, previously free space becomes permanently unreachable, effectively leaking allocator capacity. Once enough space is "lost" in this way, BIO allocation falls back to allocating standalone `DmaStream`s much earlier than expected.

## Fix

The extracted `DmaArenaAllocator` updates `min_free` by resuming the search from the **previous** `min_free` instead of the end of the newly allocated range:

```rust
manager.min_free = manager.occupied[previous_min_free..]
    .iter()
    .position(|occupied| !*occupied)
    .map(|position| previous_min_free + position)
    .unwrap_or(capacity_pages);
```

(See `kernel/libs/aster-util/src/dma_arena.rs:79`.)

With this change, in the example above `min_free` remains `0`, so the allocator will correctly revisit the free block at index `0` during future allocations, preventing capacity leakage.