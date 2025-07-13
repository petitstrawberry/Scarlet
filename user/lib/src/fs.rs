//! File system abstraction for Scarlet Native API
//!
//! This module provides a Rust standard library-like file system interface
//! using the OpenOptions builder pattern and high-level convenience functions.
//!
//! ## Core Functions
//!
//! ### File Operations
//! - [`File::open`], [`File::create`]: Open and create files
//! - [`OpenOptions`]: Flexible file opening with various options
//!
//! ### Directory Operations  
//! - [`change_directory`]: Change current working directory
//! - [`File::read_dir`]: Read directory entries from an open directory
//! - [`list_directory`]: List all entries in a directory (convenience function)
//! - [`count_directory_entries`]: Count files and directories (example function)
//!
//! ### Directory Entry Parsing
//! - [`DirectoryEntry`]: High-level directory entry structure
//! - [`DirectoryEntryRaw`]: Low-level raw directory entry structure
//! - [`parse_dir_entry`]: Parse raw directory entry data
//! - [`parse_dir_entry_safe`]: Safe directory entry parsing (backward compatibility)
//!
//! ### Filesystem Operations
//! - [`mount`]: Mount filesystems with various options
//! - [`unmount`]: Unmount filesystems
//! - [`pivot_root`]: Change root filesystem (system initialization)

use crate::handle::Handle;
use crate::handle::capability::{SeekFrom as ScarletSeekFrom};
use crate::string::String;
use crate::io::{Error, ErrorKind, Seek, SeekFrom, Write, Read, Result};


/// Options and flags which can be used to configure how a file is opened
///
/// This builder exposes the ability to configure how a [`File`] is opened
/// and what operations are permitted on the open file. The [`File::open`]
/// and [`File::create`] methods are aliases for commonly used options
/// using this builder.
///
/// # Examples
///
/// Opening a file to read:
///
/// ```
/// use scarlet::fs::OpenOptions;
///
/// let file = OpenOptions::new()
///     .read(true)
///     .open("foo.txt")?;
/// ```
///
/// Opening a file for both reading and writing, creating it if it doesn't exist:
///
/// ```
/// use scarlet::fs::OpenOptions;
///
/// let file = OpenOptions::new()
///     .read(true)
///     .write(true)
///     .create(true)
///     .open("foo.txt")?;
/// ```
#[derive(Debug, Clone)]
pub struct OpenOptions {
    read: bool,
    write: bool,
    append: bool,
    truncate: bool,
    create: bool,
    create_new: bool,
}

impl OpenOptions {
    /// Creates a blank new set of options ready for configuration
    ///
    /// All options are initially set to `false`.
    pub fn new() -> Self {
        Self {
            read: false,
            write: false,
            append: false,
            truncate: false,
            create: false,
            create_new: false,
        }
    }
    
    /// Sets the option for read access
    ///
    /// This option, when true, will indicate that the file should be
    /// readable if opened.
    ///
    /// # Examples
    ///
    /// ```
    /// use scarlet::fs::OpenOptions;
    ///
    /// let file = OpenOptions::new().read(true).open("foo.txt");
    /// ```
    pub fn read(&mut self, read: bool) -> &mut Self {
        self.read = read;
        self
    }
    
    /// Sets the option for write access
    ///
    /// This option, when true, will indicate that the file should be
    /// writable if opened.
    ///
    /// If the file already exists, any write calls on it will overwrite
    /// its contents, without truncating it.
    ///
    /// # Examples
    ///
    /// ```
    /// use scarlet::fs::OpenOptions;
    ///
    /// let file = OpenOptions::new().write(true).open("foo.txt");
    /// ```
    pub fn write(&mut self, write: bool) -> &mut Self {
        self.write = write;
        self
    }
    
    /// Sets the option for the append mode
    ///
    /// This option, when true, means that writes will append to a file instead
    /// of overwriting previous contents.
    /// Note that setting `.write(true).append(true)` has the same effect as
    /// setting only `.append(true)`.
    ///
    /// For most filesystems, the operating system guarantees that all writes are
    /// atomic: no reads-in-progress will see a half-written file.
    ///
    /// ## Note
    ///
    /// This function doesn't create the file if it doesn't exist. Use the
    /// [`OpenOptions::create`] method to do so.
    ///
    /// # Examples
    ///
    /// ```
    /// use scarlet::fs::OpenOptions;
    ///
    /// let file = OpenOptions::new().append(true).open("foo.txt");
    /// ```
    pub fn append(&mut self, append: bool) -> &mut Self {
        self.append = append;
        self
    }
    
