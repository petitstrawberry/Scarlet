use alloc::{boxed::Box, string::{String, ToString}, vec, vec::Vec};
use crate::{
    abi::xv6::riscv64::fs::xv6fs::{Dirent, Stat}, 
    arch::Trapframe, 
    device::manager::DeviceManager, 
    executor::TransparentExecutor, 
    fs::{
        FileType, 
        SeekFrom,
        DirectoryEntry, // Legacy support for conversion
        DeviceFileInfo,
    }, 
    library::std::string::{
        cstring_to_string, 
        parse_c_string_from_userspace, 
        parse_string_array_from_userspace, 
    }, 
    object::capability::StreamError, 
    sched::scheduler::get_scheduler, 
    task::mytask, 
};

/// Convert Scarlet DirectoryEntry to xv6 Dirent and write to buffer
fn read_directory_as_xv6_dirent(buf_ptr: *mut u8, count: usize, buffer_data: &[u8]) -> usize {
    if count < Dirent::DIRENT_SIZE {
        return 0; // Buffer too small for even one entry
    }

    // Parse DirectoryEntry from buffer data
    if let Some(dir_entry) = DirectoryEntry::parse(buffer_data) {
        // Convert Scarlet DirectoryEntry to xv6 Dirent
        let inum = (dir_entry.file_id & 0xFFFF) as u16; // Use lower 16 bits as inode number
        let name = dir_entry.name_str().unwrap_or("");
        
        let xv6_dirent = Dirent::new(inum, name);
        
        // Check if we have enough space
        if count >= Dirent::DIRENT_SIZE {
            // Copy the dirent to the buffer
            let dirent_bytes = xv6_dirent.as_bytes();
            unsafe {
                core::ptr::copy_nonoverlapping(
                    dirent_bytes.as_ptr(),
                    buf_ptr,
                    Dirent::DIRENT_SIZE
                );
            }
            return Dirent::DIRENT_SIZE;
        }
    }
    
    0 // No data or error
}

const MAX_PATH_LENGTH: usize = 128;
const MAX_ARG_COUNT: usize = 64;

pub fn sys_exec(_abi: &mut crate::abi::xv6::riscv64::Xv6Riscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    
    // Increment PC to avoid infinite loop if execve fails
    trapframe.increment_pc_next(task);
    
    // Get arguments from trapframe
    let path_ptr = trapframe.get_arg(0);
    let argv_ptr = trapframe.get_arg(1);
    
    // Parse path
    let path_str = match parse_c_string_from_userspace(task, path_ptr, MAX_PATH_LENGTH) {
        Ok(path) => match to_absolute_path_v2(&task, &path) {
            Ok(abs_path) => abs_path,
            Err(_) => return usize::MAX, // Path error
        },
        Err(_) => return usize::MAX, // Path parsing error
    };
    
    // Parse argv and envp
    let argv_strings = match parse_string_array_from_userspace(task, argv_ptr, MAX_ARG_COUNT, MAX_PATH_LENGTH) {
        Ok(args) => args,
        Err(_) => return usize::MAX, // argv parsing error
    };
    
    // Convert Vec<String> to Vec<&str> for TransparentExecutor
    let argv_refs: Vec<&str> = argv_strings.iter().map(|s| s.as_str()).collect();
    
    // Use TransparentExecutor for cross-ABI execution
    match TransparentExecutor::execute_binary(&path_str, &argv_refs, &[], task, trapframe, false) {
        Ok(_) => {
            // execve normally should not return on success - the process is replaced
            // However, if ABI module sets trapframe return value and returns here,
            // we should respect that value instead of hardcoding 0
            trapframe.get_return_value()
        },
        Err(_) => {
            // Execution failed - return error code
            // The trap handler will automatically set trapframe return value from our return
            usize::MAX // Error return value
        }
    }
}

#[repr(i32)]
enum OpenMode {
    ReadOnly  = 0x000,
    WriteOnly = 0x001,
    ReadWrite = 0x002,
    Create    = 0x200,
    Truncate  = 0x400,
}

