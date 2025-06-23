use crate::utils::str_to_cstr_bytes;
use crate::boxed::Box;
use crate::syscall::{syscall2, syscall3, syscall5, Syscall};
use crate::string::String;

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
    let path_bytes = str_to_cstr_bytes(path).unwrap();
    let path_boxed_slice = path_bytes.into_boxed_slice();
    let path_len = path_boxed_slice.len();
    let path_ptr = Box::into_raw(path_boxed_slice) as *const u8 as usize;
    let res = syscall2(Syscall::Open, path_ptr, flags);
    // Properly free the allocated memory with correct size information
    let _ = unsafe { Box::<[u8]>::from_raw(core::slice::from_raw_parts_mut(path_ptr as *mut u8, path_len)) };
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

/// Create a new file
/// 
/// This function creates a new file at the specified path with the given mode.
/// 
/// # Arguments
/// * `path` - Path to the file to create
/// * `mode` - Permissions for the new file (e.g., 0o644)
/// 
/// # Return Value
/// * `0` on success, `-1` on error
/// 
pub fn mkfile(path: &str, mode: u32) -> i32 {
    let path_boxed = str_to_cstr_bytes(path).unwrap().into_boxed_slice();
    let path_ptr = path_boxed.as_ptr() as usize;
    let res = syscall2(Syscall::Mkfile, path_ptr, mode as usize);
    // The allocated memory will be safely dropped when `path_boxed` goes out of scope
    res as i32
}