    /// Sets the option for truncating a previous file
    ///
    /// If a file is successfully opened with this option set it will truncate
    /// the file to 0 length if it already exists.
    ///
    /// The file must be opened with write access for truncate to work.
    ///
    /// # Examples
    ///
    /// ```
    /// use scarlet::fs::OpenOptions;
    ///
    /// let file = OpenOptions::new().write(true).truncate(true).open("foo.txt");
    /// ```
    pub fn truncate(&mut self, truncate: bool) -> &mut Self {
        self.truncate = truncate;
        self
    }
    
    /// Sets the option to create a new file, or open it if it already exists
    ///
    /// In order for the file to be created, [`OpenOptions::write`] or
    /// [`OpenOptions::append`] access must be used.
    ///
    /// # Examples
    ///
    /// ```
    /// use scarlet::fs::OpenOptions;
    ///
    /// let file = OpenOptions::new().write(true).create(true).open("foo.txt");
    /// ```
    pub fn create(&mut self, create: bool) -> &mut Self {
        self.create = create;
        self
    }
    
    /// Sets the option to create a new file, failing if it already exists
    ///
    /// No file is allowed to exist at the target location, also no (dangling) symlink.
    /// In this way, if the call succeeds, the file returned is guaranteed to be new.
    ///
    /// This option is useful because it is atomic. Otherwise between checking
    /// whether a file exists and creating a new one, the file may have been
    /// created by another process (a TOCTOU race condition / attack).
    ///
    /// If `.create_new(true)` is set, [`.create()`] and [`.truncate()`] are
    /// ignored.
    ///
    /// The file must be opened with write or append access in order to create
    /// a new file.
    ///
    /// [`.create()`]: OpenOptions::create
    /// [`.truncate()`]: OpenOptions::truncate
    ///
    /// # Examples
    ///
    /// ```
    /// use scarlet::fs::OpenOptions;
    ///
    /// let file = OpenOptions::new().write(true).create_new(true).open("foo.txt");
    /// ```
    pub fn create_new(&mut self, create_new: bool) -> &mut Self {
        self.create_new = create_new;
        self
    }
    
    /// Opens a file at `path` with the options specified by `self`
    ///
    /// # Errors
    ///
    /// This function will return an error under a number of different
    /// circumstances. Some of these error conditions are listed here, together
    /// with their [`ErrorKind`]. The mapping to [`ErrorKind`]s is not part of
    /// the compatibility contract of the function.
    ///
    /// * [`NotFound`]: The specified file does not exist and neither `create`
    ///   or `create_new` is set.
    /// * [`NotFound`]: One of the directory components of the file path does
    ///   not exist.
    /// * [`PermissionDenied`]: The user lacks permission to get the specified
    ///   access rights for the file.
    /// * [`PermissionDenied`]: The user lacks permission to open one of the
    ///   directory components of the specified path.
    /// * [`InvalidInput`]: Invalid combinations of open options (truncate
    ///   without write access, no access mode set, etc.).
    ///
    /// [`ErrorKind`]: Error
    /// [`InvalidInput`]: ErrorKind::InvalidInput
    /// [`NotFound`]: ErrorKind::NotFound
    /// [`PermissionDenied`]: ErrorKind::PermissionDenied
    ///
    /// # Examples
    ///
    /// ```
    /// use scarlet::fs::OpenOptions;
    ///
    /// let file = OpenOptions::new().read(true).open("foo.txt");
    /// ```
    pub fn open<P: AsRef<str>>(&self, path: P) -> Result<File> {
        use crate::syscall::{syscall2, Syscall};
        
        // If we need to create the file, use VfsCreateFile first
        if self.create || self.create_new {
            // Check if we have write access
            if !self.write && !self.append {
                return Err(Error::new(ErrorKind::InvalidInput, "Cannot create file without write access"));
            }
            
            // For create_new, we should check if file exists first
            // For now, just attempt to create and handle errors
            let result = syscall2(
                Syscall::VfsCreateFile,
                path.as_ref().as_ptr() as usize,
                path.as_ref().len()
            );
            
            // For create_new, creation failure is an error
            // For create, we continue even if creation fails (file might already exist)
            if self.create_new && result == usize::MAX {
                return Err(Error::new(ErrorKind::Other, "File already exists"));
            }
        }
        
        // Currently, we don't support any flags that require special handling
        let flags = 0;
        
        // Use Handle::open and wrap in File
        let handle = Handle::open(path.as_ref(), flags)
            .map_err(|_| Error::new(ErrorKind::Other, "Failed to open file"))?;
        
        Ok(File::from_handle(handle))
    }
}

