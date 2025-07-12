//! VFS v2 System Call Interface
//!
//! This module implements system call handlers for VFS v2, providing the user-space
//! interface to filesystem operations. All system calls follow POSIX-like semantics
//! and work with the task's VFS namespace.
//!
//! ## Supported System Calls
//!
//! ### File Operations
//! - `sys_open()`: Open files and directories
//! - `sys_close()`: Close file descriptors
//! - `sys_read()`: Read data from files (legacy - prefer StreamRead 200)
//! - `sys_write()`: Write data to files (legacy - prefer StreamWrite 201)
//! - `sys_lseek()`: DEPRECATED - use FileSeek (300) for file seek operations
//! - `sys_truncate()`: Truncate files by path
//! - `sys_ftruncate()`: DEPRECATED - use FileTruncate (301) for file truncate operations
//!
//!
//! ### Directory Operations
//! - `sys_mkdir()`: Create directories
//! - `sys_mkfile()`: Create regular files
//!
//! ### Mount Operations
//! - `sys_mount()`: Mount filesystems
//! - `sys_umount()`: Unmount filesystems
//! - `sys_pivot_root()`: Change root filesystem
//!
//! ## VFS Namespace Isolation
//!
//! Each task can have its own VFS namespace (Option<Arc<VfsManager>>).
//! System calls operate within the task's namespace, enabling containerization
//! and process isolation.
//!
//! ## Error Handling
//!
//! System calls return usize::MAX (-1) on error and appropriate values on success.
//! 

use alloc::{string::String, vec::Vec, string::ToString, sync::Arc};

use crate::{arch::Trapframe, fs::FileType, library::std::string::cstring_to_string, task::mytask};

use crate::fs::{VfsManager, MAX_PATH_LENGTH};

/// Open a file or directory using VFS (VfsOpen)
/// 
/// This system call opens a file or directory at the specified path using the VFS layer.
/// 
/// # Arguments
/// 
/// * `trapframe.get_arg(0)` - Pointer to the null-terminated path string
/// * `trapframe.get_arg(1)` - Open flags (O_RDONLY, O_WRONLY, O_RDWR, etc.)
/// * `trapframe.get_arg(2)` - File mode for creation (if applicable)
/// 
/// # Returns
/// 
/// * Handle number on success
/// * `usize::MAX` on error (file not found, permission denied, etc.)
pub fn sys_vfs_open(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let path_ptr = task.vm_manager.translate_vaddr(trapframe.get_arg(0)).unwrap() as *const u8;
    let _flags = trapframe.get_arg(1) as i32;
    let _mode = trapframe.get_arg(2) as i32;

    // Increment PC to avoid infinite loop if open fails
    trapframe.increment_pc_next(task);

    // Parse path as a null-terminated C string
    let mut path_bytes = Vec::new();
    let mut i = 0;
    unsafe {
        loop {
            let byte = *path_ptr.add(i);
            if byte == 0 {
                break;
            }
            path_bytes.push(byte);
            i += 1;

            if i > MAX_PATH_LENGTH {
                return usize::MAX; // Path too long
            }
        }
    }

    // Convert path bytes to string
    let path_str = match str::from_utf8(&path_bytes) {
        Ok(s) => match to_absolute_path_v2(&task, s) {
            Ok(abs) => abs,
            Err(_) => return usize::MAX,
        },
        Err(_) => return usize::MAX, // Invalid UTF-8
    };

    // Try to open the file using VFS
    let vfs = match task.get_vfs() {
        Some(vfs) => vfs,
        None => return usize::MAX, // VFS not initialized
    };
    let file_obj = vfs.open(&path_str, 0);
    match file_obj {
        Ok(kernel_obj) => {
            // Use simplified handle role classification
            use crate::object::handle::{HandleMetadata, HandleType, AccessMode};
            
            // For now, all opened files are classified as Regular usage
            // Future enhancements could infer specific roles based on path patterns,
            // but keeping it simple with the 3-category system: IpcChannel, StandardInputOutput, Regular
            let handle_type = HandleType::Regular;
            
            // Infer access mode from flags (simplified - full implementation would parse all open flags)
            let access_mode = if _flags & 0x1 != 0 { // O_WRONLY-like
                AccessMode::WriteOnly
            } else if _flags & 0x2 != 0 { // O_RDWR-like
                AccessMode::ReadWrite
            } else {
                AccessMode::ReadOnly // Default
            };
            
            let metadata = HandleMetadata {
                handle_type,
                access_mode,
                special_semantics: None, // Could be inferred from flags like O_CLOEXEC
            };
            
            let handle = task.handle_table.insert_with_metadata(kernel_obj, metadata);
            match handle {
                Ok(handle) => handle as usize,
                Err(_) => usize::MAX, // Handle table full
            }
        }
        Err(_) => usize::MAX, // File open error
    }
}

