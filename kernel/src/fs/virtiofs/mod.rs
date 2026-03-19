// SPDX-License-Identifier: MPL-2.0

use alloc::string::{String, ToString};

use ostd::boot::boot_info;

mod fs;

pub(super) fn init() {
    super::registry::register(&fs::VirtioFsType).unwrap();
}

/// Parses the kernel command line to check if rootfs=virtiofs is specified.
///
/// Returns Some(virtiofs_tag) if virtiofs mode is enabled, None otherwise.
pub fn get_virtiofs_tag_from_cmdline() -> Option<String> {
    let cmdline = boot_info().kernel_cmdline.as_str();

    for arg in cmdline.split_whitespace() {
        if let Some(value) = arg.strip_prefix("rootfs=") {
            if value == "virtiofs" {
                // Check for virtiofs_tag
                for tag_arg in cmdline.split_whitespace() {
                    if let Some(tag) = tag_arg.strip_prefix("virtiofs_tag=") {
                        return Some(tag.to_string());
                    }
                }
                // Default tag if not specified
                return Some("share_folder".to_string());
            }
        }
    }
    None
}
