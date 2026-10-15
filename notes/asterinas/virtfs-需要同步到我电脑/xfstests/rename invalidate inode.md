- rename会改变源父目录`self`, 目标父目录`target`, 被移动的inode`source`和被覆盖的inode`overwritten`
	- 但是目前接口只传了, 两个父目录和对应名称, 没有传入对应vfs解析好的dentry
		- 所以目前需要`source`和`overwritten`的发`FUSE_LOOKUP`查询
```rust
  fn rename(
      &self,
      old_name: &str,
      target: &Arc<dyn Inode>,
      new_name: &str,
      mode: RenameMode,
  )
```
- linux行为
	- 路径解析阶段使用 `dcache`；进入 filesystem 的 `rename` 回调时，直接传已经解析好的 `old_dentry` 和 `new_dentry`，不会让 filesystem 再按名字`lookup` 一次。[linux vfs_rename](https://codebrowser.dev/linux/linux/fs/namei.c.html#5808)
- 我看`old_dentry: Arc<Dentry>`和`new_dentry: Option<Arc<Dentry>>`在`DirDetnry::rename`做检查的时候已经解析了, 不过没有传递
	- #问题`Dentry`结构体不能表达`negative dentry`, 接口没法改为传两个old_dentry
	- 如果不加`inode: Option<Arc<dyn Inode>>`的话, 最小接口
```rust
fn rename(
  &self,
  old_dentry: &Arc<Dentry>,
  target: &Arc<dyn Inode>,
  new_dentry: Option<&Arc<Dentry>>,
  new_name: &str,
  mode: RenameMode,
) -> Result<()>;
```
- 副作用: 对Dentry的可见性有影响
