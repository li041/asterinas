##  Core Issue: Context Loss in the Inode level

The current implementation in Asterinas delegates specific file operations (like `fallocate`) down to the Inode layer (via InodeHandle).

The Problem: Many FUSE operations require a File Handle (`fh`), which is stored in the `file->private_data` in Linux.

The Conflict: By the time a request reaches the Inode trait's fallocate method, the connection to the specific FileIO instance is lost. The Inode represents the static file metadata, whereas the `fh` represents an active open session.

##  Information Gap: The fh Requirement Virtio-FS needs the fh for almost every virtiofs file operation.

Current Hack for virtio-fs: A redundant page_cache_fh is being maintained in the PageCacheBackend to bypass the interface limitation. And `seek` method is added to `FileIO` trait.

Incompatibility for other fs: Standard Linux behavior for ext4 involves checking f_flags (like O_SYNC) during fallocate. Our current Inode interface cannot access these per-file flags without "compensatory" arguments (e.g., adding XXXFlags), which is a suboptimal fix.

## Divergence from Linux VFS Semantics
Linux Approach: Operations like fallocate, seek, and read/write are methods of struct file_operations. They have direct access to struct file, allowing them to retrieve private_data. But it is actually due to the difference bewteen rust and c. 

## Limitation

We treat `sys_getdents` and `PathResolver` as operations on `InodeHandle`. This restricts the flexibility of adding a new `FileLike` instance for virtio-fs.