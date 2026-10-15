 > Assisted by GPT-5.

  ## Clarify VFS namespace-operation check boundaries and avoid bypassing Dentry

  ### Problem

  The current VFS namespace operation checks are split across `Path`, `Dentry`, `Inode`, and filesystem-specific implementations without a clear contract. This causes two related problems:

  - Some generic VFS checks are duplicated in filesystem `Inode` implementations.
  - Some filesystem code, especially overlayfs, directly calls lower/upper inode namespace operations and bypasses the Dentry layer.

  This makes it hard to reason about which invariants are guaranteed before entering filesystem-specific code, and prevents safely removing duplicated checks.

  ### Current Layering

  `Path` already handles path/mount-level checks. For example, `Path::link` rejects cross-mount links, and `Path::rename` rejects cross-mount renames:

  - `Path::link` cross-mount check: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/vfs/path/mod.rs#L466-L471
  - `Path::rename` cross-mount check: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/vfs/path/mod.rs#L485-L490

  `Path` also funnels mutation operations into Dentry-level helpers:

  - `Path::new_fs_child` checks write permission and then calls `Dentry::create`: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/vfs/path/mod.rs#L60-L72
  - `Path::mknod`, `unlink`, `rmdir`, and `rename` go through `as_dir_dentry_or_err()` / `DirDentry`: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/vfs/path/mod.rs#L456-L490

  `Dentry` already owns several VFS namespace checks and dcache responsibilities:

  - `as_dir_dentry_or_err()` validates that the dentry represents a directory: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/vfs/path/dentry.rs#L230-L244
  - `DirDentry` is explicitly “validated to represent a directory”: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/vfs/path/dentry.rs#L247-L250
  - `DirDentry::create` checks dcache existence before calling `Inode::create`: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/vfs/path/dentry.rs#L263-L282
  - `DirDentry::mknod` checks dcache existence before calling `Inode::mknod`: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/vfs/path/dentry.rs#L307-L327
  - `DirDentry::link` checks dcache existence before calling `Inode::link`: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/vfs/path/dentry.rs#L330-L345
  - `DirDentry::unlink` rejects `.` / `..`, checks mountpoints, updates dcache, and emits notifications: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/vfs/path/dentry.rs#L351-L399
  - `DirDentry::rmdir` rejects `.` / `..`, checks mountpoints, updates dcache, and emits notifications: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/vfs/path/dentry.rs#L401-L438
  - `DirDentry::rename` validates both parents as directories, rejects `.` / `..`, checks mountpoints, and updates dcache:
  https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/vfs/path/dentry.rs#L453-L505
  - mountpoint checks are implemented in `DentryChildren`: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/vfs/path/dentry.rs#L644-L667

  Path lookup also performs generic lookup checks:

  - execute permission and `NAME_MAX` check in `lookup_at_path`: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/vfs/path/resolver.rs#L419-L424
  - unresolved parent result for non-existent tail component: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/vfs/path/resolver.rs#L553-L564
  - basename splitting rejects empty/root paths: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/vfs/path/resolver.rs#L812-L825

  However, the `Inode` trait still exposes name-based namespace mutation methods directly:

  - `Inode::{create,mknod,link,unlink,rmdir,lookup,rename}`: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/vfs/fs_apis/inode.rs#L297-L334

  This interface shape makes it unclear whether these methods are fully checked public FS operations or VFS-checked backend hooks.

  ### Duplicated Check Types

  #### Parent-is-directory checks

  The Dentry layer already validates directory parents through `as_dir_dentry_or_err()` and `DirDentry`:

  - Dentry validation: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/vfs/path/dentry.rs#L230-L250

  But filesystem inode implementations repeat parent-is-directory checks:

  - ramfs `create`: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/fs_impls/ramfs/fs.rs#L822-L828
  - ramfs `rename`: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/fs_impls/ramfs/fs.rs#L1014-L1019
  - ext2 `create`: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/fs_impls/ext2/inode.rs#L155-L166
  - ext2 `lookup`: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/fs_impls/ext2/inode.rs#L207-L213
  - ext2 `link`: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/fs_impls/ext2/inode.rs#L228-L234
  - exfat `create`: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/fs_impls/exfat/inode.rs#L1454-L1460
  - exfat `unlink`: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/fs_impls/exfat/inode.rs#L1549-L1552
  - exfat `rmdir`: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/fs_impls/exfat/inode.rs#L1580-L1583
  - exfat `rename`: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/fs_impls/exfat/inode.rs#L1649-L1653

  These are redundant for operations that entered through the standard `Path` / `Dentry` path.

  #### `.` / `..` checks

  The Dentry layer already rejects `.` / `..` for namespace mutations:

  - `unlink`: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/vfs/path/dentry.rs#L351-L355
  - `rmdir`: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/vfs/path/dentry.rs#L401-L408
  - `rename`: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/vfs/path/dentry.rs#L453-L465

  But filesystem inode implementations repeat these checks:

  - ramfs `unlink`: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/fs_impls/ramfs/fs.rs#L919-L922
  - ramfs `rmdir`: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/fs_impls/ramfs/fs.rs#L951-L957
  - ramfs `rename`: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/fs_impls/ramfs/fs.rs#L999-L1005
  - ext2 `unlink`: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/fs_impls/ext2/inode.rs#L260-L263
  - ext2 `rmdir`: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/fs_impls/ext2/inode.rs#L297-L303
  - ext2 `rename`: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/fs_impls/ext2/inode.rs#L453-L459
  - exfat `unlink`: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/fs_impls/exfat/inode.rs#L1549-L1558
  - exfat `rmdir`: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/fs_impls/exfat/inode.rs#L1580-L1591
  - exfat `rename`: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/fs_impls/exfat/inode.rs#L1639-L1644

  #### Generic name-length checks

  Path lookup already checks VFS-wide `NAME_MAX`:

  - `lookup_at_path` `NAME_MAX` check: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/vfs/path/resolver.rs#L419-L424

  Filesystem implementations repeat similar generic name-length checks:

  - ramfs `create`: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/fs_impls/ramfs/fs.rs#L822-L825
  - ext2 `create`: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/fs_impls/ext2/inode.rs#L155-L163
  - ext2 `lookup`: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/fs_impls/ext2/inode.rs#L207-L210
  - ext2 `link`: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/fs_impls/ext2/inode.rs#L228-L230
  - ext2 `rename`: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/fs_impls/ext2/inode.rs#L453-L459
  - exfat `create`: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/fs_impls/exfat/inode.rs#L1462-L1464
  - exfat `unlink`: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/fs_impls/exfat/inode.rs#L1553-L1555
  - exfat `rmdir`: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/fs_impls/exfat/inode.rs#L1590-L1591
  - exfat `rename`: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/fs_impls/exfat/inode.rs#L1643-L1644

  Some filesystem-specific name validation may still be needed, especially if the FS limit is stricter than VFS `NAME_MAX` or has encoding-specific constraints. But the interface should make clear
  which name-length checks are VFS guarantees and which are FS-specific constraints.

  #### Destination-existence checks

  The Dentry layer checks cached destination existence before create-like operations:

  - `DirDentry::create`: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/vfs/path/dentry.rs#L263-L282
  - `DirDentry::mknod`: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/vfs/path/dentry.rs#L307-L327
  - `DirDentry::link`: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/vfs/path/dentry.rs#L330-L345

  Filesystem implementations also check real directory entries:

  - ramfs `create`: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/fs_impls/ramfs/fs.rs#L830-L833
  - ramfs `link`: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/fs_impls/ramfs/fs.rs#L898-L902
  - exfat `create`: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/fs_impls/exfat/inode.rs#L1466-L1468

  The FS-side check is still necessary as race revalidation under the FS lock. The problem is that the current interface does not distinguish clearly between “VFS already checked the dentry cache”
  and “FS must still validate the real directory state”.

  #### Mountpoint checks

  The Dentry layer owns mountpoint-busy checks:

  - `check_mountpoint`: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/vfs/path/dentry.rs#L644-L653
  - `check_mountpoint_then_find`: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/vfs/path/dentry.rs#L655-L667

  Any code that bypasses Dentry-level helpers can miss this class of checks.

  ### Non-Standard Path: overlayfs Bypasses Dentry

  `overlayfs` directly calls namespace mutation methods on lower/upper `Arc<dyn Inode>` objects, bypassing the Dentry layer.

  Examples:

  - `upper.unlink(&whiteout_name(name))` before create: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/fs_impls/overlayfs/fs.rs#L267-L278
  - `upper.create(name, type_, mode)` after whiteout handling: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/fs_impls/overlayfs/fs.rs#L278-L279
  - `upper.unlink(name)` during overlay unlink: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/fs_impls/overlayfs/fs.rs#L364-L368
  - `upper.create(...)` to create a whiteout: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/fs_impls/overlayfs/fs.rs#L372-L379
  - `target_upper.unlink(whiteout)` while removing whiteout files: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/fs_impls/overlayfs/fs.rs#L404-L414
  - `upper.rmdir(name)` and `upper.create(...)` during overlay rmdir: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/fs_impls/overlayfs/fs.rs#L417-L419
  - `upper.mknod(name, mode, type_)`: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/fs_impls/overlayfs/fs.rs#L475-L480
  - `upper.link(old, name)`: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/fs_impls/overlayfs/fs.rs#L483-L488
  - recursive upper construction calls `parent_upper.create(...)`: https://github.com/asterinas/asterinas/blob/main/kernel/src/fs/fs_impls/overlayfs/fs.rs#L771-L788

  This is problematic because these calls bypass Dentry-level responsibilities such as:

  - dcache maintenance
  - mountpoint busy checks
  - centralized `.` / `..` handling
  - VFS-level target existence semantics
  - notifications

  This also forces lower filesystem `Inode` implementations to keep generic defensive checks, making it unsafe to remove the duplicated checks described above.

  For ramfs specifically, I did not find production code that directly calls `RamInode` methods by concrete type. However, the methods are still reachable through the public `Arc<dyn Inode>`
  interface. In particular, overlayfs directly calls upper/lower inode namespace methods; if the upper layer is ramfs, those calls dispatch to ramfs without going through the Dentry layer.
  Therefore these checks are redundant for the standard VFS Path/Dentry path, but cannot be blindly removed until the direct `Arc<dyn Inode>` mutation paths are addressed.

  ### Linux Comparison

  Linux generally routes namespace mutation through VFS helpers that operate on dentries/paths and then dispatch to filesystem inode operations:

  - `vfs_create()` performs `may_create()` and then calls `dir->i_op->create(...)`: https://codebrowser.dev/linux/linux/fs/namei.c.html#4121
  - `vfs_unlink()` performs `may_delete()`, mountpoint checks, and then calls `dir->i_op->unlink(...)`: https://codebrowser.dev/linux/linux/fs/namei.c.html#5352
  - `vfs_rmdir()` performs common checks and then calls `dir->i_op->rmdir(...)`: https://codebrowser.dev/linux/linux/fs/namei.c.html#5218
  - `vfs_link()` performs `may_create()` and cross-superblock checks before `dir->i_op->link(...)`: https://codebrowser.dev/linux/linux/fs/namei.c.html#5605
  - `vfs_rename()` centralizes rename checks before dispatching to the filesystem rename op: https://codebrowser.dev/linux/linux/fs/namei.c.html#5833

  The important distinction is that Linux filesystem operations are dentry-based backend hooks after VFS checks, while Asterinas currently exposes name-based `Inode` namespace methods directly.

  ### Suggested Direction

  1. Define and document the validation boundary:
     - `Path`: path resolution, mount/cross-mount semantics, syscall/path-level rules.
     - `Dentry`: parent directory validation, dcache, mountpoint checks, `.` / `..`, VFS namespace existence checks, notifications.
     - `Inode`: filesystem-specific backend mutation, on-disk/in-memory consistency, race revalidation under FS locks, type replacement rules, empty-directory checks, link count updates.

  2. Clarify the `Inode` namespace operation contract:
     - Either make these methods explicitly VFS-checked backend hooks.
     - Or keep them as fully checked public FS APIs, but then do not treat Dentry checks as sufficient.

  3. Add Dentry/Path-level helper APIs for internal users such as overlayfs so they do not need to call lower/upper inode namespace methods directly.

  4. Refactor overlayfs to use the standardized VFS/Dentry path for namespace mutations.

  5. After the boundary is clarified and bypasses are removed, remove truly redundant generic checks from filesystem-specific inode implementations.

  ### Expected Outcome

  The intended namespace mutation flow should be:

  ```text
  syscall
    -> PathResolver / Path
    -> Dentry-level VFS checks and cache handling
    -> filesystem-specific Inode backend operation

  Once this boundary is explicit and consistently followed, filesystem-specific inode implementations should only keep checks that are actually filesystem-specific or required for race revalidation
  under their own locks.