impl Default for OpenOptions {
    /// Creates a blank new set of options ready for configuration
    ///
    /// This is equivalent to [`OpenOptions::new()`].
    fn default() -> Self {
        Self::new()
    }
}

// File system types and structures

/// High-level File wrapper with automatic resource management
/// 
/// This provides a Rust standard library-like interface while using
/// Scarlet Native capabilities under the hood. The file is automatically
/// closed when the File instance is dropped.
/// 
/// Files are not cloneable to ensure clear ownership semantics.
pub struct File {
    handle: Handle,
}

impl File {
    /// Create a File from an existing Handle
    /// 
    /// This is used internally by OpenOptions and other high-level APIs.
    /// 
    /// # Arguments
    /// * `handle` - The handle to wrap
    /// 
    /// # Returns
    /// File instance
    pub fn from_handle(handle: Handle) -> Self {
        File { handle }
    }
    
    /// Open a file with automatic resource management
    /// 
    /// This is a convenience method. For more control over file opening options, 
    /// use OpenOptions.
    /// 
    /// # Arguments
    /// * `path` - Path to the file
    /// 
    /// # Returns
    /// File instance or error
    pub fn open<P: AsRef<str>>(path: P) -> Result<Self> {
        // Open for read-only
        let handle = Handle::open(path.as_ref(), 0x0) // O_RDONLY
            .map_err(|_| Error::new(ErrorKind::Other, "Failed to open file"))?;
        Ok(File { handle })
    }
    
    /// Create a new file (equivalent to open with create, write, truncate)
    /// 
    /// This is a convenience method. For more control over file creation options,
    /// use OpenOptions.
    /// 
    /// # Arguments
    /// * `path` - Path to the file to create
    /// 
    /// # Returns
    /// File instance or error
    pub fn create<P: AsRef<str>>(path: P) -> Result<Self> {
        use crate::syscall::{syscall2, Syscall};
        
        // Use VfsCreateFile syscall to create the file
        let result = syscall2(
            Syscall::VfsCreateFile,
            path.as_ref().as_ptr() as usize,
            path.as_ref().len()
        );
        
        if result == usize::MAX {
            return Err(Error::new(ErrorKind::Other, "Failed to create file"));
        }
        
        // Open the created file for writing
        let handle = Handle::open(path.as_ref(), 0x1) // O_WRONLY
            .map_err(|_| Error::new(ErrorKind::Other, "Failed to open created file"))?;
        Ok(File { handle })
    }
    
    /// Open a file with specific flags (low-level interface)
    /// 
    /// This method provides direct access to system-level flags.
    /// Prefer using [`File::open`], [`File::create`], or [`OpenOptions`]
    /// for most use cases.
    /// 
    /// # Arguments
    /// * `path` - Path to the file
    /// * `flags` - Open flags (implementation-specific)
    /// 
    /// # Returns
    /// File instance or error
    pub fn open_with_flags<P: AsRef<str>>(path: P, flags: usize) -> Result<Self> {
        let handle = Handle::open(path.as_ref(), flags)
            .map_err(|_| Error::new(ErrorKind::Other, "Failed to open file"))?;
        Ok(File { handle })
    }
    
    /// Get the underlying handle (for advanced usage)
    /// 
    /// This allows access to the low-level Handle and its capabilities
    /// when you need more control than the high-level File interface provides.
    pub fn as_handle(&self) -> &Handle {
        &self.handle
    }

    /// Convert the File into a Handle
    /// 
    /// This consumes the File and returns the underlying Handle.
    /// 
    /// # Returns
    /// Handle instance
    pub fn into_handle(self) -> Handle {
        // Prevent the File's Drop from running
        let handle = unsafe {
            let handle_ptr = &self.handle as *const Handle;
            core::mem::forget(self);
            core::ptr::read(handle_ptr)
        };
        handle
    }

