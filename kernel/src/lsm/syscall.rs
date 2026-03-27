//! Loadable Scarlet Module (LSM) system call interface.

use alloc::vec::Vec;

use crate::arch::Trapframe;
use crate::fs::MAX_PATH_LENGTH;
use crate::library::std::string::parse_c_string_from_userspace;
use crate::lsm::load_module;
use crate::object::capability::stream::StreamOps;
use crate::task::mytask;

/// Load a kernel module from filesystem path.
///
/// # Arguments
///
/// * `trapframe.get_arg(0)` - Pointer to a null-terminated UTF-8 path string in userspace
///
/// # Returns
///
/// * `0` on success
/// * `usize::MAX` on failure
pub fn sys_lsm_load(trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => {
            crate::println!("[LSM] No current task");
            return usize::MAX;
        }
    };
    trapframe.increment_pc_next(task);

    let path_str = match parse_c_string_from_userspace(task, trapframe.get_arg(0), MAX_PATH_LENGTH)
    {
        Ok(s) => s,
        Err(_) => {
            crate::println!("[LSM] Invalid path pointer or malformed string");
            return usize::MAX;
        }
    };

    let abs_path = if path_str.starts_with('/') {
        path_str
    } else {
        let vfs = match task.vfs.read().clone() {
            Some(vfs) => vfs,
            None => {
                crate::println!("[LSM] No VFS for path resolution");
                return usize::MAX;
            }
        };
        vfs.resolve_path_to_absolute(&path_str)
    };

    let vfs = match task.get_vfs() {
        Some(vfs) => vfs,
        None => {
            crate::println!("[LSM] No VFS");
            return usize::MAX;
        }
    };

    let file_obj = match vfs.open(&abs_path, 0) {
        Ok(obj) => obj,
        Err(e) => {
            crate::println!("[LSM] Failed to open {}: {:?}", abs_path, e);
            return usize::MAX;
        }
    };

    let file = match file_obj.as_file() {
        Some(f) => f,
        None => {
            crate::println!("[LSM] Not a file object");
            return usize::MAX;
        }
    };

    let mut data = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        match StreamOps::read(file, &mut buf) {
            Ok(0) => break,
            Ok(n) => data.extend_from_slice(&buf[..n]),
            Err(_) => {
                crate::println!("[LSM] Read error");
                return usize::MAX;
            }
        }
    }

    match load_module(&data) {
        Ok(handle) => {
            crate::println!("[LSM] Module '{}' loaded successfully", handle.name);
            0
        }
        Err(e) => {
            crate::println!("[LSM] Failed to load module: {:?}", e);
            usize::MAX
        }
    }
}
