## 一、核心机制

Asterinas **不使用单一的** `PG_locked` 标志，而是通过三个组件协同实现保护与等待：

- `CachePageMeta::lock`
    
- `PageState::Uninit / UpToDate`
    
- `WaitQueue`
    

---

## 二、冷页读取流程（首次提交 I/O）

1. **分配未初始化页**  
    `BackedVmo::commit_on()` 分配一个 `Uninit` 状态的 `cache page`。
    
2. **立即持有 page lock**  
    分配完成后立刻获得 page lock。
    
3. **插入 XArray**  
    将 page 插入 cache 索引结构。
    
4. **提交异步 I/O**  
    调用 `read_page_async()`，将 `LockedCachePage` **move** 进入 completion callback。
    
5. **I/O 完成**
    
    - 成功 → `set_up_to_date()`，然后 drop `LockedCachePage`
        
    - drop 时解锁并 `wake_all()` 等待者
        

---

## 三、并发读取行为

### 场景：第二个 reader 在 I/O 完成前访问同一页

1. 页面已在 cache 中，但状态为 **Uninit**
    
2. `try_commit_with_cursor()` 发现状态不对 → 返回 `VmoCommitError::WaitUntilInit`
    
3. 上层 `handle_commit_error()` 调用 `page.ensure_init()`
    
4. `ensure_init()` 尝试 **获取 page lock**
    
    - 第一个 reader 仍持有 lock → 拿不到
        
    - 当前线程 **睡眠在此 page 的 wait queue 上**
        

---

## 四、对外接口的保护

### 1. `PageCache::read()`

- 通过 `collect_pages()` 完成 commit
    
- 未完成的 `Uninit` 页面 **不会被用来拷贝数据给用户**
    

### 2. mmap page fault

- `try_commit_page()` 可能返回 `NeedIo` 或 `WaitUntilInit`
    
- fault handler 会：
    
    - drop 页表 cursor / preempt guard
        
    - 调用 **阻塞的** `commit_on(index)` 等待 I/O
        
    - 完成后重试映射
        

---

## 五、I/O 失败处理

- completion callback **不会**调用 `set_up_to_date()`
    
- page 解锁后 **仍为 Uninit**
    
- 等待者醒来后：
    
    - 重新尝试成为 initializer
        
    - 再次提交读
        
    - 失败 → 返回 `EIO`
        

---

## 六、测试覆盖

- `concurrent_page_faults`  
    验证：多个 waiter 共享一个 in-flight read，只提交一次 I/O
    
- `persistent_backend_errors`  
    验证：失败后不会无限等待，会重试并最终返回 `EIO`