pub fn sys_open(abi: &mut crate::abi::xv6::riscv64::Xv6Riscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let path_ptr = task.vm_manager.translate_vaddr(trapframe.get_arg(0)).unwrap() as *const u8;
    let mode = trapframe.get_arg(1) as i32;

    // Increment PC to avoid infinite loop if open fails
    trapframe.increment_pc_next(task);

    // Convert path bytes to string
    let path_str = match cstring_to_string(path_ptr, MAX_PATH_LENGTH) {
        Ok((path, _)) => match to_absolute_path_v2(&task, &path) {
            Ok(abs_path) => abs_path,
            Err(_) => return usize::MAX,
        },
        Err(_) => return usize::MAX, // Invalid UTF-8
    };

    // Use task's VFS manager
    let vfs = task.vfs.as_ref().unwrap();

    // Try to open the file
    let file = vfs.open(&path_str, 0);

    match file {
        Ok(kernel_obj) => {
            // Register the file with the task using HandleTable
            let handle = task.handle_table.insert(kernel_obj);
            match handle {
                Ok(handle) => {
                    match abi.allocate_fd(handle as u32) {
                        Ok(fd) => fd,
                        Err(_) => usize::MAX, // Too many open files
                    }
                },
                Err(_) => usize::MAX, // Handle table full
            }
        }
        Err(_) =>{
            // If the file does not exist and we are trying to create it
            if mode & OpenMode::Create as i32 != 0 {
                let res = vfs.create_file(&path_str, FileType::RegularFile);
                if res.is_err() {
                    return usize::MAX; // File creation error
                }
                match vfs.open(&path_str, 0) {
                    Ok(kernel_obj) => {
                        // Register the file with the task using HandleTable
                        let handle = task.handle_table.insert(kernel_obj);
                        match handle {
                            Ok(handle) => {
                                match abi.allocate_fd(handle as u32) {
                                    Ok(fd) => fd,
                                    Err(_) => usize::MAX, // Too many open files
                                }
                            },
                            Err(_) => usize::MAX, // Handle table full
                        }
                    }
                    Err(_) => usize::MAX, // File open error
                }
            } else {
                return usize::MAX; // VFS not initialized
            }
        }
    }
}

pub fn sys_dup(abi: &mut crate::abi::xv6::riscv64::Xv6Riscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let fd = trapframe.get_arg(0) as usize;
    trapframe.increment_pc_next(task);

    // Get handle from XV6 fd
    if let Some(old_handle) = abi.get_handle(fd) {
        if let Some(old_kernel_obj) = task.handle_table.get(old_handle) {
            let kernel_obj = old_kernel_obj.clone();
            let handle = task.handle_table.insert(kernel_obj);
            match handle {
                Ok(new_handle) => {
                    match abi.allocate_fd(new_handle as u32) {
                        Ok(fd) => fd,
                        Err(_) => usize::MAX, // Too many open files
                    }
                },
                Err(_) => usize::MAX, // Handle table full
            }
        } else {
            usize::MAX // Handle not found in handle table
        }
    } else {
        usize::MAX // Invalid file descriptor
    }
}

pub fn sys_close(abi: &mut crate::abi::xv6::riscv64::Xv6Riscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let fd = trapframe.get_arg(0) as usize;
    trapframe.increment_pc_next(task);
    
    // Get handle from XV6 fd and remove mapping
    if let Some(handle) = abi.remove_fd(fd) {
        if task.handle_table.remove(handle).is_some() {
            0 // Success
        } else {
            usize::MAX // Handle not found in handle table
        }
    } else {
        usize::MAX // Invalid file descriptor
    }
}

