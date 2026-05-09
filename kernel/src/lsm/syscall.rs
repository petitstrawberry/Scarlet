//! Loadable Scarlet Module (LSM) system call interface.

use alloc::vec::Vec;

use crate::arch::Trapframe;
use crate::fs::MAX_PATH_LENGTH;
use crate::library::std::string::parse_c_string_from_userspace;
use crate::library::std::usercopy::copy_to_user;
use crate::lsm::{LsmError, LsmErrorCode, list_modules, load_module, unload_module};
use crate::object::capability::stream::StreamOps;
use crate::task::mytask;

fn map_lsm_error_to_code(err: &LsmError) -> LsmErrorCode {
    match err {
        LsmError::InvalidElf(_) => LsmErrorCode::InvalidElf,
        LsmError::NoMemory => LsmErrorCode::NoMemory,
        LsmError::Relocation(_) => LsmErrorCode::RelocationError,
        LsmError::UnresolvedSymbol(_) => LsmErrorCode::UnresolvedSymbol,
        LsmError::NoInitSymbol => LsmErrorCode::NoInit,
        LsmError::InitFailed(_) => LsmErrorCode::InitFailed,
        LsmError::BuildInfoMismatch => LsmErrorCode::BuildInfoMismatch,
        LsmError::MissingDependency(_) => LsmErrorCode::MissingDependency,
        LsmError::NotFound => LsmErrorCode::NotFound,
        LsmError::PermissionDenied => LsmErrorCode::PermissionDenied,
        LsmError::ArchMismatch => LsmErrorCode::ArchMismatch,
    }
}

/// Load a kernel module from filesystem path.
///
/// # Arguments
///
/// * `trapframe.get_arg(0)` - Pointer to a null-terminated UTF-8 path string in userspace
///
/// # Returns
///
/// * `0` on success
pub fn sys_lsm_load(trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => {
            // crate::println!("[LSM] No current task");
            return LsmErrorCode::PermissionDenied as usize;
        }
    };
    trapframe.increment_pc_next(task);

    let path_str = match parse_c_string_from_userspace(task, trapframe.get_arg(0), MAX_PATH_LENGTH)
    {
        Ok(s) => s,
        Err(_) => {
            // crate::println!("[LSM] Invalid path pointer or malformed string");
            return LsmErrorCode::InvalidPath as usize;
        }
    };

    let abs_path = if path_str.starts_with('/') {
        path_str
    } else {
        let vfs = match task.vfs.read().clone() {
            Some(vfs) => vfs,
            None => {
                // crate::println!("[LSM] No VFS for path resolution");
                return LsmErrorCode::PermissionDenied as usize;
            }
        };
        vfs.resolve_path_to_absolute(&path_str)
    };

    let vfs = match task.get_vfs() {
        Some(vfs) => vfs,
        None => {
            // crate::println!("[LSM] No VFS");
            return LsmErrorCode::PermissionDenied as usize;
        }
    };

    let file_obj = match vfs.open(&abs_path, 0) {
        Ok(obj) => obj,
        Err(e) => {
            // crate::println!("[LSM] Failed to open {}: {:?}", abs_path, e);
            return LsmErrorCode::InvalidPath as usize;
        }
    };

    let file = match file_obj.as_file() {
        Some(f) => f,
        None => {
            // crate::println!("[LSM] Not a file object");
            return LsmErrorCode::InvalidPath as usize;
        }
    };

    let mut data = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        match StreamOps::read(file, &mut buf) {
            Ok(0) => break,
            Ok(n) => data.extend_from_slice(&buf[..n]),
            Err(_) => {
                // crate::println!("[LSM] Read error");
                return LsmErrorCode::InvalidPath as usize;
            }
        }
    }

    match load_module(&data) {
        Ok(_module_id) => {
            // crate::println!("[LSM] Module id={} loaded successfully", module_id);
            LsmErrorCode::Success as usize
        }
        Err(e) => {
            if let LsmError::MissingDependency(dep) = &e {
                crate::println!("[LSM] missing dependency: {}", dep);
            }
            if let LsmError::UnresolvedSymbol(sym) = &e {
                crate::println!("[LSM] unresolved symbol: {}", sym);
            }
            map_lsm_error_to_code(&e) as usize
        }
    }
}

pub fn sys_lsm_unload(trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => {
            return LsmErrorCode::PermissionDenied as usize;
        }
    };
    trapframe.increment_pc_next(task);

    let module_id = trapframe.get_arg(0) as u64;
    match unload_module(module_id) {
        Ok(_) => LsmErrorCode::Success as usize,
        Err(e) => map_lsm_error_to_code(&e) as usize,
    }
}

pub fn sys_lsm_list(trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => {
            return 0;
        }
    };
    trapframe.increment_pc_next(task);

    let buffer_ptr = trapframe.get_arg(0);
    let buffer_size = trapframe.get_arg(1);
    let entries = list_modules();
    let entry_size = 264usize;
    if buffer_ptr == 0 || buffer_size < entry_size {
        return 0;
    }

    let max_entries = buffer_size / entry_size;
    let count = core::cmp::min(max_entries, entries.len());

    let mut payload = Vec::with_capacity(count * entry_size);
    for (module_id, module_name) in entries.into_iter().take(count) {
        payload.extend_from_slice(&module_id.to_le_bytes());
        let mut name_bytes = [0u8; 256];
        let src = module_name.as_bytes();
        let len = core::cmp::min(src.len(), name_bytes.len().saturating_sub(1));
        name_bytes[..len].copy_from_slice(&src[..len]);
        payload.extend_from_slice(&name_bytes);
    }

    if copy_to_user(task, buffer_ptr, &payload).is_err() {
        return 0;
    }

    count
}
