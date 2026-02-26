// SPDX-License-Identifier: MPL-2.0

mod fs;

pub(super) fn init() {
    super::registry::register(&fs::VirtioFsType).unwrap();
}
