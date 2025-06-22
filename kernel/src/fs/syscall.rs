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
            // Handle bind mount
            let read_only = (flags & 1) != 0; // MS_RDONLY
            match vfs.bind_mount(&source_str, &target_str, read_only) {
                Ok(_) => 0,
                Err(_) => usize::MAX,
            }
        },
        "overlay" => {
            // Handle overlay mount - parse data for upperdir/lowerdir
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
        "tmpfs" => {
            // Handle tmpfs mount
            let memory_limit = if let Some(data) = data_str {
                parse_tmpfs_size(&data).unwrap_or(64 * 1024 * 1024) // Default 64MB
            } else {
                64 * 1024 * 1024 // Default 64MB
            };
            
            // Create tmpfs using the filesystem parameter system
            match create_tmpfs_and_mount(vfs, &target_str, memory_limit) {
                Ok(_) => 0,
                Err(_) => usize::MAX,
            }
        },
        "cpiofs" => {
            // Handle memory-based filesystem (initramfs, etc.)
            if let Some(data) = data_str {
                match parse_memory_range(&data) {
                    Ok((start, end)) => {
                        let memory_area = crate::vm::vmem::MemoryArea::new(start, end - start);
                        match vfs.create_and_register_memory_fs("cpiofs", &memory_area) {
                            Ok(fs_id) => {
                                match vfs.mount(fs_id, &target_str) {
                                    Ok(_) => 0,
                                    Err(_) => usize::MAX,
                                }
                            },
                            Err(_) => usize::MAX,
                        }
                    },
                    Err(_) => usize::MAX,
                }
            } else {
                usize::MAX // Memory FS requires data with memory range
            }
        },
        _ => {
            // Handle block device mount (ext4, etc.)
            // For now, assume it's a block device mount
            match create_block_fs_and_mount(vfs, &fstype_str, &source_str, &target_str) {
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

// Helper function to parse tmpfs size option
fn parse_tmpfs_size(data: &str) -> Result<usize, ()> {
    for option in data.split(',') {
        if let Some(size_str) = option.strip_prefix("size=") {
            // Parse size with suffix (K, M, G)
            let size_str = size_str.trim();
            if size_str.is_empty() {
                continue;
            }
            
            let (number_part, multiplier) = if size_str.ends_with('K') || size_str.ends_with('k') {
                (&size_str[..size_str.len()-1], 1024)
            } else if size_str.ends_with('M') || size_str.ends_with('m') {
                (&size_str[..size_str.len()-1], 1024 * 1024)
            } else if size_str.ends_with('G') || size_str.ends_with('g') {
                (&size_str[..size_str.len()-1], 1024 * 1024 * 1024)
            } else {
                (size_str, 1)
            };
            
            if let Ok(number) = number_part.parse::<usize>() {
                return Ok(number * multiplier);
            }
        }
    }
    Err(())
}

// Helper function to parse memory range (start,end)
fn parse_memory_range(data: &str) -> Result<(usize, usize), ()> {
    let parts: Vec<&str> = data.split(',').collect();
    if parts.len() != 2 {
        return Err(());
    }
    
    let start = if parts[0].starts_with("0x") {
        usize::from_str_radix(&parts[0][2..], 16).map_err(|_| ())?
    } else {
        parts[0].parse().map_err(|_| ())?
    };
    
    let end = if parts[1].starts_with("0x") {
        usize::from_str_radix(&parts[1][2..], 16).map_err(|_| ())?
    } else {
        parts[1].parse().map_err(|_| ())?
    };
    
    Ok((start, end))
}

// Helper function to create and mount tmpfs
fn create_tmpfs_and_mount(_vfs: &VfsManager, _mount_point: &str, _memory_limit: usize) -> Result<(), super::FileSystemError> {
    // This would typically create tmpfs params and use create_and_register_fs_with_params
    // For now, simplified implementation
    // TODO: Implement proper tmpfs parameter handling
    Err(super::FileSystemError {
        kind: super::FileSystemErrorKind::NotFound,
        message: "Tmpfs creation not fully implemented".to_string(),
    })
}

// Helper function to create and mount block filesystem
fn create_block_fs_and_mount(_vfs: &VfsManager, _fstype: &str, _device_path: &str, _mount_point: &str) -> Result<(), super::FileSystemError> {
    // This would typically open the block device and create filesystem
    // For now, simplified implementation
    // TODO: Implement proper block device handling
    Err(super::FileSystemError {
        kind: super::FileSystemErrorKind::NotFound,
        message: "Block device filesystem creation not fully implemented".to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_parse_overlay_options_basic() {
        // Test basic overlay options with single lowerdir
        let result = parse_overlay_options("lowerdir=/lower");
        assert!(result.is_ok());
        let (upperdir, lowerdirs) = result.unwrap();
        assert!(upperdir.is_none());
        assert_eq!(lowerdirs.len(), 1);
        assert_eq!(lowerdirs[0], "/lower");
    }

    #[test_case]
    fn test_parse_overlay_options_with_upper() {
        // Test overlay options with upperdir and lowerdir
        let result = parse_overlay_options("upperdir=/upper, lowerdir=/lower");
        assert!(result.is_ok());
        let (upperdir, lowerdirs) = result.unwrap();
        assert!(upperdir.is_some());
        assert_eq!(upperdir.unwrap(), "/upper");
        assert_eq!(lowerdirs.len(), 1);
        assert_eq!(lowerdirs[0], "/lower");
    }

    #[test_case]
    fn test_parse_overlay_options_multiple_lower() {
        // Test overlay options with multiple lowerdirs
        let result = parse_overlay_options("lowerdir=/lower1:/lower2:/lower3");
        assert!(result.is_ok());
        let (upperdir, lowerdirs) = result.unwrap();
        assert!(upperdir.is_none());
        assert_eq!(lowerdirs.len(), 3);
        assert_eq!(lowerdirs[0], "/lower1");
        assert_eq!(lowerdirs[1], "/lower2");
        assert_eq!(lowerdirs[2], "/lower3");
    }

    #[test_case]
    fn test_parse_overlay_options_complex() {
        // Test complex overlay options
        let result = parse_overlay_options("upperdir=/upper,lowerdir=/lower1:/lower2,workdir=/work");
        assert!(result.is_ok());
        let (upperdir, lowerdirs) = result.unwrap();
        assert!(upperdir.is_some());
        assert_eq!(upperdir.unwrap(), "/upper");
        assert_eq!(lowerdirs.len(), 2);
        assert_eq!(lowerdirs[0], "/lower1");
        assert_eq!(lowerdirs[1], "/lower2");
    }

    #[test_case]
    fn test_parse_overlay_options_no_lowerdir() {
        // Test overlay options without lowerdir (should fail)
        let result = parse_overlay_options("upperdir=/upper");
        assert!(result.is_err());
    }

    #[test_case]
    fn test_parse_overlay_options_empty() {
        // Test empty options (should fail)
        let result = parse_overlay_options("");
        assert!(result.is_err());
    }

    #[test_case]
    fn test_parse_tmpfs_size_bytes() {
        // Test parsing size in bytes
        let result = parse_tmpfs_size("size=1048576");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 1048576);
    }

    #[test_case]
    fn test_parse_tmpfs_size_kilobytes() {
        // Test parsing size in kilobytes
        let result = parse_tmpfs_size("size=10K");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 10 * 1024);

        let result = parse_tmpfs_size("size=5k");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 5 * 1024);
    }

    #[test_case]
    fn test_parse_tmpfs_size_megabytes() {
        // Test parsing size in megabytes
        let result = parse_tmpfs_size("size=64M");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 64 * 1024 * 1024);

        let result = parse_tmpfs_size("size=128m");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 128 * 1024 * 1024);
    }

    #[test_case]
    fn test_parse_tmpfs_size_gigabytes() {
        // Test parsing size in gigabytes
        let result = parse_tmpfs_size("size=2G");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 2 * 1024 * 1024 * 1024);

        let result = parse_tmpfs_size("size=1g");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 1 * 1024 * 1024 * 1024);
    }

    #[test_case]
    fn test_parse_tmpfs_size_multiple_options() {
        // Test parsing size with multiple options
        let result = parse_tmpfs_size("nodev,size=32M,noexec");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 32 * 1024 * 1024);
    }

    #[test_case]
    fn test_parse_tmpfs_size_no_size() {
        // Test parsing without size option (should fail)
        let result = parse_tmpfs_size("nodev,noexec");
        assert!(result.is_err());
    }

    #[test_case]
    fn test_parse_tmpfs_size_invalid() {
        // Test parsing invalid size
        let result = parse_tmpfs_size("size=invalid");
        assert!(result.is_err());

        let result = parse_tmpfs_size("size=");
        assert!(result.is_err());
    }

    #[test_case]
    fn test_parse_memory_range_decimal() {
        // Test parsing memory range in decimal
        let result = parse_memory_range("134217728,268435456");
        assert!(result.is_ok());
        let (start, end) = result.unwrap();
        assert_eq!(start, 134217728);
        assert_eq!(end, 268435456);
    }

    #[test_case]
    fn test_parse_memory_range_hex() {
        // Test parsing memory range in hexadecimal
        let result = parse_memory_range("0x80000000,0x81000000");
        assert!(result.is_ok());
        let (start, end) = result.unwrap();
        assert_eq!(start, 0x80000000);
        assert_eq!(end, 0x81000000);
    }

    #[test_case]
    fn test_parse_memory_range_mixed() {
        // Test parsing memory range with mixed formats
        let result = parse_memory_range("0x80000000,268435456");
        assert!(result.is_ok());
        let (start, end) = result.unwrap();
        assert_eq!(start, 0x80000000);
        assert_eq!(end, 268435456);
    }

    #[test_case]
    fn test_parse_memory_range_invalid_format() {
        // Test parsing invalid memory range formats
        let result = parse_memory_range("0x80000000");
        assert!(result.is_err());

        let result = parse_memory_range("0x80000000,0x81000000,extra");
        assert!(result.is_err());

        let result = parse_memory_range("");
        assert!(result.is_err());
    }

    #[test_case]
    fn test_parse_memory_range_invalid_numbers() {
        // Test parsing invalid numbers
        let result = parse_memory_range("invalid,0x81000000");
        assert!(result.is_err());

        let result = parse_memory_range("0x80000000,invalid");
        assert!(result.is_err());

        let result = parse_memory_range("0xinvalid,0x81000000");
        assert!(result.is_err());
    }

    #[test_case]
    fn test_parse_memory_range_boundary_cases() {
        // Test boundary cases
        let result = parse_memory_range("0,1");
        assert!(result.is_ok());
        let (start, end) = result.unwrap();
        assert_eq!(start, 0);
        assert_eq!(end, 1);

        let result = parse_memory_range("0x0,0xffffffff");
        assert!(result.is_ok());
        let (start, end) = result.unwrap();
        assert_eq!(start, 0);
        assert_eq!(end, 0xffffffff);
    }
}