/// Legacy close wrapper - redirects to HandleClose
#[deprecated(note = "Use sys_handle_close instead")]
pub fn sys_close(trapframe: &mut Trapframe) -> usize {
    crate::object::handle::syscall::sys_handle_close(trapframe)
}

/// Legacy dup wrapper - redirects to HandleDuplicate
#[deprecated(note = "Use sys_handle_duplicate instead")]
pub fn sys_dup(trapframe: &mut Trapframe) -> usize {
    crate::object::handle::syscall::sys_handle_duplicate(trapframe)
}

pub fn sys_read(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let fd = trapframe.get_arg(0) as u32; // Handle is u32
    let buf_ptr = task.vm_manager.translate_vaddr(trapframe.get_arg(1)).unwrap() as *mut u8;
    let count = trapframe.get_arg(2) as usize;

    // Increment PC to avoid infinite loop if read fails
    trapframe.increment_pc_next(task);

    let kernel_obj = task.handle_table.get(fd);
    if kernel_obj.is_none() {
        return usize::MAX; // Invalid file descriptor
    }

    let kernel_obj = kernel_obj.unwrap();
    let stream = kernel_obj.as_stream();
    if stream.is_none() {
        return usize::MAX; // Object doesn't support stream operations
    }

    let stream = stream.unwrap();
    let buffer = unsafe { core::slice::from_raw_parts_mut(buf_ptr, count) };
    
    match stream.read(buffer) {
        Ok(n) => {
            n
        }
        Err(_) => {
            return usize::MAX; // Read error
        }
    }
}

pub fn sys_write(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let fd = trapframe.get_arg(0) as u32; // Handle is u32
    let buf_ptr = task.vm_manager.translate_vaddr(trapframe.get_arg(1)).unwrap() as *const u8;
    let count = trapframe.get_arg(2) as usize;

    // Increment PC to avoid infinite loop if write fails
    trapframe.increment_pc_next(task);

    let kernel_obj = task.handle_table.get(fd);
    if kernel_obj.is_none() {
        return usize::MAX; // Invalid file descriptor
    }

    let kernel_obj = kernel_obj.unwrap();
    let stream = kernel_obj.as_stream();
    if stream.is_none() {
        return usize::MAX; // Object doesn't support stream operations
    }

    let stream = stream.unwrap();
    let buffer = unsafe { core::slice::from_raw_parts(buf_ptr, count) };
    
    match stream.write(buffer) {
        Ok(n) => {
            n
        }
        Err(_) => {
            return usize::MAX; // Write error
        }
    }
}

// sys_lseek is now deprecated - use FileSeek (300) syscall for file seek operations

