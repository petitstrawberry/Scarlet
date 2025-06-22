use alloc::{string::String, vec::Vec, string::ToString};

use crate::{arch::Trapframe, library::std::string::cstring_to_string, task::mytask};

use super::{SeekFrom, VfsManager, MAX_PATH_LENGTH};

pub fn sys_open(trapframe: &mut Trapframe) -> usize {
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
        Ok(s) => VfsManager::to_absolute_path(&task, s).unwrap(),
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
            // file_obj is already a KernelObject::File
            let handle = task.handle_table.insert(kernel_obj);
            match handle {
                Ok(handle) => handle as usize,
                Err(_) => usize::MAX, // Handle table full
            }
        }
        Err(_) => usize::MAX, // File open error
    }
}

pub fn sys_close(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let fd = trapframe.get_arg(0) as u32; // Handle is u32
    trapframe.increment_pc_next(task);
    if task.handle_table.remove(fd).is_some() {
        0
    } else {
        usize::MAX // -1
    }
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

pub fn sys_lseek(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let fd = trapframe.get_arg(0) as u32; // Handle is u32
    let offset = trapframe.get_arg(1) as i64;
    let whence = trapframe.get_arg(2) as i32;

    // Increment PC to avoid infinite loop if lseek fails
    trapframe.increment_pc_next(task);

    let kernel_obj = task.handle_table.get(fd);
    if kernel_obj.is_none() {
        return usize::MAX; // Invalid file descriptor
    }

    let kernel_obj = kernel_obj.unwrap();
    let file = kernel_obj.as_file();
    if file.is_none() {
        return usize::MAX; // Object doesn't support file operations
    }

    let file = file.unwrap();
    let whence = match whence {
        0 => SeekFrom::Start(offset as u64),
        1 => SeekFrom::Current(offset),
        2 => SeekFrom::End(offset),
        _ => return usize::MAX, // Invalid whence
    };

    match file.seek(whence) {
        Ok(pos) => {
            pos as usize
        }
        Err(_) => {
            return usize::MAX; // Lseek error
        }
    }
}

pub fn sys_truncate(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let path_ptr = task.vm_manager.translate_vaddr(trapframe.get_arg(0)).unwrap() as *const u8;
    let length = trapframe.get_arg(1) as u64;
    
    trapframe.increment_pc_next(task);

    // Convert path bytes to string
    let path_str = match cstring_to_string(path_ptr, MAX_PATH_LENGTH) {
        Ok((s, _)) => match VfsManager::to_absolute_path(&task, &s) {
            Ok(abs_path) => abs_path,
            Err(_) => return usize::MAX,
        },
        Err(_) => return usize::MAX, // Invalid UTF-8
    };
    
    let vfs = match task.vfs.as_ref() {
        Some(vfs) => vfs,
        None => return usize::MAX, // VFS not initialized
    };
    
    match vfs.truncate(&path_str, length) {
        Ok(_) => 0,
        Err(_) => usize::MAX, // -1
    }
}

pub fn sys_ftruncate(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let fd = trapframe.get_arg(0) as u32;
    let length = trapframe.get_arg(1) as u64;
    
    trapframe.increment_pc_next(task);
    
    let kernel_obj = task.handle_table.get(fd);
    if kernel_obj.is_none() {
        return usize::MAX; // Invalid file descriptor
    }
    
    let kernel_obj = kernel_obj.unwrap();
    let file = kernel_obj.as_file();
    if file.is_none() {
        return usize::MAX; // Object doesn't support file operations
    }
    
    let file = file.unwrap();
    match file.truncate(length) {
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
            let read_only = (flags & 1) != 0; // MS_RDONLY
            match vfs.bind_mount(&source_str, &target_str, read_only) {
                Ok(_) => 0,
                Err(_) => usize::MAX,
            }
        },
        "overlay" => {
            // Handle overlay mount - this is a special case handled by VFS
            if let Some(data) = data_str {
                match parse_overlay_options(&data) {
                    Ok((upperdir, lowerdirs)) => {
                        let lowerdir_refs: Vec<&str> = lowerdirs.iter().map(|s| s.as_str()).collect();
                        match vfs.overlay_mount(upperdir.as_deref(), lowerdir_refs, &target_str) {
                            Ok(_) => 0,
                            Err(_) => usize::MAX,
                        }
                    },
                    Err(_) => usize::MAX,
                }
            } else {
                usize::MAX // Overlay requires options
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
    
    // Get the filesystem driver manager
    let driver_manager = get_fs_driver_manager();
    
    // Create filesystem using the driver
    let filesystem = driver_manager.create_from_option_string(fstype, options)?;
    
    // Register the filesystem with VFS and get fs_id
    let fs_id = vfs.register_fs(filesystem);
    
    // Mount the filesystem at the target path
    vfs.mount(fs_id, target)?;
    
    Ok(())
}

pub fn sys_umount(trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let target_ptr = task.vm_manager.translate_vaddr(trapframe.get_arg(0)).unwrap() as *const u8;
    let _flags = trapframe.get_arg(1) as u32; // Reserved for future use

    trapframe.increment_pc_next(task);

    // Convert target path to string
    let target_str = match cstring_to_string(target_ptr, MAX_PATH_LENGTH) {
        Ok((s, _)) => match VfsManager::to_absolute_path(&task, &s) {
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