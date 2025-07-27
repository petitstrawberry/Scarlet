use alloc::{string::{String, ToString}, sync::Arc, vec::Vec, vec};
use crate::{
    abi::{linux::riscv64::LinuxRiscv64Abi}, 
    arch::Trapframe, 
    device::manager::DeviceManager, 
    executor::TransparentExecutor, 
    fs::{
        DeviceFileInfo, DirectoryEntry, FileType, SeekFrom
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

/// Linux stat structure for RISC-V 64-bit
/// This structure matches the Linux kernel's definition for newstat on RISC-V 64-bit
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct LinuxStat {
    pub st_dev: u64,        // Device ID of device containing file
    pub st_ino: u64,        // Inode number
    pub st_mode: u32,       // File type and mode
    pub st_nlink: u32,      // Number of hard links
    pub st_uid: u32,        // User ID of owner
    pub st_gid: u32,        // Group ID of owner
    pub st_rdev: u64,       // Device ID (if special file)
    pub st_size: i64,       // Total size, in bytes
    pub st_blksize: i32,    // Block size for filesystem I/O
    pub st_blocks: i64,     // Number of 512B blocks allocated
    pub st_atime: i64,      // Time of last access (seconds)
    pub st_atime_nsec: i64, // Time of last access (nanoseconds)
    pub st_mtime: i64,      // Time of last modification (seconds)
    pub st_mtime_nsec: i64, // Time of last modification (nanoseconds)  
    pub st_ctime: i64,      // Time of last status change (seconds)
    pub st_ctime_nsec: i64, // Time of last status change (nanoseconds)
    pub __unused: [i32; 2], // Reserved for future use
}

// Linux file type constants for st_mode field
pub const S_IFMT: u32 = 0o170000;   // Bit mask for the file type bit field
pub const S_IFSOCK: u32 = 0o140000; // Socket
pub const S_IFLNK: u32 = 0o120000;  // Symbolic link
pub const S_IFREG: u32 = 0o100000;  // Regular file
pub const S_IFBLK: u32 = 0o060000;  // Block device
pub const S_IFDIR: u32 = 0o040000;  // Directory
pub const S_IFCHR: u32 = 0o020000;  // Character device
pub const S_IFIFO: u32 = 0o010000;  // FIFO

// Linux permission constants
pub const S_IRWXU: u32 = 0o0700;    // User (file owner) has read, write, and execute permission
pub const S_IRUSR: u32 = 0o0400;    // User has read permission
pub const S_IWUSR: u32 = 0o0200;    // User has write permission
pub const S_IXUSR: u32 = 0o0100;    // User has execute permission
pub const S_IRWXG: u32 = 0o0070;    // Group has read, write, and execute permission
pub const S_IRGRP: u32 = 0o0040;    // Group has read permission
pub const S_IWGRP: u32 = 0o0020;    // Group has write permission
pub const S_IXGRP: u32 = 0o0010;    // Group has execute permission
pub const S_IRWXO: u32 = 0o0007;    // Others have read, write, and execute permission
pub const S_IROTH: u32 = 0o0004;    // Others have read permission
pub const S_IWOTH: u32 = 0o0002;    // Others have write permission
pub const S_IXOTH: u32 = 0o0001;    // Others have execute permission

// Linux fcntl command constants
pub const F_DUPFD: u32 = 0;          // Duplicate file descriptor
pub const F_GETFD: u32 = 1;          // Get file descriptor flags
pub const F_SETFD: u32 = 2;          // Set file descriptor flags
pub const F_GETFL: u32 = 3;          // Get file status flags
pub const F_SETFL: u32 = 4;          // Set file status flags
pub const F_GETLK: u32 = 5;          // Get record locking information
pub const F_SETLK: u32 = 6;          // Set record lock (non-blocking)
pub const F_SETLKW: u32 = 7;         // Set record lock (blocking)
pub const F_SETOWN: u32 = 8;         // Set owner (process receiving SIGIO/SIGURG)
pub const F_GETOWN: u32 = 9;         // Get owner (process receiving SIGIO/SIGURG)
pub const F_SETSIG: u32 = 10;        // Set signal sent when I/O is possible
pub const F_GETSIG: u32 = 11;        // Get signal sent when I/O is possible
pub const F_SETLEASE: u32 = 1024;    // Set a lease
pub const F_GETLEASE: u32 = 1025;    // Get current lease
pub const F_NOTIFY: u32 = 1026;      // Request notifications on a directory
pub const F_DUPFD_CLOEXEC: u32 = 1030; // Duplicate with close-on-exec

// Linux file descriptor flags
pub const FD_CLOEXEC: u32 = 1;          // Close-on-exec flag

impl LinuxStat {
    /// Create a new LinuxStat from Scarlet FileMetadata
    pub fn from_metadata(metadata: &crate::fs::FileMetadata) -> Self {
        let st_mode = match metadata.file_type {
            FileType::RegularFile => S_IFREG,
            FileType::Directory => S_IFDIR,
            FileType::CharDevice(_) => S_IFCHR,
            FileType::BlockDevice(_) => S_IFBLK,
            FileType::SymbolicLink(_) => S_IFLNK,
            FileType::Pipe => S_IFIFO,
            FileType::Socket => S_IFSOCK,
            FileType::Unknown => S_IFREG, // Default to regular file
        } | if metadata.permissions.read { S_IRUSR | S_IRGRP | S_IXGRP | S_IROTH } else { 0 }
          | if metadata.permissions.write { S_IWUSR } else { 0 }
          | if metadata.permissions.execute { S_IXUSR | S_IXGRP | S_IXOTH } else { 0 };

        Self {
            st_dev: 0, // Virtual device ID
            st_ino: metadata.file_id,
            st_mode,
            st_nlink: metadata.link_count as u32,
            st_uid: 0, // Root user
            st_gid: 0, // Root group
            st_rdev: 0, // Not a special file by default
            st_size: metadata.size as i64,
            st_blksize: 4096, // Standard block size
            st_blocks: ((metadata.size + 511) / 512) as i64, // Number of 512-byte blocks
            st_atime: metadata.accessed_time as i64,
            st_atime_nsec: 0,
            st_mtime: metadata.modified_time as i64,
            st_mtime_nsec: 0,
            st_ctime: metadata.created_time as i64,
            st_ctime_nsec: 0,
            __unused: [0; 2],
        }
    }
}

// /// Convert Scarlet DirectoryEntry to Linux Dirent and write to buffer
// fn read_directory_as_Linux_dirent(buf_ptr: *mut u8, count: usize, buffer_data: &[u8]) -> usize {
//     if count < Dirent::DIRENT_SIZE {
//         return 0; // Buffer too small for even one entry
//     }

//     // Parse DirectoryEntry from buffer data
//     if let Some(dir_entry) = DirectoryEntry::parse(buffer_data) {
//         // Convert Scarlet DirectoryEntry to Linux Dirent
//         let inum = (dir_entry.file_id & 0xFFFF) as u16; // Use lower 16 bits as inode number
//         let name = dir_entry.name_str().unwrap_or("");
        
//         let Linux_dirent = Dirent::new(inum, name);
        
//         // Check if we have enough space
//         if count >= Dirent::DIRENT_SIZE {
//             // Copy the dirent to the buffer
//             let dirent_bytes = Linux_dirent.as_bytes();
//             unsafe {
//                 core::ptr::copy_nonoverlapping(
//                     dirent_bytes.as_ptr(),
//                     buf_ptr,
//                     Dirent::DIRENT_SIZE
//                 );
//             }
//             return Dirent::DIRENT_SIZE;
//         }
//     }
    
//     0 // No data or error
// }

const MAX_PATH_LENGTH: usize = 128;
const MAX_ARG_COUNT: usize = 64;

pub fn sys_exec(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
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

/// Linux sys_openat implementation for Scarlet VFS v2
///
/// Opens a file relative to a directory file descriptor (dirfd) and path.
/// If dirfd == AT_FDCWD, uses the current working directory as the base.
/// Otherwise, resolves the base directory from the file descriptor.
/// Uses VfsManager::open_relative for safe and efficient path resolution.
///
/// Arguments:
/// - abi: LinuxRiscv64Abi context
/// - trapframe: Trapframe containing syscall arguments
///
/// Returns:
/// - File descriptor on success
/// - usize::MAX (Linux -1) on error
pub fn sys_openat(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let dirfd = trapframe.get_arg(0) as i32;
    let path_ptr = task.vm_manager.translate_vaddr(trapframe.get_arg(1)).unwrap() as *const u8;
    let flags = trapframe.get_arg(2) as i32;

    // Increment PC to avoid infinite loop if openat fails
    trapframe.increment_pc_next(task);

    // Parse path from user space
    let path_str = match cstring_to_string(path_ptr, MAX_PATH_LENGTH) {
        Ok((path, _)) => path,
        Err(_) => return usize::MAX, // Invalid UTF-8
    };

    let vfs = task.vfs.as_ref().unwrap();

    // Determine base directory (entry and mount) for path resolution
    use crate::fs::vfs_v2::core::VfsFileObject;

    const AT_FDCWD: i32 = -100;
    let (base_entry, base_mount) = if dirfd == AT_FDCWD {
        // Use current working directory as base
        vfs.get_cwd().unwrap_or_else(|| {
            let root_mount = vfs.mount_tree.root_mount.read().clone();
            (root_mount.root.clone(), root_mount)
        })
    } else {
        // Use directory file descriptor as base
        let handle = match abi.get_handle(dirfd as usize) {
            Some(h) => h,
            None => return usize::MAX,
        };
        let kernel_obj = match task.handle_table.get(handle) {
            Some(obj) => obj,
            None => return usize::MAX,
        };
        let file_obj = match kernel_obj.as_file() {
            Some(f) => f,
            None => return usize::MAX,
        };
        let vfs_file_obj = file_obj.as_any().downcast_ref::<VfsFileObject>().ok_or(()).unwrap();
        (vfs_file_obj.get_vfs_entry().clone(), vfs_file_obj.get_mount_point().clone())
    };

    // Open the file using VfsManager::open_relative
    let file = vfs.open_from(&base_entry, &base_mount, &path_str, flags as u32);

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
        Err(_) => usize::MAX, // open_relative error
    }
}

pub fn sys_dup(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let fd = trapframe.get_arg(0) as usize;
    trapframe.increment_pc_next(task);

    // Get handle from Linux fd
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

pub fn sys_close(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let fd = trapframe.get_arg(0) as usize;
    trapframe.increment_pc_next(task);
    
    // Get handle from Linux fd and remove mapping
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

pub fn sys_read(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let fd = trapframe.get_arg(0) as usize;
    let buf_ptr = task.vm_manager.translate_vaddr(trapframe.get_arg(1)).unwrap() as *mut u8;
    let count = trapframe.get_arg(2) as usize;

    // Get handle from Linux fd
    let handle = match abi.get_handle(fd) {
        Some(h) => h,
        None => {
            trapframe.increment_pc_next(task);
            return usize::MAX; // Invalid file descriptor
        }
    };

    let kernel_obj = match task.handle_table.get(handle) {
        Some(obj) => obj,
        None => {
            trapframe.increment_pc_next(task);
            return usize::MAX; // Invalid file descriptor
        }
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
        None => {
            trapframe.increment_pc_next(task);
            return usize::MAX; // Not a stream object
        }
    };

    if is_directory {
        // // For directories, we need a larger buffer to read DirectoryEntry, then convert to Dirent
        // let directory_entry_size = core::mem::size_of::<DirectoryEntry>();
        // let mut temp_buffer = vec![0u8; directory_entry_size];
        
        // match stream.read(&mut temp_buffer) {
        //     Ok(n) => {
        //         trapframe.increment_pc_next(task); // Increment PC to avoid infinite loop
        //         if n > 0 && n >= directory_entry_size {
        //             // Convert DirectoryEntry to Linux Dirent
        //             let converted_bytes = read_directory_as_Linux_dirent(buf_ptr, count, &temp_buffer[..n]);
        //             if converted_bytes > 0 {
        //                 return converted_bytes; // Return converted Linux dirent size
        //             }
        //         }
        //         0 // EOF or no valid directory entry
        //     },
        //     Err(e) => {
        //         match e {
        //             StreamError::EndOfStream => {
        //                 trapframe.increment_pc_next(task); // Increment PC to avoid infinite loop
        //                 0 // EOF
        //             },
        //             StreamError::WouldBlock => {
        //                 // If the stream would block, we need to set the trapframe's EPC
        //                 // trapframe.epc = epc;
        //                 // task.vcpu.store(trapframe); // Store the trapframe in the task's vcpu
        //                 get_scheduler().schedule(trapframe); // Yield to the scheduler
        //             },
        //             _ => {
        //                 trapframe.increment_pc_next(task);
        //                 usize::MAX // Other errors
        //             }
        //         }
        //     }
        // }
        trapframe.increment_pc_next(task);
        return usize::MAX; // Directory reading not implemented yet
    } else {
        // For regular files, use the user-provided buffer directly
        let mut buffer = unsafe { core::slice::from_raw_parts_mut(buf_ptr, count) };
        
        match stream.read(&mut buffer) {
            Ok(n) => {
                trapframe.increment_pc_next(task); // Increment PC to avoid infinite loop
                n
            }, // Return original read size for regular files
            Err(e) => {
                match e {
                    StreamError::EndOfStream => {
                        trapframe.increment_pc_next(task); // Increment PC to avoid infinite loop
                        0 // EOF
                    },
                    StreamError::WouldBlock => {
                        get_scheduler().schedule(trapframe); // Yield to the scheduler
                        usize::MAX // Unreachable, but needed to satisfy return type
                    },
                    _ => {
                        // Other errors, return -1
                        trapframe.increment_pc_next(task); // Increment PC to avoid infinite loop
                        usize::MAX
                    }
                }
            }
        }
    }
}

pub fn sys_write(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let fd = trapframe.get_arg(0) as usize;
    let buf_ptr = task.vm_manager.translate_vaddr(trapframe.get_arg(1)).unwrap() as *const u8;
    let count = trapframe.get_arg(2) as usize;

    // Increment PC to avoid infinite loop if write fails
    trapframe.increment_pc_next(task);

    // Get handle from Linux fd
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

/// Linux writev system call implementation
/// 
/// This system call writes data from multiple buffers (I/O vectors) to a file descriptor.
/// It provides scatter-gather I/O functionality, allowing efficient writes from multiple
/// non-contiguous memory regions in a single system call.
/// 
/// # Arguments
/// - fd: File descriptor
/// - iovec: Array of iovec structures describing the buffers
/// - iovcnt: Number of iovec structures in the array
/// 
/// # Returns
/// - Number of bytes written on success
/// - usize::MAX on error (-1 in Linux)
pub fn sys_writev(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let fd = trapframe.get_arg(0) as usize;
    let iovec_ptr = trapframe.get_arg(1);
    let iovcnt = trapframe.get_arg(2) as usize;

    // Increment PC to avoid infinite loop if writev fails
    trapframe.increment_pc_next(task);

    // Validate parameters
    if iovcnt == 0 {
        return 0; // Nothing to write
    }

    // Linux typically limits iovcnt to prevent resource exhaustion
    const IOV_MAX: usize = 1024;
    if iovcnt > IOV_MAX {
        return usize::MAX; // Too many vectors
    }

    // Get handle from Linux fd
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

    // Translate and validate iovec array pointer
    let iovec_vaddr = match task.vm_manager.translate_vaddr(iovec_ptr) {
        Some(addr) => addr as *const IoVec,
        None => return usize::MAX, // Invalid address
    };

    if iovec_vaddr.is_null() {
        return usize::MAX; // NULL pointer
    }

    // Read iovec structures from user space
    let iovecs = unsafe { core::slice::from_raw_parts(iovec_vaddr, iovcnt) };

    let mut total_written = 0usize;

    // Process each iovec
    for iovec in iovecs {
        if iovec.iov_len == 0 {
            continue; // Skip empty buffers
        }

        // Translate buffer address
        let buf_vaddr = match task.vm_manager.translate_vaddr(iovec.iov_base as usize) {
            Some(addr) => addr as *const u8,
            None => return usize::MAX, // Invalid buffer address
        };

        if buf_vaddr.is_null() {
            return usize::MAX; // NULL buffer pointer
        }

        // Create a slice from the user buffer
        let buffer = unsafe { core::slice::from_raw_parts(buf_vaddr, iovec.iov_len) };

        // Write data from this buffer
        match stream.write(buffer) {
            Ok(n) => {
                total_written = total_written.saturating_add(n);
                
                // If partial write occurred, stop processing remaining vectors
                // This matches Linux behavior for writev
                if n < iovec.iov_len {
                    break;
                }
            }
            Err(_) => {
                // If no bytes were written at all, return error
                // If some bytes were written, return the count
                if total_written == 0 {
                    return usize::MAX;
                } else {
                    break;
                }
            }
        }
    }

    total_written
}

pub fn sys_lseek(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let fd = trapframe.get_arg(0) as usize;
    let offset = trapframe.get_arg(1) as i64;
    let whence = trapframe.get_arg(2) as i32;

    // Increment PC to avoid infinite loop if lseek fails
    trapframe.increment_pc_next(task);

    // Get handle from Linux fd
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

// // Create device file
// pub fn sys_mknod(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
//     let task = mytask().unwrap();
//     trapframe.increment_pc_next(task);
//     let name_ptr = task.vm_manager.translate_vaddr(trapframe.get_arg(0)).unwrap() as *const u8;
//     let name = get_path_str_v2(name_ptr).unwrap();
//     let path = to_absolute_path_v2(&task, &name).unwrap();

//     let major = trapframe.get_arg(1) as u32;
//     let minor = trapframe.get_arg(2) as u32;

//     match (major, minor) {
//         (1, 0) => {
//             // Create a console device
//             let console_dev = Some(DeviceManager::get_mut_manager().register_device(Arc::new(
//                 crate::abi::Linux::drivers::console::ConsoleDevice::new(0, "console")
//             )));
        
//             let vfs = task.vfs.as_mut().unwrap();
//             let _res = vfs.create_file(&path, FileType::CharDevice(
//                 DeviceFileInfo {
//                     device_id: console_dev.unwrap(),
//                     device_type: crate::device::DeviceType::Char,
//                 }
//             ));
//             // crate::println!("Created console device at {}", path);
//         },
//         _ => {},
//     }
//     0
// }


// pub fn sys_fstat(abi: &mut LinuxRiscv64Abi, trapframe: &mut crate::arch::Trapframe) -> usize {
//     let fd = trapframe.get_arg(0) as usize;

//     let task = mytask()
//         .expect("sys_fstat: No current task found");
//     trapframe.increment_pc_next(task); // Increment the program counter

//     let stat_ptr = task.vm_manager.translate_vaddr(trapframe.get_arg(1) as usize)
//         .expect("sys_fstat: Failed to translate stat pointer") as *mut Stat;
    
//     // Get handle from Linux fd
//     let handle = match abi.get_handle(fd) {
//         Some(h) => h,
//         None => return usize::MAX, // Invalid file descriptor
//     };
    
//     let kernel_obj = match task.handle_table.get(handle) {
//         Some(obj) => obj,
//         None => return usize::MAX, // Return -1 on error
//     };

//     let file = match kernel_obj.as_file() {
//         Some(file) => file,
//         None => return usize::MAX, // Not a file object
//     };

//     let metadata = file.metadata()
//         .expect("sys_fstat: Failed to get file metadata");

//     if stat_ptr.is_null() {
//         return usize::MAX; // Return -1 if stat pointer is null
//     }
    
//     let stat = unsafe { &mut *stat_ptr };

//     *stat = Stat {
//         dev: 0,
//         ino: metadata.file_id as u32,
//         file_type: match metadata.file_type {
//             FileType::Directory => 1, // T_DIR
//             FileType::RegularFile => 2,      // T_FILE
//             FileType::CharDevice(_) => 3, // T_DEVICE
//             FileType::BlockDevice(_) => 3, // T_DEVICE
//             _ => 0, // Unknown type
//         },
//         nlink: 1,
//         size: metadata.size as u64,
//     };

//     0
// }

/// Linux sys_newfstatat implementation for Scarlet VFS v2
///
/// Gets file status relative to a directory file descriptor (dirfd) and path.
/// If dirfd == AT_FDCWD, uses the current working directory as the base.
/// Otherwise, resolves the base directory from the file descriptor.
/// Uses VfsManager::resolve_path_from for safe and efficient path resolution.
///
/// Arguments:
/// - abi: LinuxRiscv64Abi context
/// - trapframe: Trapframe containing syscall arguments
///
/// Returns:
/// - 0 on success
/// - usize::MAX (Linux -1) on error
pub fn sys_newfstatat(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let dirfd = trapframe.get_arg(0) as i32;
    let path_ptr = task.vm_manager.translate_vaddr(trapframe.get_arg(1)).unwrap() as *const u8;
    let stat_ptr = task.vm_manager.translate_vaddr(trapframe.get_arg(2)).unwrap() as *mut u8;
    let flags = trapframe.get_arg(3) as i32;

    // Increment PC to avoid infinite loop if fstatat fails
    trapframe.increment_pc_next(task);

    // Parse path from user space
    let path_str = match cstring_to_string(path_ptr, MAX_PATH_LENGTH) {
        Ok((path, _)) => path,
        Err(_) => return usize::MAX, // Invalid UTF-8
    };

    let vfs = task.vfs.as_ref().unwrap();

    // Determine base directory (entry and mount) for path resolution
    use crate::fs::vfs_v2::core::VfsFileObject;

    const AT_FDCWD: i32 = -100;
    const AT_SYMLINK_NOFOLLOW: i32 = 0x100;
    
    // TODO: Handle AT_SYMLINK_NOFOLLOW flag properly
    // For now, we always follow symbolic links
    let _follow_symlinks = (flags & AT_SYMLINK_NOFOLLOW) == 0;
    
    let (base_entry, base_mount) = if dirfd == AT_FDCWD {
        // Use current working directory as base
        vfs.get_cwd().unwrap_or_else(|| {
            let root_mount = vfs.mount_tree.root_mount.read().clone();
            (root_mount.root.clone(), root_mount)
        })
    } else {
        // Use directory file descriptor as base
        let handle = match abi.get_handle(dirfd as usize) {
            Some(h) => h,
            None => return usize::MAX,
        };
        let kernel_obj = match task.handle_table.get(handle) {
            Some(obj) => obj,
            None => return usize::MAX,
        };
        let file_obj = match kernel_obj.as_file() {
            Some(f) => f,
            None => return usize::MAX,
        };
        let vfs_file_obj = file_obj.as_any().downcast_ref::<VfsFileObject>().ok_or(()).unwrap();
        (vfs_file_obj.get_vfs_entry().clone(), vfs_file_obj.get_mount_point().clone())
    };

    // Resolve the path from the base directory
    match vfs.resolve_path_from(&base_entry, &base_mount, &path_str) {
        Ok((entry, _mount_point)) => {
            // Get metadata from the resolved VfsEntry
            let node = entry.node();
            match node.metadata() {
                Ok(metadata) => {
                    if stat_ptr.is_null() {
                        return usize::MAX; // Return -1 if stat pointer is null
                    }
                    
                    let stat = unsafe { &mut *(stat_ptr as *mut LinuxStat) };
                    *stat = LinuxStat::from_metadata(&metadata);
                    0 // Success
                },
                Err(_) => usize::MAX, // Error getting metadata
            }
        },
        Err(_) => usize::MAX, // Error resolving path
    }
}

pub fn sys_mkdir(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
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

pub fn sys_unlink(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
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

pub fn sys_link(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
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
            
            // Map VFS errors to appropriate errno values for Linux
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

/// VFS v2 helper function for path absolutization using VfsManager
fn to_absolute_path_v2(task: &crate::task::Task, path: &str) -> Result<String, ()> {
    if path.starts_with('/') {
        Ok(path.to_string())
    } else {
        let vfs = task.vfs.as_ref().ok_or(())?;
        Ok(vfs.resolve_path_to_absolute(path))
    }
}

/// Helper function to replace the missing get_path_str function
/// TODO: This should be moved to a shared helper when VFS v2 provides public API
fn get_path_str_v2(ptr: *const u8) -> Result<String, ()> {
    const MAX_PATH_LENGTH: usize = 128;
    cstring_to_string(ptr, MAX_PATH_LENGTH).map(|(s, _)| s).map_err(|_| ())
}

/// Linux ioctl system call implementation
/// 
/// This system call performs device-specific control operations on file descriptors,
/// similar to the POSIX ioctl system call. It acts as a bridge between Linux ABI
/// and Scarlet's native HandleControl functionality.
/// 
/// # Arguments
/// - fd: File descriptor
/// - request: Control operation command
/// - arg: Argument for the control operation (often a pointer)
/// 
/// # Returns
/// - 0 or positive value on success
/// - usize::MAX on error (-1 in Linux)
pub fn sys_ioctl(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let fd = trapframe.get_arg(0) as usize;
    let request = trapframe.get_arg(1) as u32;
    let arg = trapframe.get_arg(2);

    // Increment PC to avoid infinite loop
    trapframe.increment_pc_next(task);

    // Get handle from Linux fd
    let handle = match abi.get_handle(fd) {
        Some(h) => h,
        None => return usize::MAX, // Invalid file descriptor
    };

    // Get the kernel object from the handle table
    let kernel_object = match task.handle_table.get(handle) {
        Some(obj) => obj,
        None => return usize::MAX, // Invalid handle
    };

    // Perform the control operation using the ControlOps capability
    let result = match kernel_object.as_control() {
        Some(control_ops) => {

            control_ops.control(request, arg)
        }
        None => {
            // Fallback: if object doesn't support control operations,
            // return ENOTTY (inappropriate ioctl for device)
            Err("Inappropriate ioctl for device") 
        }
    };

    // Convert result to Linux ioctl semantics
    match result {
        Ok(value) => {
            // Linux ioctl returns non-negative values on success
            if value >= 0 {
                value as usize
            } else {
                usize::MAX // Negative values are treated as errors
            }
        }
        Err(_) => usize::MAX, // Return -1 on error
    }
}

pub fn sys_execve(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    
    // Increment PC to avoid infinite loop if execve fails
    trapframe.increment_pc_next(task);
    
    // Get arguments from trapframe
    let path_ptr = trapframe.get_arg(0);
    let argv_ptr = trapframe.get_arg(1);
    let envp_ptr = trapframe.get_arg(2);
    
    // Parse path
    let path_str = match parse_c_string_from_userspace(task, path_ptr, MAX_PATH_LENGTH) {
        Ok(path) => match to_absolute_path_v2(&task, &path) {
            Ok(abs_path) => abs_path,
            Err(_) => return usize::MAX, // Path error
        },
        Err(_) => return usize::MAX, // Path parsing error
    };
    
    // Parse argv
    let argv_strings = match parse_string_array_from_userspace(task, argv_ptr, MAX_ARG_COUNT, MAX_PATH_LENGTH) {
        Ok(args) => args,
        Err(_) => return usize::MAX, // argv parsing error
    };
    
    // Parse envp (optional)
    let envp_strings = match parse_string_array_from_userspace(task, envp_ptr, MAX_ARG_COUNT, MAX_PATH_LENGTH) {
        Ok(envs) => envs,
        Err(_) => return usize::MAX, // envp parsing error
    };
    
    // Convert Vec<String> to Vec<&str> for TransparentExecutor
    let argv_refs: Vec<&str> = argv_strings.iter().map(|s| s.as_str()).collect();
    let envp_refs: Vec<&str> = envp_strings.iter().map(|s| s.as_str()).collect();
    
    // Use TransparentExecutor for cross-ABI execution
    match TransparentExecutor::execute_binary(&path_str, &argv_refs, &envp_refs, task, trapframe, false) {
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

/// Linux iovec structure for vectored I/O operations
/// This structure matches the Linux kernel's definition for struct iovec
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct IoVec {
    /// Base address of the buffer
    pub iov_base: *mut u8,
    /// Length of the buffer
    pub iov_len: usize,
}

/// Linux sys_fcntl implementation for Scarlet VFS v2
/// Currently provides basic logging of commands to understand usage patterns
/// 
/// This is a minimal implementation that logs the fcntl commands being used
/// to help understand what functionality needs to be implemented.
pub fn sys_fcntl(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let fd = trapframe.get_arg(0) as usize;
    let cmd = trapframe.get_arg(1) as u32;
    let arg = trapframe.get_arg(2);

    // Increment PC to avoid infinite loop
    trapframe.increment_pc_next(task);

    // Log the fcntl command to understand usage patterns
    match cmd {
        F_DUPFD => {
            crate::println!("[sys_fcntl] F_DUPFD: fd={}, arg={} - NOT IMPLEMENTED", fd, arg);
            // TODO: Implement F_DUPFD
        },
        F_GETFD => {
            // Get file descriptor flags (IMPLEMENTED)
            if let Some(_handle) = abi.get_handle(fd) {
                if let Some(flags) = abi.get_fd_flags(fd) {
                    return flags as usize; // Return the flags
                } else {
                    return usize::MAX; // Invalid file descriptor
                }
            } else {
                return usize::MAX; // Invalid file descriptor
            }
        },
        F_SETFD => {
            // Set file descriptor flags (IMPLEMENTED)
            if let Some(_handle) = abi.get_handle(fd) {
                match abi.set_fd_flags(fd, arg as u32) {
                    Ok(()) => return 0, // Success
                    Err(_) => return usize::MAX, // Error
                }
            } else {
                return usize::MAX; // Invalid file descriptor
            }
        },
        F_GETFL => {
            crate::println!("[sys_fcntl] F_GETFL: fd={} - NOT IMPLEMENTED", fd);
            // TODO: Implement F_GETFL - return file status flags
        },
        F_SETFL => {
            crate::println!("[sys_fcntl] F_SETFL: fd={}, flags={:#x} - NOT IMPLEMENTED", fd, arg);
            // TODO: Implement F_SETFL - set file status flags
        },
        F_GETLK => {
            crate::println!("[sys_fcntl] F_GETLK: fd={}, lock_ptr={:#x} - NOT IMPLEMENTED", fd, arg);
            // TODO: Implement file locking
        },
        F_SETLK => {
            crate::println!("[sys_fcntl] F_SETLK: fd={}, lock_ptr={:#x} - NOT IMPLEMENTED", fd, arg);
            // TODO: Implement file locking
        },
        F_SETLKW => {
            crate::println!("[sys_fcntl] F_SETLKW: fd={}, lock_ptr={:#x} - NOT IMPLEMENTED", fd, arg);
            // TODO: Implement file locking
        },
        F_SETOWN => {
            crate::println!("[sys_fcntl] F_SETOWN: fd={}, owner={} - NOT IMPLEMENTED", fd, arg);
            // TODO: Implement F_SETOWN
        },
        F_GETOWN => {
            crate::println!("[sys_fcntl] F_GETOWN: fd={} - NOT IMPLEMENTED", fd);
            // TODO: Implement F_GETOWN
        },
        F_SETSIG => {
            crate::println!("[sys_fcntl] F_SETSIG: fd={}, sig={} - NOT IMPLEMENTED", fd, arg);
            // TODO: Implement F_SETSIG
        },
        F_GETSIG => {
            crate::println!("[sys_fcntl] F_GETSIG: fd={} - NOT IMPLEMENTED", fd);
            // TODO: Implement F_GETSIG
        },
        F_SETLEASE => {
            crate::println!("[sys_fcntl] F_SETLEASE: fd={}, lease_type={} - NOT IMPLEMENTED", fd, arg);
            // TODO: Implement F_SETLEASE
        },
        F_GETLEASE => {
            crate::println!("[sys_fcntl] F_GETLEASE: fd={} - NOT IMPLEMENTED", fd);
            // TODO: Implement F_GETLEASE
        },
        F_NOTIFY => {
            crate::println!("[sys_fcntl] F_NOTIFY: fd={}, events={:#x} - NOT IMPLEMENTED", fd, arg);
            // TODO: Implement F_NOTIFY
        },
        F_DUPFD_CLOEXEC => {
            crate::println!("[sys_fcntl] F_DUPFD_CLOEXEC: fd={}, arg={} - NOT IMPLEMENTED", fd, arg);
            // TODO: Implement F_DUPFD_CLOEXEC
        },
        _ => {
            crate::println!("[sys_fcntl] UNKNOWN_CMD: fd={}, cmd={}, arg={:#x} - NOT IMPLEMENTED", fd, cmd, arg);
        }
    }

    // All unimplemented commands return ENOSYS (already logged above)
    usize::MAX // Return -1 (ENOSYS - Function not implemented)
}