pub fn sys_truncate(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let path_ptr = task.vm_manager.translate_vaddr(trapframe.get_arg(0)).unwrap() as *const u8;
    let length = trapframe.get_arg(1) as u64;
    
    trapframe.increment_pc_next(task);

    // Convert path bytes to string
    let path_str: String = match cstring_to_string(path_ptr, MAX_PATH_LENGTH) {
        Ok((s, _)) => match to_absolute_path_v2(&task, &s) {
            Ok(abs_path) => abs_path,
            Err(_) => return usize::MAX,
        },
        Err(_) => return usize::MAX, // Invalid UTF-8
    };
    
    let vfs = match task.vfs.as_ref() {
        Some(vfs) => vfs,
        None => return usize::MAX, // VFS not initialized
    };

    let file_obj = match vfs.open(&path_str, 0) {
        Ok(obj) => obj,
        Err(_) => return usize::MAX,
    };
    let file = match file_obj.as_file() {
        Some(f) => f,
        None => return usize::MAX,
    };
    match file.truncate(length) {
        Ok(_) => 0,
        Err(_) => usize::MAX, // -1
    }
}

// sys_ftruncate is now deprecated - use FileTruncate (301) syscall for file truncate operations

pub fn sys_mkfile(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let path_ptr = task.vm_manager.translate_vaddr(trapframe.get_arg(0)).unwrap() as *const u8;
    let _mode = trapframe.get_arg(1) as i32;

    trapframe.increment_pc_next(task);

    // Convert path bytes to string
    let path_str = match cstring_to_string(path_ptr, MAX_PATH_LENGTH) {
        Ok((s, _)) => match to_absolute_path_v2(&task, &s) {
            Ok(abs_path) => abs_path,
            Err(_) => return usize::MAX,
        },
        Err(_) => return usize::MAX, // Invalid UTF-8
    };
    
    let vfs = match task.vfs.as_ref() {
        Some(vfs) => vfs,
        None => return usize::MAX, // VFS not initialized
    };

    match vfs.create_file(&path_str, FileType::RegularFile) {
        Ok(_) => 0,
        Err(_) => usize::MAX, // -1
    }
}

pub fn sys_mkdir(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let path_ptr = task.vm_manager.translate_vaddr(trapframe.get_arg(0)).unwrap() as *const u8;
    
    trapframe.increment_pc_next(task);

    // Convert path bytes to string
    let path_str = match cstring_to_string(path_ptr, MAX_PATH_LENGTH) {
        Ok((s, _)) => match to_absolute_path_v2(&task, &s) {
            Ok(abs_path) => abs_path,
            Err(_) => return usize::MAX,
        },
        Err(_) => return usize::MAX, // Invalid UTF-8
    };
    
    let vfs = match task.vfs.as_ref() {
        Some(vfs) => vfs,
        None => return usize::MAX, // VFS not initialized
    };
    
    match vfs.create_dir(&path_str) {
        Ok(_) => 0,
        Err(_) => usize::MAX, // -1
    }
}

pub fn sys_mount(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let source_ptr = task.vm_manager.translate_vaddr(trapframe.get_arg(0)).unwrap() as *const u8;
    let target_ptr = task.vm_manager.translate_vaddr(trapframe.get_arg(1)).unwrap() as *const u8;
    let fstype_ptr = task.vm_manager.translate_vaddr(trapframe.get_arg(2)).unwrap() as *const u8;
    let flags = trapframe.get_arg(3) as u32;
    let data_ptr = if trapframe.get_arg(4) == 0 {
        core::ptr::null()
    } else {
        task.vm_manager.translate_vaddr(trapframe.get_arg(4)).unwrap() as *const u8
    };

    trapframe.increment_pc_next(task);

    // Convert paths and parameters to strings
    let source_str = match cstring_to_string(source_ptr, MAX_PATH_LENGTH) {
        Ok((s, _)) => s,
        Err(_) => return usize::MAX,
    };
    
    let target_str = match cstring_to_string(target_ptr, MAX_PATH_LENGTH) {
        Ok((s, _)) => s,
        Err(_) => return usize::MAX,
    };
    
    let fstype_str = match cstring_to_string(fstype_ptr, MAX_PATH_LENGTH) {
        Ok((s, _)) => s,
        Err(_) => return usize::MAX,
    };
    
    let data_str = if !data_ptr.is_null() {
        match cstring_to_string(data_ptr, MAX_PATH_LENGTH) {
            Ok((s, _)) => Some(s),
            Err(_) => return usize::MAX,
        }
    } else {
        None
    };

    // Get VFS reference
    let vfs = match task.vfs.as_ref() {
        Some(vfs) => vfs,
        None => return usize::MAX,
    };

    // Handle different mount types
    match fstype_str.as_str() {
        "bind" => {
            // Handle bind mount - this is a special case handled by VFS
            let _read_only = (flags & 1) != 0; // MS_RDONLY
            match vfs.bind_mount(&source_str, &target_str) {
                Ok(_) => 0,
                Err(_) => usize::MAX,
            }
        },
        _ => {
            // Handle filesystem creation using drivers
            let options = data_str.unwrap_or_default();
            match create_filesystem_and_mount(vfs, &fstype_str, &target_str, &options) {
                Ok(_) => 0,
                Err(_) => usize::MAX,
            }
        }
    }
}