/// Create a directory
/// 
/// This function creates a new directory at the specified path with the given mode.
/// 
/// # Arguments
/// * `path` - Path to the directory to create
/// * `mode` - Permissions for the new directory (e.g., 0o755)
/// 
/// # Return Value
/// * `0` on success, `-1` on error
/// 
pub fn mkdir(path: &str, mode: u32) -> i32 {
    let path_bytes = str_to_cstr_bytes(path).unwrap();
    let path_boxed_slice = path_bytes.into_boxed_slice();
    let path_len = path_boxed_slice.len();
    let path_ptr = Box::into_raw(path_boxed_slice) as *const u8 as usize;
    let res = syscall2(Syscall::Mkdir, path_ptr, mode as usize);
    // Free the allocated memory
    let _ = unsafe { Box::<[u8]>::from_raw(core::slice::from_raw_parts_mut(path_ptr as *mut u8, path_len)) };
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
    let source_bytes = str_to_cstr_bytes(source).unwrap();
    let source_boxed_slice = source_bytes.into_boxed_slice();
    let source_len = source_boxed_slice.len();
    let source_ptr = Box::into_raw(source_boxed_slice) as *const u8 as usize;
    
    let target_bytes = str_to_cstr_bytes(target).unwrap();
    let target_boxed_slice = target_bytes.into_boxed_slice();
    let target_len = target_boxed_slice.len();
    let target_ptr = Box::into_raw(target_boxed_slice) as *const u8 as usize;
    
    let fstype_bytes = str_to_cstr_bytes(fstype).unwrap();
    let fstype_boxed_slice = fstype_bytes.into_boxed_slice();
    let fstype_len = fstype_boxed_slice.len();
    let fstype_ptr = Box::into_raw(fstype_boxed_slice) as *const u8 as usize;
    
    let (data_ptr, data_len) = if let Some(data_str) = data {
        let data_bytes = str_to_cstr_bytes(data_str).unwrap();
        let data_boxed_slice = data_bytes.into_boxed_slice();
        let data_len = data_boxed_slice.len();
        let data_ptr = Box::into_raw(data_boxed_slice) as *const u8 as usize;
        (data_ptr, data_len)
    } else {
        (0, 0) // null pointer
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
    let _ = unsafe { Box::<[u8]>::from_raw(core::slice::from_raw_parts_mut(source_ptr as *mut u8, source_len)) };
    let _ = unsafe { Box::<[u8]>::from_raw(core::slice::from_raw_parts_mut(target_ptr as *mut u8, target_len)) };
    let _ = unsafe { Box::<[u8]>::from_raw(core::slice::from_raw_parts_mut(fstype_ptr as *mut u8, fstype_len)) };
    if data_ptr != 0 {
        let _ = unsafe { Box::<[u8]>::from_raw(core::slice::from_raw_parts_mut(data_ptr as *mut u8, data_len)) };
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
    let target_bytes = str_to_cstr_bytes(target).unwrap();
    let target_boxed_slice = target_bytes.into_boxed_slice();
    let target_len = target_boxed_slice.len();
    let target_ptr = Box::into_raw(target_boxed_slice) as *const u8 as usize;
    
    let res = syscall2(
        Syscall::Umount,
        target_ptr,
        flags as usize
    );
    
    // Free allocated memory
    let _ = unsafe { Box::<[u8]>::from_raw(core::slice::from_raw_parts_mut(target_ptr as *mut u8, target_len)) };
    
    res as i32
}

/// Change the root filesystem
/// 
/// This function performs a pivot_root operation, which atomically moves the
/// root filesystem to a new location and makes the new filesystem the root.
/// This is commonly used during system initialization to switch from an
/// initramfs to the real root filesystem.
/// 
/// # Arguments
/// 
/// * `new_root` - Path to the directory that will become the new root
/// * `old_root` - Path where the old root will be moved (relative to new_root)
/// 
/// # Returns
/// 
/// * `0` on success, `-1` on error
/// 
/// # Requirements
/// 
/// * The calling process must have its own VFS namespace (isolated filesystem)
/// * The new_root must be a mount point of a different filesystem than the current root
/// * The old_root must be a valid path under the new root filesystem
/// 
/// # Examples
/// 
/// ```rust
/// use crate::fs::pivot_root;
/// 
/// // Switch from initramfs to real root filesystem
/// // 1. Mount the real root filesystem
/// mount("/dev/sda1", "/mnt/newroot", "ext4", 0, None);
/// 
/// // 2. Create directory for old root
/// // (This would typically be done via mkdir syscall)
/// 
/// // 3. Pivot to new root
/// let result = pivot_root("/mnt/newroot", "/mnt/newroot/old_root");
/// if result == 0 {
///     println!("Successfully pivoted to new root");
///     // At this point:
///     // - "/" points to what was previously "/mnt/newroot"
///     // - "/old_root" contains the old root filesystem (initramfs)
/// } else {
///     println!("Failed to pivot root");
/// }
/// ```
/// 
/// # Container Usage
/// 
/// ```rust
/// // In a container setup
/// mount("/host/container/root", "/mnt/container", "bind", MS_BIND, None);
/// let result = pivot_root("/mnt/container", "/mnt/container/host");
/// if result == 0 {
///     // Container now has isolated root filesystem
///     // Host filesystem accessible at /host
/// }
/// ```
pub fn pivot_root(new_root: &str, old_root: &str) -> i32 {
    // Convert the new_root and old_root strings to C-style strings
    let new_root_bytes = match str_to_cstr_bytes(new_root) {
        Ok(bytes) => bytes,
        Err(_) => return -1, // Return -1 if conversion fails
    };
    let old_root_bytes = match str_to_cstr_bytes(old_root) {
        Ok(bytes) => bytes,
        Err(_) => return -1, // Return -1 if conversion fails
    };
    
    let new_root_boxed_slice = new_root_bytes.into_boxed_slice();
    let new_root_len = new_root_boxed_slice.len();
    let new_root_ptr = Box::into_raw(new_root_boxed_slice) as *const u8 as usize;
    
    let old_root_boxed_slice = old_root_bytes.into_boxed_slice();
    let old_root_len = old_root_boxed_slice.len();
    let old_root_ptr = Box::into_raw(old_root_boxed_slice) as *const u8 as usize;
    
    let res = syscall2(
        Syscall::PivotRoot,
        new_root_ptr,
        old_root_ptr
    );
    
    // Free allocated memory
    let _ = unsafe { Box::<[u8]>::from_raw(core::slice::from_raw_parts_mut(new_root_ptr as *mut u8, new_root_len)) };
    let _ = unsafe { Box::<[u8]>::from_raw(core::slice::from_raw_parts_mut(old_root_ptr as *mut u8, old_root_len)) };
    
    res as i32
}

/// Read directory entries.
/// 
/// This function reads directory entries one at a time from an opened directory.
/// Each call returns the next directory entry or None if end of directory is reached.
/// 
/// # Arguments
/// * `fd` - File descriptor of an opened directory
/// 
/// # Return Value
/// - `Ok(Some(entry))` - Successfully read a directory entry
/// - `Ok(None)` - End of directory reached (EOF)
/// - `Err(errno)` - Error occurred (errno value from kernel)
/// 
/// # Example
/// ```rust
/// let dir_fd = open("/tmp", 0);
/// if dir_fd >= 0 {
///     loop {
///         match readdir(dir_fd) {
///             Ok(Some(entry)) => {
///                 if let Ok(name) = entry.name_str() {
///                     println!("Found: {} (type: {})", name, entry.file_type);
///                 }
///             }
///             Ok(None) => break, // End of directory
///             Err(errno) => {
///                 println!("Error reading directory: {}", errno);
///                 break;
///             }
///         }
///     }
///     close(dir_fd);
/// }
/// ```
pub fn readdir(fd: i32) -> Result<Option<DirectoryEntry>, i32> {
    let mut buf = [0u8; core::mem::size_of::<DirectoryEntryRaw>()];
    let bytes_read = read(fd, &mut buf);
    
    if bytes_read < 0 {
        return Err(bytes_read); // Return error code
    }
    
    if bytes_read == 0 {
        return Ok(None); // EOF - no more entries
    }
    
    // Parse the directory entry
    if let Some(entry) = parse_dir_entry(&buf[..bytes_read as usize]) {
        Ok(Some(DirectoryEntry::from_raw(entry)))
    } else {
        Err(-1) // Parse error
    }
}

/// Raw Directory entry structure (must match kernel definition)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct DirectoryEntryRaw {
    /// Unique file identifier
    pub file_id: u64,
    /// File size in bytes
    pub size: u64,
    /// File type as a byte value
    pub file_type: u8,
    /// Length of the file name
    pub name_len: u8,
    /// Reserved bytes for alignment
    pub _reserved: [u8; 6],
    /// File name (null-terminated, max 255 characters)
    pub name: [u8; 256],
}

impl DirectoryEntryRaw {
    /// Get the name as a string
    pub fn name_str(&self) -> Result<&str, core::str::Utf8Error> {
        let name_bytes = &self.name[..self.name_len as usize];
        core::str::from_utf8(name_bytes)
    }
    
    /// Get the name as an owned String
    pub fn name_string(&self) -> Result<crate::string::String, core::str::Utf8Error> {
        let name_str = self.name_str()?;
        let mut owned_name = crate::string::String::new();
        for c in name_str.chars() {
            owned_name.push(c);
        }
        Ok(owned_name)
    }
    
    /// Check if this entry is a directory
    pub fn is_directory(&self) -> bool {
        self.file_type == 1 // FileType::Directory as u8
    }
    
    /// Check if this entry is a regular file
    pub fn is_file(&self) -> bool {
        self.file_type == 0 // FileType::RegularFile as u8
    }
    
    /// Check if this entry is a symbolic link
    pub fn is_symlink(&self) -> bool {
        self.file_type == 2 // FileType::SymbolicLink as u8
    }
    
    /// Get file type as a human-readable string
    pub fn file_type_str(&self) -> &'static str {
        match self.file_type {
            0 => "file",
            1 => "directory",
            2 => "symlink",
            3 => "device",
            4 => "pipe",
            5 => "socket",
            _ => "unknown",
        }
    }
}

