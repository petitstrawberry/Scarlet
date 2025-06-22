use crate::utils::str_to_cstr_bytes;
use crate::boxed::Box;
use crate::syscall::{syscall2, syscall3, syscall5, Syscall};

// Mount flags (similar to Linux mount flags)
pub const MS_RDONLY: u32 = 1;        // Mount read-only
pub const MS_NOSUID: u32 = 2;        // Ignore suid and sgid bits
pub const MS_NODEV: u32 = 4;         // Disallow access to device special files
pub const MS_NOEXEC: u32 = 8;        // Disallow program execution
pub const MS_SYNCHRONOUS: u32 = 16;  // Writes are synced at once
pub const MS_BIND: u32 = 4096;       // Create bind mount
pub const MS_MOVE: u32 = 8192;       // Move mount point
pub const MS_REC: u32 = 16384;       // Recursive bind mount
pub const MS_SILENT: u32 = 32768;    // Suppress kernel messages
pub const MS_REMOUNT: u32 = 32;      // Remount filesystem

/// Open a file.
/// 
/// # Arguments
/// * `path` - Path to the file
/// * `flags` - Flags for opening the file
/// 
/// # Return Value
/// - On success: file descriptor
/// - On error: -1
/// 
pub fn open(path: &str, flags: usize) -> i32 {
    let path_ptr = Box::into_raw(str_to_cstr_bytes(path).unwrap().into_boxed_slice()) as *const u8 as usize;
    let res = syscall2(Syscall::Open, path_ptr, flags);
    let _ = unsafe { Box::from_raw(path_ptr as *mut u8) }; // Free the allocated memory
    // Return the result of the syscall
    res as i32
}

/// Close a file.
/// 
/// # Arguments
/// * `fd` - File descriptor
/// 
/// # Return Value
/// - On success: 0
/// - On error: -1
/// 
pub fn close(fd: i32) -> i32 {
    let res = syscall2(Syscall::Close, fd as usize, 0);
    // Return the result of the syscall
    res as i32
}

/// Read from a file.
/// 
/// # Arguments
/// * `fd` - File descriptor
/// * `buf` - Buffer to read into
///
/// # Return Value
/// - On success: number of bytes read
/// - On error: -1
/// 
pub fn read(fd: i32, buf: &mut [u8]) -> i32 {
    let res = syscall3(Syscall::Read, fd as usize, buf.as_mut_ptr() as usize, buf.len());
    // Return the result of the syscall
    res as i32
}

/// Write to a file.
/// 
/// # Arguments
/// * `fd` - File descriptor
/// * `buf` - Buffer to write from
/// 
/// # Return Value
/// - On success: number of bytes written
/// - On error: -1
/// 
pub fn write(fd: i32, buf: &[u8]) -> i32 {
    let res = syscall3(Syscall::Write, fd as usize, buf.as_ptr() as usize, buf.len());
    // Return the result of the syscall
    res as i32
}

/// Seek to a position in a file.
/// 
/// # Arguments
/// * `fd` - File descriptor
/// * `offset` - Offset to seek to
/// * `whence` - Whence for the seek operation
/// 
/// # Return Value
/// - On success: new position in the file
/// - On error: -1
/// 
pub fn lseek(fd: i32, offset: i64, whence: u32) -> i32 {
    let res = syscall3(Syscall::Lseek, fd as usize, offset as usize, whence as usize);
    // Return the result of the syscall
    res as i32
}