    /// Clone the underlying handle via duplication
    /// 
    /// This creates a new Handle that duplicates the underlying kernel object.
    /// This requires a syscall and creates an independent handle.
    /// 
    /// # Returns
    /// Cloned Handle instance or error
    pub fn clone_handle(&self) -> Result<Handle> {
        self.handle.duplicate()
            .map_err(|_| Error::new(ErrorKind::Other, "Failed to duplicate handle"))
    }
    
    /// Get the raw handle ID
    pub fn as_raw(&self) -> i32 {
        self.handle.as_raw()
    }
}

// Implement Rust standard library-like methods
impl File {
    /// Read data from the file
    /// 
    /// # Arguments
    /// * `buf` - Buffer to read data into
    /// 
    /// # Returns
    /// Number of bytes read or error
    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let stream = self.handle.as_stream()
            .map_err(|_| Error::new(ErrorKind::Unsupported, "Object does not support stream operations"))?;
        
        stream.read(buf)
            .map_err(|_| Error::new(ErrorKind::Other, "Read operation failed"))
    }

    /// Read directory entries from a directory file
    ///
    /// This method reads an entry from a directory file and returns a DirectoryEntry
    /// 
    /// # Returns
    /// * `Ok(entries)` - Vector of directory entries on success
    /// * `Err(errno)` - Error code on failure
    pub fn read_dir(&mut self) -> Result<Option<DirectoryEntry>> {
        // let file_handle = self.handle.as_file()
        //     .map_err(|_| Error::new(ErrorKind::Unsupported, "Object does not support file operations"))?;
        // let metadata = file_handle.metadata()
        //     .map_err(|_| Error::new(ErrorKind::Other, "Failed to get file metadata"))?;

        // crate::println!("metadata: {:?}", metadata);

        // if !metadata.is_directory() {
        //     return Err(Error::new(ErrorKind::InvalidInput, "Handle is not a directory"));
        // }

        let mut buf = [0u8; core::mem::size_of::<DirectoryEntryRaw>()];
        let bytes_read = self.handle.as_stream().unwrap().read(&mut buf);

        if bytes_read.is_err() {
            return Err(Error::new(ErrorKind::Other, "Failed to read directory entry"));
        }
        let bytes_read = bytes_read.unwrap();


        if bytes_read == 0 {
            return Ok(None); // EOF - no more entries
        }
        
        // Parse the directory entry
        if let Some(entry) = parse_dir_entry(&buf[..bytes_read as usize]) {
            Ok(Some(DirectoryEntry::from_raw(entry)))
        } else {
            Err(Error::new(ErrorKind::InvalidData, "Failed to parse directory entry"))
        }
    }

    /// Write data to the file
    /// 
    /// # Arguments
    /// * `buf` - Data to write
    /// 
    /// # Returns
    /// Number of bytes written or error
    pub fn write(&mut self, buf: &[u8]) -> Result<usize> {
        let stream = self.handle.as_stream()
            .map_err(|_| Error::new(ErrorKind::Unsupported, "Object does not support stream operations"))?;
            
        stream.write(buf)
            .map_err(|_| Error::new(ErrorKind::Other, "Write operation failed"))
    }
    
    /// Write all data to the file
    /// 
    /// This is a convenience method that ensures all data is written.
    /// 
    /// # Arguments
    /// * `buf` - Data to write
    /// 
    /// # Returns
    /// Success or error
    pub fn write_all(&mut self, buf: &[u8]) -> Result<()> {
        let stream = self.handle.as_stream()
            .map_err(|_| Error::new(ErrorKind::Unsupported, "Object does not support stream operations"))?;
            
        stream.write_all(buf)
            .map_err(|_| Error::new(ErrorKind::Other, "Write all operation failed"))
    }
    
    /// Seek to a position in the file
    /// 
    /// # Arguments
    /// * `pos` - Position to seek to
    /// 
    /// # Returns
    /// New absolute position or error
    pub fn seek(&mut self, pos: SeekFrom) -> Result<u64> {
        let file_obj = self.handle.as_file()
            .map_err(|_| Error::new(ErrorKind::Unsupported, "Object does not support file operations"))?;
            
        let scarlet_pos = match pos {
            SeekFrom::Start(offset) => ScarletSeekFrom::Start(offset),
            SeekFrom::Current(offset) => ScarletSeekFrom::Current(offset),
            SeekFrom::End(offset) => ScarletSeekFrom::End(offset),
        };
        
        file_obj.seek(scarlet_pos)
            .map_err(|_| Error::new(ErrorKind::Other, "Seek operation failed"))
    }
    
    /// Truncate the file to the specified size
    /// 
    /// # Arguments
    /// * `size` - New size of the file in bytes
    /// 
    /// # Returns
    /// Success or error
    pub fn set_len(&mut self, size: u64) -> Result<()> {
        let file_obj = self.handle.as_file()
            .map_err(|_| Error::new(ErrorKind::Unsupported, "Object does not support file operations"))?;
            
        file_obj.truncate(size)
            .map_err(|_| Error::new(ErrorKind::Other, "Truncate operation failed"))
    }
    
    // /// Get file metadata
    // /// 
    // /// # Returns
    // /// File metadata or error
    // pub fn metadata(&self) -> Result<FileMetadata> {
    //     let file_obj = self.handle.as_file()
    //         .map_err(|_| Error::new(ErrorKind::Unsupported, "Object does not support file operations"))?;
            
    //     file_obj.metadata()
    //         .map_err(|_| Error::new(ErrorKind::Other, "Metadata operation failed"))
    // }
    
    /// Get the current position in the file
    /// 
    /// # Returns
    /// Current position or error
    pub fn stream_position(&mut self) -> Result<u64> {
        self.seek(SeekFrom::Current(0))
    }
}