pub fn sys_read(abi: &mut crate::abi::xv6::riscv64::Xv6Riscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let fd = trapframe.get_arg(0) as usize;
    let buf_ptr = task.vm_manager.translate_vaddr(trapframe.get_arg(1)).unwrap() as *mut u8;
    let count = trapframe.get_arg(2) as usize;

    let epc = trapframe.epc;

    // Increment PC to avoid infinite loop if read fails
    trapframe.increment_pc_next(task);

    // Get handle from XV6 fd
    let handle = match abi.get_handle(fd) {
        Some(h) => h,
        None => return usize::MAX, // Invalid file descriptor
    };

    let kernel_obj = match task.handle_table.get(handle) {
        Some(obj) => obj,
        None => return usize::MAX, // Invalid file descriptor
    };

    // Check if this is a directory by getting file metadata
    let is_directory = if let Some(file_obj) = kernel_obj.as_file() {
        if let Ok(metadata) = file_obj.metadata() {
            matches!(metadata.file_type, FileType::Directory)
        } else {
            false
        }
    } else {
        false
    };

    let stream = match kernel_obj.as_stream() {
        Some(stream) => stream,
        None => return usize::MAX, // Not a stream object
    };

    if is_directory {
        // For directories, we need a larger buffer to read DirectoryEntry, then convert to Dirent
        let directory_entry_size = core::mem::size_of::<DirectoryEntry>();
        let mut temp_buffer = vec![0u8; directory_entry_size];
        
        match stream.read(&mut temp_buffer) {
            Ok(n) => {
                if n > 0 && n >= directory_entry_size {
                    // Convert DirectoryEntry to xv6 Dirent
                    let converted_bytes = read_directory_as_xv6_dirent(buf_ptr, count, &temp_buffer[..n]);
                    if converted_bytes > 0 {
                        return converted_bytes; // Return converted xv6 dirent size
                    }
                }
                0 // EOF or no valid directory entry
            },
            Err(e) => {
                match e {
                    StreamError::EndOfStream => 0, // EOF
                    StreamError::WouldBlock => {
                        // If the stream would block, we need to set the trapframe's EPC
                        trapframe.epc = epc;
                        task.vcpu.store(trapframe); // Store the trapframe in the task's vcpu
                        get_scheduler().schedule(trapframe); // Yield to the scheduler
                        trapframe.get_return_value() // Return the value from the trapframe
                    },
                    _ => usize::MAX, // Other errors
                }
            }
        }
    } else {
        // For regular files, use the user-provided buffer directly
        let mut buffer = unsafe { core::slice::from_raw_parts_mut(buf_ptr, count) };
        
        match stream.read(&mut buffer) {
            Ok(n) => n, // Return original read size for regular files
            Err(e) => {
                match e {
                    StreamError::EndOfStream => 0, // EOF
                    StreamError::WouldBlock => {
                        // If the stream would block, we need to set the trapframe's EPC
                        trapframe.epc = epc;
                        task.vcpu.store(trapframe); // Store the trapframe in the task's vcpu
                        get_scheduler().schedule(trapframe); // Yield to the scheduler
                        trapframe.get_return_value() // Return the value from the trapframe
                    },
                    _ => {
                        // Other errors, return -1
                        usize::MAX
                    }
                }
            }
        }
    }
}

pub fn sys_write(abi: &mut crate::abi::xv6::riscv64::Xv6Riscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let fd = trapframe.get_arg(0) as usize;
    let buf_ptr = task.vm_manager.translate_vaddr(trapframe.get_arg(1)).unwrap() as *const u8;
    let count = trapframe.get_arg(2) as usize;

    // Increment PC to avoid infinite loop if write fails
    trapframe.increment_pc_next(task);

    // Get handle from XV6 fd
    let handle = match abi.get_handle(fd) {
        Some(h) => h,
        None => return usize::MAX, // Invalid file descriptor
    };

    let kernel_obj = match task.handle_table.get(handle) {
        Some(obj) => obj,
        None => return usize::MAX, // Invalid file descriptor
    };

    let stream = match kernel_obj.as_stream() {
        Some(stream) => stream,
        None => return usize::MAX, // Not a stream object
    };

    let buffer = unsafe { core::slice::from_raw_parts(buf_ptr, count) };

    match stream.write(buffer) {
        Ok(n) => n,
        Err(_) => usize::MAX, // Write error
    }
}