// Helper function to parse overlay mount options
#[allow(dead_code)]
fn parse_overlay_options(data: &str) -> Result<(Option<String>, Vec<String>), ()> {
    let mut upperdir = None;
    let mut lowerdirs = Vec::new();
    
    for option in data.split(',') {
        if let Some(value) = option.strip_prefix("upperdir=") {
            upperdir = Some(value.to_string());
        } else if let Some(value) = option.strip_prefix("lowerdir=") {
            // Multiple lowerdirs can be separated by ':'
            for lowerdir in value.split(':') {
                lowerdirs.push(lowerdir.to_string());
            }
        }
    }
    
    if lowerdirs.is_empty() {
        return Err(()); // At least one lowerdir is required
    }
    
    Ok((upperdir, lowerdirs))
}

/// Create a filesystem using the driver and mount it
/// 
/// This function uses the new driver-based approach where option parsing
/// is delegated to the filesystem driver, and registration is handled
/// by sys_mount.
fn create_filesystem_and_mount(
    vfs: &crate::fs::VfsManager,
    fstype: &str,
    target: &str,
    options: &str,
) -> Result<(), crate::fs::FileSystemError> {
    use crate::fs::get_fs_driver_manager;
    let driver_manager = get_fs_driver_manager();
    // v2: directly create FS as Arc and mount it
    let filesystem = driver_manager.create_from_option_string(fstype, options)?;
    vfs.mount(filesystem, target, 0)?;
    Ok(())
}

pub fn sys_umount(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let target_ptr = task.vm_manager.translate_vaddr(trapframe.get_arg(0)).unwrap() as *const u8;
    let _flags = trapframe.get_arg(1) as u32; // Reserved for future use

    trapframe.increment_pc_next(task);

    // Convert target path to string
    let target_str: String = match cstring_to_string(target_ptr, MAX_PATH_LENGTH) {
        Ok((s, _)) => match to_absolute_path_v2(&task, &s) {
            Ok(abs_path) => abs_path,
            Err(_) => return usize::MAX,
        },
        Err(_) => return usize::MAX, // Invalid UTF-8
    };

    // Get VFS reference
    let vfs = match task.vfs.as_ref() {
        Some(vfs) => vfs,
        None => return usize::MAX,
    };

    // Perform umount operation
    match vfs.unmount(&target_str) {
        Ok(_) => 0,
        Err(_) => usize::MAX,
    }
}

