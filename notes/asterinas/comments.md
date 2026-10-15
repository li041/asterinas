- move dmapool的comment
```
Yes, this is feasible. However, moving DmaPool to aster-util is a broader refactor: it affects the module ownership, network and virtio imports,
  Cargo dependencies, and the current BottomHalfDisabled lock-guardian handling.

  To keep this PR focused, I suggest leaving DmaPool where it is for now and addressing the relocation in a follow-up PR.
```
- 