pub fn sys_lseek(abi: &mut crate::abi::xv6::riscv64::Xv6Riscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let fd = trapframe.get_arg(0) as usize;
    let offset = trapframe.get_arg(1) as i64;
    let whence = trapframe.get_arg(2) as i32;

    // Increment PC to avoid infinite loop if lseek fails
    trapframe.increment_pc_next(task);

    // Get handle from XV6 fd
    let handle = match abi.get_handle(fd) {
        Some(h) => h,
        None => return usize::MAX, // Invalid file descriptor
    };

    let kernel_obj = match task.handle_table.get(handle) {
        Some(obj) => obj,
        None => return usize::MAX, // Invalid file descriptor
    };

    let file = match kernel_obj.as_file() {
        Some(file) => file,
        None => return usize::MAX, // Not a file object
    };

    let whence = match whence {
        0 => SeekFrom::Start(offset as u64),
        1 => SeekFrom::Current(offset),
        2 => SeekFrom::End(offset),
        _ => return usize::MAX, // Invalid whence
    };

    match file.seek(whence) {
        Ok(pos) => pos as usize,
        Err(_) => usize::MAX, // Lseek error
    }
}

// Create device file
pub fn sys_mknod(_abi: &mut crate::abi::xv6::riscv64::Xv6Riscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);
    let name_ptr = task.vm_manager.translate_vaddr(trapframe.get_arg(0)).unwrap() as *const u8;
    let name = get_path_str_v2(name_ptr).unwrap();
    let path = to_absolute_path_v2(&task, &name).unwrap();

    let major = trapframe.get_arg(1) as u32;
    let minor = trapframe.get_arg(2) as u32;

    match (major, minor) {
        (1, 0) => {
            // Create a console device
            let console_dev = Some(DeviceManager::get_mut_manager().register_device(Box::new(
                crate::abi::xv6::drivers::console::ConsoleDevice::new(0, "console")
            )));
        
            let vfs = task.vfs.as_mut().unwrap();
            let _res = vfs.create_file(&path, FileType::CharDevice(
                DeviceFileInfo {
                    device_id: console_dev.unwrap(),
                    device_type: crate::device::DeviceType::Char,
                }
            ));
            crate::println!("Created console device at {}", path);
        },
        _ => {},
    }
    0
}


pub fn sys_fstat(abi: &mut crate::abi::xv6::riscv64::Xv6Riscv64Abi, trapframe: &mut crate::arch::Trapframe) -> usize {
    let fd = trapframe.get_arg(0) as usize;

    let task = mytask()
        .expect("sys_fstat: No current task found");
    trapframe.increment_pc_next(task); // Increment the program counter

    let stat_ptr = task.vm_manager.translate_vaddr(trapframe.get_arg(1) as usize)
        .expect("sys_fstat: Failed to translate stat pointer") as *mut Stat;
    
    // Get handle from XV6 fd
    let handle = match abi.get_handle(fd) {
        Some(h) => h,
        None => return usize::MAX, // Invalid file descriptor
    };
    
    let kernel_obj = match task.handle_table.get(handle) {
        Some(obj) => obj,
        None => return usize::MAX, // Return -1 on error
    };

    let file = match kernel_obj.as_file() {
        Some(file) => file,
        None => return usize::MAX, // Not a file object
    };

    let metadata = file.metadata()
        .expect("sys_fstat: Failed to get file metadata");

    if stat_ptr.is_null() {
        return usize::MAX; // Return -1 if stat pointer is null
    }
    
    let stat = unsafe { &mut *stat_ptr };

    *stat = Stat {
        dev: 0,
        ino: metadata.file_id as u32,
        file_type: match metadata.file_type {
            FileType::Directory => 1, // T_DIR
            FileType::RegularFile => 2,      // T_FILE
            FileType::CharDevice(_) => 3, // T_DEVICE
            FileType::BlockDevice(_) => 3, // T_DEVICE
            _ => 0, // Unknown type
        },
        nlink: 1,
        size: metadata.size as u64,
    };

    0
}