// Automatic resource cleanup
impl Drop for File {
    fn drop(&mut self) {
        // Handle already implements Drop, so the file will be automatically
        // closed when the File goes out of scope
    }
}

// Standard library-like traits for compatibility
impl Read for File {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        File::read(self, buf)
    }
}

impl Write for File {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        File::write(self, buf)
    }
    
    fn flush(&mut self) -> Result<()> {
        // For now, we don't have explicit flush capability
        // This could be added as a future enhancement
        Ok(())
    }
}

impl Seek for File {
    fn seek(&mut self, pos: SeekFrom) -> Result<u64> {
        File::seek(self, pos)
    }
}

//
// Mount Operations
//

/// Mount flags for mount operations
///
/// These flags are passed to the mount() system call to control mount behavior.
pub mod mount_flags {
    /// Mount filesystem read-only
    pub const MS_RDONLY: u32 = 0x01;
    /// Ignore suid and sgid bits
    pub const MS_NOSUID: u32 = 0x02;
    /// Disallow access to device special files
    pub const MS_NODEV: u32 = 0x04;
    /// Disallow program execution
    pub const MS_NOEXEC: u32 = 0x08;
    /// Writes are synced at once
    pub const MS_SYNCHRONOUS: u32 = 0x10;
    /// Bind mount
    pub const MS_BIND: u32 = 0x1000;
}

//
// File system operations  
//

/// Mount a filesystem
///
/// # Arguments
///
/// * `source` - Source device or filesystem name (e.g., "/dev/sda1", "tmpfs")
/// * `target` - Target mount point (e.g., "/mnt/data")
/// * `fstype` - Filesystem type (e.g., "ext4", "tmpfs", "bind")
/// * `flags` - Mount flags (see `mount_flags` module)
/// * `data` - Optional filesystem-specific data
///
/// # Examples
///
/// Mount a tmpfs:
/// ```
/// use scarlet::fs;
/// 
/// fs::mount("tmpfs", "/tmp", "tmpfs", 0, Some("size=100M"))?;
/// ```
///
/// Bind mount:
/// ```
/// use scarlet::fs::{mount, mount_flags};
///
/// mount("/source/dir", "/target/dir", "bind", mount_flags::MS_BIND, None)?;
/// ```
///
/// # Errors
///
/// Returns `Err` if the mount operation fails, such as:
/// - Invalid mount point
/// - Filesystem type not supported
/// - Permission denied
/// - Mount point already mounted
pub fn mount(
    source: &str,
    target: &str,
    fstype: &str,
    flags: u32,
    data: Option<&str>,
) -> Result<()> {
    use crate::syscall::{syscall5, Syscall};
    use crate::ffi::str_to_cstr_bytes;

    let source_c = str_to_cstr_bytes(source).map_err(|_| Error::new(ErrorKind::InvalidInput, "source contains null byte"))?;
    let target_c = str_to_cstr_bytes(target).map_err(|_| Error::new(ErrorKind::InvalidInput, "target contains null byte"))?;
    let fstype_c = str_to_cstr_bytes(fstype).map_err(|_| Error::new(ErrorKind::InvalidInput, "fstype contains null byte"))?;
    
    let data_c;
    let data_ptr = if let Some(data_str) = data {
        data_c = str_to_cstr_bytes(data_str).map_err(|_| Error::new(ErrorKind::InvalidInput, "data contains null byte"))?;
        data_c.as_ptr() as usize
    } else {
        0
    };

    let result = syscall5(
        Syscall::FsMount,
        source_c.as_ptr() as usize,
        target_c.as_ptr() as usize,
        fstype_c.as_ptr() as usize,
        flags as usize,
        data_ptr,
    );

    if result == usize::MAX {
        Err(Error::new(ErrorKind::Other, "mount failed"))
    } else {
        Ok(())
    }
}