/// Directory entry structure for user space
/// This structure is a higher-level representation of a directory entry
/// that can be used in user space

#[derive(Debug, Clone)]
pub struct DirectoryEntry {
    /// Unique file identifier
    pub file_id: u64,
    /// File size in bytes
    pub size: u64,
    /// File type as a byte value
    pub file_type: u8,
    /// File name
    pub name: String,
}

impl DirectoryEntry {
    /// Create a new DirectoryEntry from raw data
    pub fn from_raw(entry: DirectoryEntryRaw) -> Self {
        Self {
            file_id: entry.file_id,
            size: entry.size,
            file_type: entry.file_type,
            name: entry.name_string().unwrap_or_else(|_| String::new()),
        }
    }
    
    /// Get the name as a string slice
    pub fn name_str(&self) -> &str {
        &self.name
    }
    
    /// Check if this entry is a directory
    pub fn is_directory(&self) -> bool {
        self.file_type == 1 // FileType::Directory as u8
    }
    
    /// Check if this entry is a regular file
    pub fn is_file(&self) -> bool {
        self.file_type == 0 // FileType::RegularFile as u8
    }
}

/// Helper function to parse directory entries from readdir buffer (backward compatibility).
/// 
/// This function is kept for backward compatibility. Consider using the new
/// `readdir()` function instead, which handles parsing automatically.
/// 
/// # Arguments
/// * `buf` - Buffer containing directory entry from readdir
/// * `bytes_read` - Number of bytes actually read
/// 
/// # Return Value
/// Option containing the parsed directory entry data as a tuple
/// 
pub fn parse_dir_entry_safe(buf: &[u8], bytes_read: usize) -> Option<(crate::string::String, u8, u64, u64)> {
    if bytes_read == 0 {
        return None; // EOF
    }
    
    if let Some(entry) = parse_dir_entry(&buf[..bytes_read]) {
        if let Ok(owned_name) = entry.name_string() {
            return Some((
                owned_name,
                entry.file_type,
                entry.file_id,
                entry.size
            ));
        }
    }
    
    None
}

