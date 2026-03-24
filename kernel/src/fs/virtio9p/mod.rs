// SPDX-License-Identifier: MPL-2.0

mod fid;
mod fs;

pub(super) fn init() {
    super::registry::register(&fs::Virtio9PFsType).unwrap();
}