/// Unmount a filesystem
///
/// # Arguments
///
/// * `target` - Mount point to unmount (e.g., "/mnt/data")
/// * `flags` - Unmount flags (reserved for future use, pass 0)
///
/// # Examples
///
/// ```
/// use scarlet::fs::unmount;
///
/// unmount("/mnt/data", 0)?;
/// ```
///
/// # Errors
///
/// Returns `Err` if the unmount operation fails, such as:
/// - Mount point not found
/// - Filesystem busy (files still open)
/// - Permission denied
pub fn unmount(target: &str, flags: u32) -> Result<()> {
    use crate::syscall::{syscall2, Syscall};
    use crate::ffi::str_to_cstr_bytes;

    let target_c = str_to_cstr_bytes(target).map_err(|_| Error::new(ErrorKind::InvalidInput, "target contains null byte"))?;

    let result = syscall2(
        Syscall::FsUmount,
        target_c.as_ptr() as usize,
        flags as usize,
    );

    if result == usize::MAX {
        Err(Error::new(ErrorKind::Other, "unmount failed"))
    } else {
        Ok(())
    }
}

/// Change the root filesystem (pivot_root)
///
/// This system call moves the old root filesystem to `old_root` and makes
/// `new_root` the new root filesystem. This is typically used during system
/// initialization to switch from an initramfs to the real root filesystem.
///
/// # Arguments
///
/// * `new_root` - Path to the new root filesystem
/// * `old_root` - Path where the old root filesystem will be moved
///
/// # Examples
///
/// ```
/// use scarlet::fs::pivot_root;
///
/// // Switch to new root, moving old root to /old_root
/// pivot_root("/mnt/newroot", "/mnt/newroot/old_root")?;
/// ```
///
/// # Errors
///
/// Returns `Err` if the pivot_root operation fails, such as:
/// - New root path does not exist or is not a mount point
/// - Old root path is invalid
/// - Permission denied
/// - Operation not supported in current namespace
pub fn pivot_root(new_root: &str, old_root: &str) -> Result<()> {
    use crate::syscall::{syscall2, Syscall};
    use crate::ffi::str_to_cstr_bytes;

    let new_root_c = str_to_cstr_bytes(new_root).map_err(|_| Error::new(ErrorKind::InvalidInput, "new_root contains null byte"))?;
    let old_root_c = str_to_cstr_bytes(old_root).map_err(|_| Error::new(ErrorKind::InvalidInput, "old_root contains null byte"))?;

    let result = syscall2(
        Syscall::FsPivotRoot,
        new_root_c.as_ptr() as usize,
        old_root_c.as_ptr() as usize,
    );

    if result == usize::MAX {
        Err(Error::new(ErrorKind::Other, "pivot_root failed"))
    } else {
        Ok(())
    }
}

/// Create a new directory
/// 
/// This function creates a new directory at the specified path.
/// 
/// # Arguments
/// * `path` - Path to the new directory
/// 
pub fn create_directory<P: AsRef<str>>(path: P) -> Result<()> {
    use crate::syscall::{syscall1, Syscall};
    use crate::ffi::str_to_cstr_bytes;

    let path_c = str_to_cstr_bytes(path.as_ref())
        .map_err(|_| Error::new(ErrorKind::InvalidInput, "path contains null byte"))?;

    let result = syscall1(
        Syscall::VfsCreateDirectory,
        path_c.as_ptr() as usize,
    );

    if result == usize::MAX {
        Err(Error::new(ErrorKind::Other, "create directory failed"))
    } else {
        Ok(())
    }
}