pub fn sys_pivot_root(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let new_root_ptr = task.vm_manager.translate_vaddr(trapframe.get_arg(0)).unwrap() as *const u8;
    let old_root_ptr = task.vm_manager.translate_vaddr(trapframe.get_arg(1)).unwrap() as *const u8;

    trapframe.increment_pc_next(&task);

    // Convert new_root path to string
    let new_root_str: String = match cstring_to_string(new_root_ptr, MAX_PATH_LENGTH) {
        Ok((s, _)) => match to_absolute_path_v2(&task, &s) {
            Ok(abs_path) => abs_path,
            Err(_) => return usize::MAX,
        },
        Err(_) => return usize::MAX, // Invalid UTF-8
    };

    // Convert old_root path to string
    let old_root_str: String = match cstring_to_string(old_root_ptr, MAX_PATH_LENGTH) {
        Ok((s, _)) => match to_absolute_path_v2(&task, &s) {
            Ok(abs_path) => abs_path,
            Err(_) => return usize::MAX,
        },
        Err(_) => return usize::MAX, // Invalid UTF-8
    };

    // Get current VFS reference - pivot_root requires isolated VFS namespace
    let current_vfs = match task.vfs.as_ref() {
        Some(vfs) => vfs.clone(),
        None => {
            // pivot_root requires a task-specific VFS namespace
            // Tasks without VFS should use the global namespace, but pivot_root
            // is a namespace operation that doesn't make sense in that context
            return usize::MAX;
        },
    };

    // Perform pivot_root by replacing the mount_tree inside the existing VfsManager
    match pivot_root_in_place(&current_vfs, &new_root_str, &old_root_str) {
        Ok(_) => 0,
        Err(_) => usize::MAX,
    }
}

/// Pivot root by replacing the mount tree inside the existing VfsManager
/// 
/// This function implements pivot_root without creating a new VfsManager instance.
/// Instead, it manipulates the mount_tree directly to achieve the same effect.
/// This approach preserves the relationship between the init process and the global VFS.
fn pivot_root_in_place(
    vfs: &Arc<VfsManager>, 
    new_root_path: &str, 
    old_root_path: &str
) -> Result<(), crate::fs::FileSystemError> {
    // Use bind mount to mount the new root as "/" in the new mount tree
    let temp_vfs = VfsManager::new();
    temp_vfs.bind_mount_from(&vfs, new_root_path, "/")?;
    let old_root_path = if old_root_path == new_root_path {
        return Err(crate::fs::FileSystemError {
            kind: crate::fs::FileSystemErrorKind::InvalidPath,
            message: "Old root path cannot be the same as new root path".to_string(),
        });
    } else if old_root_path.starts_with(new_root_path) {
        &old_root_path[new_root_path.len()..]
    } else {
        old_root_path
    };

    let temp_root_entry = vfs.mount_tree.resolve_path(new_root_path)?.0;
    let temp_root = temp_root_entry.node();
    let fs = match temp_root.filesystem() {
        Some(fs) => {
            match fs.upgrade() {
                Some(fs) => fs,
                None => return Err(crate::fs::FileSystemError {
                    kind: crate::fs::FileSystemErrorKind::InvalidPath,
                    message: "New root path does not have a valid filesystem".to_string(),
                }),
            }
        }
        None => return Err(crate::fs::FileSystemError {
            kind: crate::fs::FileSystemErrorKind::InvalidPath,
            message: "New root path does not have a filesystem".to_string(),
        }),
    };
    // Mount the new root filesystem at "/"
    match temp_vfs.mount(fs, "/", 0) {
        Ok(_) => {},
        Err(e) => {
            crate::println!("Failed to mount new root filesystem: {}", e.message);
            return Err(e);
        }
    }

    temp_vfs.create_dir(old_root_path)?;

    match temp_vfs.bind_mount_from(&vfs, "/", old_root_path) {
        Ok(_) => {},
        Err(e) => {
            crate::println!("Failed to bind mount old root path: {}", e.message);
            return Err(e);
        }
    }

    {
        let mut original_guard = temp_vfs.mount_tree.root_mount.write();
        let mut temp_guard = vfs.mount_tree.root_mount.write();
        core::mem::swap(&mut *original_guard, &mut *temp_guard);
    }

    {
        let mut vfs_fs = vfs.mounted_filesystems.write();
        let temp_fs = temp_vfs.mounted_filesystems.read();
        *vfs_fs = temp_fs.clone();
    }

    Ok(())
}