/// Mount a filesystem
/// 
/// This function provides a POSIX-like mount interface that internally uses
/// Scarlet's powerful VFS system. The mount type is automatically determined
/// based on the source and filesystem type.
/// 
/// # Arguments
/// 
/// * `source` - Device path, memory area, or filesystem source
/// * `target` - Mount point path
/// * `fstype` - Filesystem type: "ext4", "tmpfs", "cpiofs", "bind", "overlay", etc.
/// * `flags` - Mount flags (MS_RDONLY, MS_BIND, etc.)
/// * `data` - Mount-specific data (optional)
/// 
/// # Returns
/// 
/// * `0` on success, `-1` on error
/// 
/// # Mount Types Supported
/// 
/// * **Block devices**: `mount("/dev/sda1", "/mnt", "ext4", 0, None)`
/// * **Tmpfs**: `mount("tmpfs", "/tmp", "tmpfs", 0, Some("size=10M"))`
/// * **Bind mounts**: `mount("/source", "/target", "bind", MS_BIND, None)`
/// * **Overlay**: `mount("overlay", "/overlay", "overlay", 0, Some("lowerdir=/lower,upperdir=/upper"))`
/// * **Memory FS**: `mount("initramfs", "/", "cpiofs", 0, Some("0x80000000,0x81000000"))`
/// 
/// # Example
/// 
/// ```rust
/// use crate::fs::{mount, MS_BIND, MS_RDONLY};
/// 
/// // Mount a bind mount
/// let result = mount("/source", "/target", "bind", MS_BIND, None);
/// if result == 0 {
///     println!("Bind mount successful");
/// }
/// 
/// // Create tmpfs with size limit
/// let result = mount("tmpfs", "/tmp", "tmpfs", 0, Some("size=10M"));
/// if result == 0 {
///     println!("Tmpfs mounted successfully");
/// }
/// 
/// // Create overlay mount
/// let result = mount(
///     "overlay", 
///     "/overlay", 
///     "overlay", 
///     0, 
///     Some("lowerdir=/lower1:/lower2,upperdir=/upper")
/// );
/// ```
pub fn mount(source: &str, target: &str, fstype: &str, flags: u32, data: Option<&str>) -> i32 {
    let source_ptr = Box::into_raw(str_to_cstr_bytes(source).unwrap().into_boxed_slice()) as *const u8 as usize;
    let target_ptr = Box::into_raw(str_to_cstr_bytes(target).unwrap().into_boxed_slice()) as *const u8 as usize;
    let fstype_ptr = Box::into_raw(str_to_cstr_bytes(fstype).unwrap().into_boxed_slice()) as *const u8 as usize;
    
    let data_ptr = if let Some(data_str) = data {
        Box::into_raw(str_to_cstr_bytes(data_str).unwrap().into_boxed_slice()) as *const u8 as usize
    } else {
        0 // null pointer
    };
    
    let res = syscall5(
        Syscall::Mount,
        source_ptr,
        target_ptr,
        fstype_ptr,
        flags as usize,
        data_ptr
    );
    
    // Free allocated memory
    let _ = unsafe { Box::from_raw(source_ptr as *mut u8) };
    let _ = unsafe { Box::from_raw(target_ptr as *mut u8) };
    let _ = unsafe { Box::from_raw(fstype_ptr as *mut u8) };
    if data_ptr != 0 {
        let _ = unsafe { Box::from_raw(data_ptr as *mut u8) };
    }
    
    res as i32
}

/// Unmount a filesystem
/// 
/// This function unmounts a filesystem from the specified mount point.
/// All files and directories under the mount point will become inaccessible
/// after the unmount operation completes.
/// 
/// # Arguments
/// 
/// * `target` - Mount point path to unmount
/// * `flags` - Unmount flags (for future extension, currently unused)
/// 
/// # Returns
/// 
/// * `0` on success, `-1` on error
/// 
/// # Examples
/// 
/// ```rust
/// use crate::fs::umount;
/// 
/// // Unmount a filesystem
/// let result = umount("/mnt", 0);
/// if result == 0 {
///     println!("Filesystem unmounted successfully");
/// } else {
///     println!("Failed to unmount filesystem");
/// }
/// 
/// // Unmount a bind mount
/// let result = umount("/target", 0);
/// if result == 0 {
///     println!("Bind mount unmounted successfully");
/// }
/// ```
pub fn umount(target: &str, flags: u32) -> i32 {
    let target_ptr = Box::into_raw(str_to_cstr_bytes(target).unwrap().into_boxed_slice()) as *const u8 as usize;
    
    let res = syscall2(
        Syscall::Umount,
        target_ptr,
        flags as usize
    );
    
    // Free allocated memory
    let _ = unsafe { Box::from_raw(target_ptr as *mut u8) };
    
    res as i32
}