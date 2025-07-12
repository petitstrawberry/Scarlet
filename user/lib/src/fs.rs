//! File system abstraction for Scarlet Native API
//!
//! This module provides a Rust standard library-like file system interface
//! using the OpenOptions builder pattern and high-level convenience functions.
//!
//! ## Core Functions
//!
//! ### File Operations
//! - [`create_file`]: Create a new regular file
//! - File access via [`File::open`], [`File::create`], and [`OpenOptions`]
//!
//! ### Directory Operations  
//! - [`create_directory`]: Create a new directory
//! - [`change_directory`]: Change current working directory
//! - [`remove`]: Remove files or directories (unified)
//!
//! ### Filesystem Operations
//! - [`mount`]: Mount filesystems
//! - [`unmount`]: Unmount filesystems
//! - [`pivot_root`]: Change root filesystem

use crate::handle::Handle;
use crate::handle::capability::{SeekFrom as ScarletSeekFrom, FileMetadata};
use crate::string::String;
use crate::vec::Vec;
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
        // Convert options to flags
        let flags = self.to_flags()?;
        
        // Use Handle::open and wrap in File
        let handle = Handle::open(path.as_ref(), flags)
            .map_err(|_| Error::new(ErrorKind::Other, "Failed to open file"))?;
        
        Ok(File::from_handle(handle))
    }
    
    /// Convert OpenOptions to system flags
    fn to_flags(&self) -> Result<usize> {
        let mut flags = 0usize;
        
        // Validate option combinations
        if !self.read && !self.write && !self.append {
            return Err(Error::new(ErrorKind::InvalidInput, "Must specify at least one access mode"));
        }
        
        if self.truncate && !self.write && !self.append {
            return Err(Error::new(ErrorKind::InvalidInput, "Cannot truncate without write access"));
        }
        
        if self.create_new && !self.write && !self.append {
            return Err(Error::new(ErrorKind::InvalidInput, "Cannot create new file without write access"));
        }
        
        // Set access mode flags
        if self.read && self.write {
            flags |= 0x2; // O_RDWR
        } else if self.write || self.append {
            flags |= 0x1; // O_WRONLY
        } else {
            flags |= 0x0; // O_RDONLY (default)
        }
        
        // Set creation flags
        if self.create_new {
            flags |= 0x200 | 0x80; // O_CREAT | O_EXCL
        } else if self.create {
            flags |= 0x200; // O_CREAT
        }
        
        // Set other flags
        if self.append {
            flags |= 0x8; // O_APPEND
        }
        
        if self.truncate && !self.create_new {
            flags |= 0x400; // O_TRUNC
        }
        
        Ok(flags)
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
    pub(crate) fn from_handle(handle: Handle) -> Self {
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
        // O_CREAT | O_WRONLY | O_TRUNC
        let handle = Handle::open(path.as_ref(), 0x200 | 0x1 | 0x400)
            .map_err(|_| Error::new(ErrorKind::Other, "Failed to create file"))?;
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
    pub fn handle(&self) -> &Handle {
        &self.handle
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
    
    /// Get file metadata
    /// 
    /// # Returns
    /// File metadata or error
    pub fn metadata(&self) -> Result<FileMetadata> {
        let file_obj = self.handle.as_file()
            .map_err(|_| Error::new(ErrorKind::Unsupported, "Object does not support file operations"))?;
            
        file_obj.metadata()
            .map_err(|_| Error::new(ErrorKind::Other, "Metadata operation failed"))
    }
    
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