pub fn sys_chdir(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let path_ptr = task.vm_manager.translate_vaddr(trapframe.get_arg(0)).unwrap() as *const u8;
    
    // Increment PC to avoid infinite loop if chdir fails
    trapframe.increment_pc_next(task);
    
    // Convert path pointer to string
    let path = match cstring_to_string(path_ptr, MAX_PATH_LENGTH) {
        Ok(p) => p.0,
        Err(_) => return usize::MAX,
    };
    
    // Get the VFS manager (either task-specific or global)
    let vfs = match task.get_vfs() {
        Some(vfs) => vfs,
        None => return usize::MAX,
    };

    // Resolve absolute path
    let absolute_path = match to_absolute_path_v2(&task, &path) {
        Ok(path) => path,
        Err(_) => return usize::MAX,
    };
    
    // Check if the path exists and is a directory
    match vfs.resolve_path(&absolute_path) {
        Ok(entry) => {
            if entry.node().file_type().unwrap() == FileType::Directory {
                // Update the task's current working directory
                task.set_cwd(absolute_path);
                0 // Success
            } else {
                usize::MAX // Not a directory
            }
        }
        Err(_) => return usize::MAX, // Path resolution error
    }
}

/// Remove a file or directory (unified VfsRemove)
/// 
/// This system call provides a unified interface for removing both files and directories,
/// replacing the traditional separate `unlink` (for files) and `rmdir` (for directories)
/// operations with a single system call.
/// 
/// For directories, they must be empty to be removed successfully.
/// 
/// # Arguments
/// 
/// * `trapframe.get_arg(0)` - Pointer to the null-terminated path string
/// 
/// # Returns
/// 
/// * `0` on success
/// * `usize::MAX` on error (file/directory not found, permission denied, directory not empty, etc.)
pub fn sys_vfs_remove(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let path_ptr = task.vm_manager.translate_vaddr(trapframe.get_arg(0)).unwrap() as *const u8;

    // Increment PC to avoid infinite loop if remove fails
    trapframe.increment_pc_next(task);

    // Convert path pointer to Rust string
    let path = match cstring_to_string(path_ptr, MAX_PATH_LENGTH) {
        Ok((s, _)) => s,
        Err(_) => return usize::MAX,
    };

    // Resolve absolute path
    let absolute_path = match to_absolute_path_v2(&task, &path) {
        Ok(path) => path,
        Err(_) => return usize::MAX,
    };

    // Get VFS manager instance
    let vfs = match task.get_vfs() {
        Some(vfs) => vfs,
        None => return usize::MAX, // VFS not initialized
    };

    // Try to resolve the path to check if it exists
    match vfs.resolve_path(&absolute_path) {
        Ok(_) => {
            // Path exists, attempt to remove it using unified VFS remove method
            match vfs.remove(&absolute_path) {
                Ok(_) => 0,
                Err(_) => usize::MAX,
            }
        }
        Err(_) => usize::MAX, // Path not found
    }
}

/// Create a directory using VFS (VfsCreateDirectory)
/// 
/// This system call creates a new directory at the specified path using the VFS layer.
/// 
/// # Arguments
/// 
/// * `trapframe.get_arg(0)` - Pointer to the null-terminated path string
/// 
/// # Returns
/// 
/// * `0` on success
/// * `usize::MAX` on error (path already exists, permission denied, etc.)
pub fn sys_vfs_create_directory(trapframe: &mut Trapframe) -> usize {
    sys_mkdir(trapframe)
}

/// Change current working directory using VFS (VfsChangeDirectory)
/// 
/// This system call changes the current working directory of the calling task
/// to the specified path using the VFS layer.
/// 
/// # Arguments
/// 
/// * `trapframe.get_arg(0)` - Pointer to the null-terminated path string
/// 
/// # Returns
/// 
/// * `0` on success
/// * `usize::MAX` on error (path not found, not a directory, etc.)
pub fn sys_vfs_change_directory(trapframe: &mut Trapframe) -> usize {
    sys_chdir(trapframe)
}