pub fn sys_mkdir(_abi: &mut crate::abi::xv6::riscv64::Xv6Riscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);
    
    let path_ptr = task.vm_manager.translate_vaddr(trapframe.get_arg(0)).unwrap() as *const u8;
    let path = match get_path_str_v2(path_ptr) {
        Ok(p) => to_absolute_path_v2(&task, &p).unwrap(),
        Err(_) => return usize::MAX, // Invalid path
    };

    // Try to create the directory
    let vfs = task.vfs.as_mut().unwrap();
    match vfs.create_dir(&path) {
        Ok(_) => 0, // Success
        Err(_) => usize::MAX, // Error
    }
}

pub fn sys_unlink(_abi: &mut crate::abi::xv6::riscv64::Xv6Riscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);
    
    let path_ptr = task.vm_manager.translate_vaddr(trapframe.get_arg(0)).unwrap() as *const u8;
    let path = match cstring_to_string(path_ptr, MAX_PATH_LENGTH) {
        Ok((p, _)) => to_absolute_path_v2(&task, &p).unwrap(),
        Err(_) => return usize::MAX, // Invalid path
    };

    // Try to remove the file or directory
    let vfs = task.vfs.as_mut().unwrap();
    match vfs.remove(&path) {
        Ok(_) => 0, // Success
        Err(_) => usize::MAX, // Error
    }
}

pub fn sys_link(_abi: &mut crate::abi::xv6::riscv64::Xv6Riscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);
    
    let src_path_ptr = task.vm_manager.translate_vaddr(trapframe.get_arg(0)).unwrap() as *const u8;
    let dst_path_ptr = task.vm_manager.translate_vaddr(trapframe.get_arg(1)).unwrap() as *const u8;

    let src_path = match cstring_to_string(src_path_ptr, MAX_PATH_LENGTH) {
        Ok((p, _)) => to_absolute_path_v2(&task, &p).unwrap(),
        Err(_) => return usize::MAX, // Invalid path
    };

    let dst_path = match cstring_to_string(dst_path_ptr, MAX_PATH_LENGTH) {
        Ok((p, _)) => to_absolute_path_v2(&task, &p).unwrap(),
        Err(_) => return usize::MAX, // Invalid path
    };

    let vfs = task.vfs.as_ref().unwrap();
    match vfs.create_hardlink(&src_path, &dst_path) {
        Ok(_) => 0, // Success
        Err(err) => {
            use crate::fs::FileSystemErrorKind;
            
            // Map VFS errors to appropriate errno values for xv6
            match err.kind {
                FileSystemErrorKind::NotFound => {
                    // Source file doesn't exist
                    2 // ENOENT
                },
                FileSystemErrorKind::FileExists => {
                    // Destination already exists
                    17 // EEXIST
                },
                FileSystemErrorKind::CrossDevice => {
                    // Hard links across devices not supported
                    18 // EXDEV
                },
                FileSystemErrorKind::InvalidOperation => {
                    // Operation not supported (e.g., directory hardlink)
                    1 // EPERM
                },
                FileSystemErrorKind::PermissionDenied => {
                    13 // EACCES
                },
                _ => {
                    // Other errors
                    5 // EIO
                }
            }
        }
    }
}

/// VFS v2 helper function for path absolutization
/// TODO: Move this to a shared helper module when VFS v2 provides public API
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

/// Helper function to replace the missing get_path_str function
/// TODO: This should be moved to a shared helper when VFS v2 provides public API
fn get_path_str_v2(ptr: *const u8) -> Result<String, ()> {
    const MAX_PATH_LENGTH: usize = 128;
    cstring_to_string(ptr, MAX_PATH_LENGTH).map(|(s, _)| s).map_err(|_| ())
}