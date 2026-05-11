//! Darwin path ↔ Scarlet VFS path translation
//!
//! The overlay VFS mounts `/scarlet/system/darwin-aarch64/` as the task's root,
//! so system paths (`/usr/lib`, `/System/Library`, etc.) resolve automatically.
//! Only Darwin-specific user path semantics need manual translation.

use alloc::format;
use alloc::string::{String, ToString};

pub fn translate_to_scarlet(darwin_path: &str) -> String {
    match darwin_path {
        p if p.starts_with("/Users/") => {
            let user = p.split('/').nth(2).unwrap_or("");
            let rest_start = 7 + user.len();
            let rest = if rest_start < p.len() {
                &p[rest_start..]
            } else {
                ""
            };
            format!("/home/{}{}", user, rest)
        }
        _ => darwin_path.to_string(),
    }
}

pub fn translate_to_darwin(scarlet_path: &str) -> String {
    if scarlet_path.starts_with("/home/") {
        let user = scarlet_path.split('/').nth(2).unwrap_or("");
        let rest_start = 6 + user.len();
        let rest = if rest_start < scarlet_path.len() {
            &scarlet_path[rest_start..]
        } else {
            ""
        };
        format!("/Users/{}{}", user, rest)
    } else {
        scarlet_path.to_string()
    }
}