/// Legacy open wrapper - redirects to VfsOpen
#[deprecated(note = "Use sys_vfs_open instead")]
pub fn sys_open(trapframe: &mut Trapframe) -> usize {
    sys_vfs_open(trapframe)
}

/// Mount a filesystem (FsMount)
/// 
/// This system call mounts a filesystem at the specified target path.
/// 
/// # Arguments
/// 
/// * `trapframe.get_arg(0)` - Pointer to source path (device/filesystem)
/// * `trapframe.get_arg(1)` - Pointer to target mount point path
/// * `trapframe.get_arg(2)` - Pointer to filesystem type string
/// * `trapframe.get_arg(3)` - Mount flags
/// * `trapframe.get_arg(4)` - Pointer to mount data/options
/// 
/// # Returns
/// 
/// * `0` on success
/// * `usize::MAX` on error (invalid path, filesystem not supported, etc.)
pub fn sys_fs_mount(trapframe: &mut Trapframe) -> usize {
    sys_mount(trapframe)
}

/// Unmount a filesystem (FsUmount)
/// 
/// This system call unmounts a filesystem at the specified path.
/// 
/// # Arguments
/// 
/// * `trapframe.get_arg(0)` - Pointer to target path to unmount
/// * `trapframe.get_arg(1)` - Unmount flags (reserved for future use)
/// 
/// # Returns
/// 
/// * `0` on success
/// * `usize::MAX` on error (path not found, filesystem busy, etc.)
pub fn sys_fs_umount(trapframe: &mut Trapframe) -> usize {
    sys_umount(trapframe)
}

/// Change root filesystem (FsPivotRoot)
/// 
/// This system call changes the root filesystem of the calling process.
/// 
/// # Arguments
/// 
/// * `trapframe.get_arg(0)` - Pointer to new root path
/// * `trapframe.get_arg(1)` - Pointer to old root mount point
/// 
/// # Returns
/// 
/// * `0` on success
/// * `usize::MAX` on error (invalid path, operation not permitted, etc.)
pub fn sys_fs_pivot_root(trapframe: &mut Trapframe) -> usize {
    sys_pivot_root(trapframe)
}

/// Legacy mount wrapper - redirects to FsMount
#[deprecated(note = "Use sys_fs_mount instead")]
pub fn sys_mount_legacy(trapframe: &mut Trapframe) -> usize {
    sys_mount(trapframe)
}

/// Legacy umount wrapper - redirects to FsUmount
#[deprecated(note = "Use sys_fs_umount instead")]
pub fn sys_umount_legacy(trapframe: &mut Trapframe) -> usize {
    sys_umount(trapframe)
}

/// Legacy pivot_root wrapper - redirects to FsPivotRoot
#[deprecated(note = "Use sys_fs_pivot_root instead")]
pub fn sys_pivot_root_legacy(trapframe: &mut Trapframe) -> usize {
    sys_pivot_root(trapframe)
}


// Use a local path normalization function
fn to_absolute_path_v2(task: &crate::task::Task, path: &str) -> Result<String, ()> {
    if path.starts_with('/') {
        Ok(path.to_string())
    } else {
        let cwd = task.cwd.clone().ok_or(())?;
        let mut absolute_path = cwd;
        if !absolute_path.ends_with('/') {
            absolute_path.push('/');
        }
        absolute_path.push_str(path);
        // Simple normalization (removes "//", ".", etc.)
        let mut components = Vec::new();
        for comp in absolute_path.split('/') {
            match comp {
                "" | "." => {},
                ".." => { components.pop(); },
                _ => components.push(comp),
            }
        }
        Ok("/".to_string() + &components.join("/"))
    }
}