/// Change the current working directory
///
/// # Arguments
///
/// * `path` - Path to the new working directory
///
/// # Examples
///
/// ```
/// use scarlet::fs::change_directory;
///
/// change_directory("/tmp")?;
/// ```
///
/// # Errors
///
/// Returns `Err` if the directory change fails, such as:
/// - Directory does not exist
/// - Permission denied
/// - Invalid path
pub fn change_directory<P: AsRef<str>>(path: P) -> Result<()> {
    use crate::syscall::{syscall1, Syscall};
    use crate::ffi::str_to_cstr_bytes;

    let path_c = str_to_cstr_bytes(path.as_ref())
        .map_err(|_| Error::new(ErrorKind::InvalidInput, "path contains null byte"))?;

    let result = syscall1(
        Syscall::VfsChangeDirectory,
        path_c.as_ptr() as usize,
    );

    if result == usize::MAX {
        Err(Error::new(ErrorKind::Other, "change directory failed"))
    } else {
        Ok(())
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
    pub fn name_str(&self) -> core::result::Result<&str, core::str::Utf8Error> {
        let name_bytes = &self.name[..self.name_len as usize];
        core::str::from_utf8(name_bytes)
    }
    
    /// Get the name as an owned String
    pub fn name_string(&self) -> core::result::Result<crate::string::String, core::str::Utf8Error> {
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

/// Helper function to parse directory entries from readdir buffer (backward compatibility)
/// 
/// This function is kept for backward compatibility with older code that manually
/// handles directory entry parsing. Consider using [`File::read_dir`] or 
/// [`list_directory`] for new code, which handle parsing automatically.
/// 
/// # Arguments
/// * `buf` - Buffer containing directory entry from readdir
/// * `bytes_read` - Number of bytes actually read
/// 
/// # Returns
/// * `Some((name, file_type, file_id, size))` - Parsed directory entry data
/// * `None` - If parsing failed or EOF reached
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

/// List all files and directories in a directory
/// 
/// This is a convenience function that opens a directory and reads all entries.
/// It demonstrates how to use the new directory reading API.
/// 
/// # Arguments
/// * `path` - Path to the directory to list
/// 
/// # Returns
/// * `Ok(entries)` - Vector of directory entries on success
/// * `Err(error)` - I/O error on failure
/// 
/// # Examples
/// 
/// ```
/// use scarlet::fs;
/// 
/// let entries = fs::list_directory("/tmp")?;
/// for entry in entries {
///     println!("{}: {} bytes", entry.name, entry.size);
/// }
/// ```
/// 
pub fn list_directory(path: &str) -> Result<crate::vec::Vec<DirectoryEntry>> {
    use crate::vec::Vec;

    let dir_file = File::open(path);
    if dir_file.is_err() {
        return Err(dir_file.err().unwrap());
    }
    
    let mut entries = Vec::new();

    let mut file = dir_file.unwrap();
    
    loop {
        match file.read_dir() {
            Ok(Some(entry)) => {
                entries.push(entry);
            }
            Ok(None) => break, // EOF
            Err(errno) => {
                return Err(errno);
            }
        }
    }

    Ok(entries)
}

/// Count files and directories in a directory
/// 
/// This is an example function that demonstrates using the directory listing API
/// to analyze directory contents.
/// 
/// # Arguments
/// * `path` - Path to the directory to analyze
/// 
/// # Returns
/// * `Ok((file_count, dir_count))` - Tuple of (number of files, number of directories)
/// * `Err(error)` - I/O error on failure
/// 
/// # Examples
/// 
/// ```
/// use scarlet::fs;
/// 
/// let (files, dirs) = fs::count_directory_entries("/home")?;
/// println!("Found {} files and {} directories", files, dirs);
/// ```
/// 
pub fn count_directory_entries(path: &str) -> Result<(usize, usize)> {
    let entries = list_directory(path)?;
    
    let mut file_count = 0;
    let mut dir_count = 0;

    for entry in entries {
        if entry.is_file() {
            file_count += 1;
        } else if entry.is_directory() {
            dir_count += 1;
        }
    }

    Ok((file_count, dir_count))
}