/// Parse a single directory entry from buffer (low-level function)
pub fn parse_dir_entry(buf: &[u8]) -> Option<DirectoryEntryRaw> {
    if buf.len() < core::mem::size_of::<DirectoryEntryRaw>() {
        return None;
    }
    
    unsafe {
        Some(*(buf.as_ptr() as *const DirectoryEntryRaw))
    }
}

/// Example: List all files in a directory
/// 
/// This is a demonstration of how to use the new readdir API to collect
/// all entries in a directory.
/// 
/// # Arguments
/// * `path` - Path to the directory to list
/// 
/// # Returns
/// * `Ok(entries)` - Vector of directory entries on success
/// * `Err(errno)` - Error code on failure
/// 
pub fn list_directory(path: &str) -> Result<crate::vec::Vec<DirectoryEntry>, i32> {
    use crate::vec::Vec;
    
    let dir_fd = open(path, 0);
    if dir_fd < 0 {
        return Err(dir_fd);
    }
    
    let mut entries = Vec::new();
    
    loop {
        match readdir(dir_fd) {
            Ok(Some(entry)) => {
                entries.push(entry);
            }
            Ok(None) => break, // End of directory
            Err(errno) => {
                close(dir_fd);
                return Err(errno);
            }
        }
    }
    
    close(dir_fd);
    Ok(entries)
}

/// Example: Count files and directories
/// 
/// # Arguments
/// * `path` - Path to the directory to analyze
/// 
/// # Returns
/// * `Ok((file_count, dir_count))` on success
/// * `Err(errno)` on error
/// 
pub fn count_directory_entries(path: &str) -> Result<(usize, usize), i32> {
    let dir_fd = open(path, 0);
    if dir_fd < 0 {
        return Err(dir_fd);
    }
    
    let mut file_count = 0;
    let mut dir_count = 0;
    
    loop {
        match readdir(dir_fd) {
            Ok(Some(entry)) => {
                if entry.is_file() {
                    file_count += 1;
                } else if entry.is_directory() {
                    dir_count += 1;
                }
            }
            Ok(None) => break,
            Err(errno) => {
                close(dir_fd);
                return Err(errno);
            }
        }
    }
    
    close(dir_fd);
    Ok((file_count, dir_count))
}