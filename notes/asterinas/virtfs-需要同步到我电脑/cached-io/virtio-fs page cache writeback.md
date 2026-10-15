你的担心是合理的，但需要区分两类状态：

  1. PageState：Dirty、UpToDate、Uninit、Evicted。
  2. is_writing_back：表示 I/O 是否在飞行中的独立标志。

  当前设计中，真正的 PageState 转换应该始终通过 LockedCachePage 完成：

  LockedCachePage::set_dirty()
  LockedCachePage::set_up_to_date()
  LockedCachePage::invalidate()
  LockedCachePage::set_evicted()

  完成回调也应该先获取页锁，再执行 invalidate()。这和原来的状态转换协议是一致的。

  原协议本来就允许 is_writing_back 在完成回调中无锁清除：

  clear_writing_back()

  因为它不是 PageState，而是类似 Linux PG_writeback 的独立同步位。等待这个位清除也不应该持有
  页锁，否则会出现：

  新写回持有页锁，等待旧写回
  旧写回完成回调等待页锁

  所以真正需要确定的是下面这组协议，而不是简单要求所有操作都持有页锁：

  - PageState 的改变必须持有页锁；
  - is_writing_back 的设置必须持有页锁；
  - is_writing_back 的清除可以由完成回调无锁完成；
  - 等待 is_writing_back 时不能持有页锁；
  - writeback 期间的新写入可以获取页锁并把页重新标记为 dirty；
  - 完成回调获取页锁后，只能使仍然是当前写回版本的 UpToDate 页失效，不能覆盖已经重新变 dirty
    的新版本。

  因此，新增一个“无锁等待 writeback”的内部辅助方法，本身不改变页状态协议；但如果这个方法让调
  用者在无锁情况下直接修改 PageState，那确实就是破坏协议。

  我同意不应该再引入类似原来 mark_dirty() 那种无锁状态修改。现在需要进一步确认的是：是否接受
  一个仅用于等待 is_writing_back 的内部 helper；如果不接受，就需要重新设计等待路径，但仍然必
  须避免“持页锁等待完成回调”的死锁。