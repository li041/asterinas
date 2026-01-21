// SPDX-License-Identifier: MPL-2.0

use aster_util::slot_vec::SlotVec;
use ostd::sync::RwMutexUpgradeableGuard;

use super::template::{
    DirOps, ProcDir, ProcDirBuilder, lookup_child_from_table, populate_children_from_table,
};
use crate::{
    events::IoEvents,
    fs::{
        inode_handle::FileIo,
        procfs::pid::task::{TaskDirOps, TidDirOps},
        utils::{Inode, InodeIo, StatusFlags, mkmod},
    },
    prelude::*,
    process::{
        Process,
        signal::{PollHandle, Pollable},
    },
};

mod task;

/// Represents the inode at `/proc/[pid]`.
#[derive(Clone)]
pub struct PidDirOps(
    // The `/proc/<pid>` directory is a superset of the `/proc/<pid>/task/<tid>` directory.
    // So we embed `TidDirOps` here so that `PidDirOps` can "inherit" entries and methods
    // from `TidDirOps`.
    TidDirOps,
);

impl PidDirOps {
    pub fn new_inode(process_ref: Arc<Process>, parent: Weak<dyn Inode>) -> Arc<dyn Inode> {
        let tid_dir_ops = TidDirOps {
            process_ref,
            thread_ref: None,
        };
        // Reference: <https://elixir.bootlin.com/linux/v6.16.5/source/fs/proc/base.c#L3493>
        ProcDirBuilder::new(Self(tid_dir_ops.clone()), mkmod!(a+rx))
            .parent(parent)
            // The PID directories must be volatile, because it is just associated with one process.
            .volatile()
            .build()
            .unwrap()
    }

    pub fn process(&self) -> Arc<Process> {
        self.0.process_ref.clone()
    }

    #[expect(clippy::type_complexity)]
    const STATIC_ENTRIES: &'static [(
        &'static str,
        fn(&PidDirOps, Weak<dyn Inode>) -> Arc<dyn Inode>,
    )] = &[("task", TaskDirOps::new_inode)];
}

impl DirOps for PidDirOps {
    fn lookup_child(&self, dir: &ProcDir<Self>, name: &str) -> Result<Arc<dyn Inode>> {
        let mut cached_children = dir.cached_children().write();

        // Look up entries that either exist under `/proc/<pid>`
        // but not under `/proc/<pid>/task/<tid>`,
        // or entries whose contents differ between `/proc/<pid>` and `/proc/<pid>/task/<tid>`.
        if let Some(child) =
            lookup_child_from_table(name, &mut cached_children, Self::STATIC_ENTRIES, |f| {
                (f)(self, dir.this_weak().clone())
            })
        {
            return Ok(child);
        }

        // For all other children, the content is the same under both `/proc/<pid>` and `/proc/<pid>/task/<tid>`.
        self.0
            .lookup_child_locked(&mut cached_children, dir.this_weak().clone(), name)
    }

    fn populate_children<'a>(
        &self,
        dir: &'a ProcDir<Self>,
    ) -> RwMutexUpgradeableGuard<'a, SlotVec<(String, Arc<dyn Inode>)>> {
        let mut cached_children = dir.cached_children().write();

        // Populate entries that either exist under `/proc/<pid>`
        // but not under `/proc/<pid>/task/<tid>`,
        // or whose contents differ between the two paths.
        populate_children_from_table(&mut cached_children, Self::STATIC_ENTRIES, |f| {
            (f)(self, dir.this_weak().clone())
        });

        // Populate the remaining children that are identical
        // under both `/proc/<pid>` and `/proc/<pid>/task/<tid>`.
        self.0
            .populate_children_locked(&mut cached_children, dir.this_weak().clone());

        cached_children.downgrade()
    }

    fn open(
        &self,
        _access_mode: crate::fs::utils::AccessMode,
        _status_flags: crate::fs::utils::StatusFlags,
    ) -> Option<Result<Box<dyn crate::fs::inode_handle::FileIo>>> {
        Some(Ok(Box::new(self.clone())))
    }
}

impl Pollable for PidDirOps {
    fn poll(&self, _mask: IoEvents, _poller: Option<&mut PollHandle>) -> IoEvents {
        IoEvents::empty()
    }
}

impl InodeIo for PidDirOps {
    fn read_at(
        &self,
        _offset: usize,
        _writer: &mut VmWriter,
        _status_flags: StatusFlags,
    ) -> Result<usize> {
        Err(Error::new(Errno::EISDIR))
    }

    fn write_at(
        &self,
        _offset: usize,
        _reader: &mut VmReader,
        _status_flags: StatusFlags,
    ) -> Result<usize> {
        Err(Error::new(Errno::EISDIR))
    }
}

impl FileIo for PidDirOps {
    fn check_seekable(&self) -> Result<()> {
        Ok(())
    }

    fn is_offset_aware(&self) -> bool {
        false
    }
}
