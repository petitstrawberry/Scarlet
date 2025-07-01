//! Virtual File System (VFS) Module - Version 2 Architecture
//!
//! This module provides a modern Virtual File System implementation based on VFS v2
//! architecture, supporting per-task isolated filesystems, containerization, and 
//! advanced mount operations including bind mounts and overlay filesystems.
//!
//! # VFS v2 Architecture Overview
//!
//! The VFS v2 architecture provides a clean separation of concerns with three main
//! components inspired by modern operating systems:
//!
//! ## Core Components
//!
//! - **VfsEntry**: Path hierarchy cache (similar to Linux dentry)
//!   - Represents "names" and "links" in the filesystem hierarchy
//!   - Provides fast path resolution with weak reference-based caching
//!   - Manages parent-child relationships in the VFS tree
//!
//! - **VfsNode**: File entity interface (similar to Linux inode/BSD vnode)
//!   - Abstract representation of files, directories, and special files
//!   - Provides metadata access and type information
//!   - Enables clean downcasting for filesystem-specific operations
//!
//! - **FileSystemOperations**: Unified driver API for filesystem implementations
//!   - Consolidated interface for all filesystem operations (lookup, create, etc.)
//!   - Clean separation between VFS core and filesystem drivers
//!   - Supports both simple and complex filesystem types
//!
//! ## Key Infrastructure
//!
//! - **VfsManager**: Main VFS management structure supporting isolation and sharing
//! - **MountTree**: Hierarchical mount tree with support for bind mounts and overlays
//! - **FileSystemDriverManager**: Global singleton for driver registration (VFS v1 compatibility)
//! - **MountPoint**: Associates filesystem instances with mount paths and manages mount relationships
//!
//! ## VfsManager Distribution and Isolation
//!
//! - **Per-Task VfsManager**: Each task can have its own isolated `VfsManager` instance
//!   stored as `Option<Arc<VfsManager>>` in the task structure
//! - **Shared Filesystems**: Multiple VfsManager instances can share underlying filesystem
//!   objects while maintaining independent mount points
//! - **Global Fallback**: Tasks without their own VFS use the global VfsManager instance
//!
//! ## Advanced Mount Operations
//!
//! VFS v2 provides comprehensive mount functionality for flexible filesystem composition:
//!
//! ### Basic Filesystem Mounting
//! ```rust
//! let vfs = VfsManager::new();
//! 
//! // Create and mount a tmpfs
//! let tmpfs = TmpFS::new(1024 * 1024); // 1MB limit
//! vfs.mount(tmpfs, "/tmp", 0)?;
//! 
//! // Mount with specific options
//! vfs.mount_with_options(filesystem, "/mnt/data", &mount_options)?;
//! ```
//!
//! ### Bind Mount Operations
//! ```rust
//! // Basic bind mount - mount a directory at another location
//! vfs.bind_mount("/source/dir", "/target/dir")?;
//! 
//! // Cross-VFS bind mount for container isolation
//! let host_vfs = Arc::new(host_vfs_manager);
//! container_vfs.bind_mount_from(host_vfs, "/host/data", "/container/data")?;
//! ```
//!
//! ### Overlay Filesystem Support
//! ```rust
//! // Create overlay combining multiple layers
//! let overlay = OverlayFS::new(
//!     Some((upper_mount, upper_entry)),  // Upper layer (writable)
//!     vec![(lower_mount, lower_entry)],  // Lower layers (read-only)
//!     "system_overlay".to_string()
//! )?;
//! vfs.mount(overlay, "/merged", 0)?;
//! ```
//!
//! ## Available Filesystem Types
//!
//! VFS v2 includes several built-in filesystem drivers:
//!
//! - **TmpFS**: Memory-based temporary filesystem with optional size limits
//! - **CpioFS**: Read-only CPIO archive filesystem for initramfs
//! - **OverlayFS**: Union/overlay filesystem combining multiple layers
//! - **InitramFS**: Special handling for initial ramdisk mounting
//!
//! ## Usage Patterns
//!
//! ### Container Isolation with Namespaces
//! ```rust
//! // Create isolated VfsManager for container
//! let container_vfs = VfsManager::new();
//! 
//! // Mount container root filesystem
//! let container_fs = TmpFS::new(512 * 1024 * 1024); // 512MB
//! container_vfs.mount(container_fs, "/", 0)?;
//! 
//! // Bind mount host resources selectively
//! let host_vfs = get_global_vfs();
//! container_vfs.bind_mount_from(&host_vfs, "/host/shared", "/shared")?;
//! 
//! // Assign isolated namespace to task
//! task.vfs = Some(Arc::new(container_vfs));
//! ```
//!
//! ### Shared VFS Access Patterns
//!
//! VFS v2 supports multiple sharing patterns for different use cases:
//!
//! #### Full VFS Sharing via Arc
//! ```rust
//! // Share entire VfsManager instance including mount points
//! let shared_vfs = Arc::new(vfs_manager);
//! let task_vfs = Arc::clone(&shared_vfs);
//! 
//! // All mount operations affect the shared mount tree
//! shared_vfs.mount(tmpfs, "/tmp", 0)?;  // Visible to all references
//! 
//! // Useful for:
//! // - Fork-like behavior where child inherits parent's filesystem view
//! // - Thread-like sharing where all threads see the same mount points
//! // - System-wide mount operations
//! ```
//!
//! #### Selective Resource Sharing via Bind Mounts
//! ```rust
//! // Each container has isolated filesystem but shares specific directories
//! let container1_vfs = VfsManager::new();
//! let container2_vfs = VfsManager::new();
//! 
//! // Both containers share a common data directory
//! let host_vfs = get_global_vfs();
//! container1_vfs.bind_mount_from(&host_vfs, "/host/shared", "/data")?;
//! container2_vfs.bind_mount_from(&host_vfs, "/host/shared", "/data")?;
//! ```
//!
//! ## System Call Interface
//!
//! VFS v2 provides system calls that operate within each task's
//! VFS namespace:
//!
//! - File operations: `open()`, `read()`, `write()`, `close()`, `lseek()`
//! - Directory operations: `mkdir()`, `readdir()`
//! - Mount operations: `mount()`, `umount()`, `pivot_root()`
//!
//! ## Performance Characteristics
//!
//! VFS v2 is designed for performance with:
//!
//! - **Path Resolution Caching**: VfsEntry provides fast lookup of recently accessed paths
//! - **Weak Reference Cleanup**: Automatic cleanup of expired cache entries
//! - **Mount Boundary Optimization**: Efficient crossing of mount points during path resolution
//! - **Lock Granularity**: Fine-grained locking to minimize contention
//!
//! ## Migration from VFS v1
//!
//! VFS v2 maintains compatibility with existing code while providing improved APIs.
//! The old interfaces are deprecated but still functional for transition purposes.
//!
//! This architecture enables flexible deployment scenarios from simple shared filesystems
//! to complete filesystem isolation with selective resource sharing for containerized
//! applications, all while maintaining high performance and POSIX compatibility.

pub mod vfs_v2;
pub use vfs_v2::*;
pub mod params;
pub use params::*;
pub use vfs_v2::manager::VfsManager;

use alloc::{boxed::Box, collections::BTreeMap, format, string::{String, ToString}, sync::Arc, vec::Vec};
use crate::{device::{block::{BlockDevice}, DeviceType}, vm::vmem::MemoryArea};
use crate::object::capability::{StreamOps, StreamError};

use spin::RwLock;
use ::core::fmt;

extern crate alloc;

pub const MAX_PATH_LENGTH: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FileSystemErrorKind {
    NotFound,
    NoSpace,
    PermissionDenied,
    IoError,
    InvalidData,
    InvalidPath,
    AlreadyExists,
    NotADirectory,
    NotAFile,
    IsADirectory,
    ReadOnly,
    DeviceError,
    NotSupported,
    BrokenFileSystem,
    Busy,
    DirectoryNotEmpty,
    InvalidOperation,
    CrossDevice,
    FileExists,
}

#[derive(Clone)]
pub struct FileSystemError {
    pub kind: FileSystemErrorKind,
    pub message: String,
}

impl FileSystemError {
    pub fn new(kind: FileSystemErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl fmt::Debug for FileSystemError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "FileSystemError {{ kind: {:?}, message: {} }}", self.kind, self.message)
    }
}

/// Information about device files in the filesystem
/// 
/// Scarlet uses a simplified device identification system based on unique device IDs
/// rather than the traditional Unix major/minor number pairs. This provides:
/// 
/// - **Simplified Management**: Single ID instead of major/minor pair reduces complexity
/// - **Unified Namespace**: All devices share a common ID space regardless of type
/// - **Dynamic Allocation**: Device IDs can be dynamically assigned without conflicts
/// - **Type Safety**: Device type is explicitly specified alongside the ID
/// 
/// # Architecture
/// 
/// Each device in Scarlet is uniquely identified by:
/// - `device_id`: A unique identifier within the system's device namespace
/// - `device_type`: Explicit type classification (Character, Block, etc.)
/// 
/// This differs from traditional Unix systems where:
/// - Major numbers identify device drivers
/// - Minor numbers identify specific devices within a driver
/// 
/// # Examples
/// 
/// ```rust
/// // Character device for terminal
/// let tty_device = DeviceFileInfo {
///     device_id: 1,
///     device_type: DeviceType::Char,
/// };
/// 
/// // Block device for storage
/// let disk_device = DeviceFileInfo {
///     device_id: 100,
///     device_type: DeviceType::Block,
/// };
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DeviceFileInfo {
    pub device_id: usize,
    pub device_type: DeviceType,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FileType {
    RegularFile,
    Directory,
    CharDevice(DeviceFileInfo),
    BlockDevice(DeviceFileInfo),
    Pipe,
    SymbolicLink,
    Socket,
    Unknown,
}

#[derive(Debug, Clone, Copy)]
pub struct FilePermission {
    pub read: bool,
    pub write: bool,
    pub execute: bool,
}

#[derive(Debug, Clone)]
pub struct FileMetadata {
    pub file_type: FileType,
    pub size: usize,
    pub permissions: FilePermission,
    pub created_time: u64,
    pub modified_time: u64,
    pub accessed_time: u64,
    /// Unique file identifier within the filesystem
    /// Used for hard link management - multiple directory entries
    /// can share the same file_id to point to the same file data
    pub file_id: u64,
    /// Number of hard links pointing to this file
    /// File data is only deleted when link_count reaches zero
    pub link_count: u32,
}

/// Structure representing a directory entry (internal representation)
#[derive(Debug, Clone)]
pub struct DirectoryEntryInternal {
    pub name: String,
    pub file_type: FileType,
    pub size: usize,
    /// Unique file identifier - same as the file_id in FileMetadata
    /// Multiple directory entries with the same file_id represent hard links
    pub file_id: u64,
    pub metadata: Option<FileMetadata>,
}

/// Binary representation of directory entry for system call interface
/// This structure has a fixed layout for efficient copying between kernel and user space
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct DirectoryEntry {
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

impl DirectoryEntry {
    /// Create a DirectoryEntry from internal representation
    pub fn from_internal(internal: &DirectoryEntryInternal) -> Self {
        let file_type_byte = match internal.file_type {
            FileType::RegularFile => 0u8,
            FileType::Directory => 1u8,
            FileType::SymbolicLink => 2u8,
            FileType::CharDevice(_) => 3u8,
            FileType::BlockDevice(_) => 4u8,
            FileType::Pipe => 5u8,
            FileType::Socket => 6u8,
            FileType::Unknown => 7u8,
        };

        let name_bytes = internal.name.as_bytes();
        let mut name_array = [0u8; 256];
        let copy_len = ::core::cmp::min(name_bytes.len(), 255); // Reserve 1 byte for null terminator
        name_array[..copy_len].copy_from_slice(&name_bytes[..copy_len]);
        name_array[copy_len] = 0; // Null terminator

        Self {
            file_id: internal.file_id,
            size: internal.size as u64,
            file_type: file_type_byte,
            name_len: copy_len as u8,
            _reserved: [0; 6],
            name: name_array,
        }
    }

    /// Get the name as a string
    pub fn name_str(&self) -> Result<&str, ::core::str::Utf8Error> {
        let name_bytes = &self.name[..self.name_len as usize];
        ::core::str::from_utf8(name_bytes)
    }

    /// Get the actual size of this entry
    pub fn entry_size(&self) -> usize {
        // Fixed size of the entry structure
        ::core::mem::size_of::<Self>()  as usize
    }

    /// Parse a DirectoryEntry from raw bytes
    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < ::core::mem::size_of::<Self>() {
            return None;
        }

        // Safety: We've checked the size above
        let entry = unsafe {
            ::core::ptr::read(data.as_ptr() as *const Self)
        };

        // Basic validation
        if entry.name_len as usize > 255 {
            return None;
        }

        Some(entry)
    }
}

/// Structure representing a directory
pub struct Directory {
    pub path: String,
}

impl Directory {
    pub fn open(path: String) -> Self {
        Self {
            path,
        }
    }
}

pub enum SeekFrom {
    Start(u64),
    Current(i64),
    End(i64),
}

/// Trait for file object
/// 
/// This trait represents a file-like object that supports both stream operations
/// and file-specific operations like seeking and metadata access.
/// Directory reading is handled through normal read() operations.
pub trait FileObject: StreamOps {
    /// Seek to a position in the file stream
    fn seek(&self, whence: SeekFrom) -> Result<u64, StreamError>;
    
    /// Get metadata about the file
    fn metadata(&self) -> Result<crate::fs::FileMetadata, StreamError>;

    /// Truncate the file to the specified size
    /// 
    /// This method changes the size of the file to the specified length.
    /// If the new size is smaller than the current size, the file is truncated.
    /// If the new size is larger, the file is extended with zero bytes.
    /// 
    /// # Arguments
    /// 
    /// * `size` - New size of the file in bytes
    /// 
    /// # Returns
    /// 
    /// * `Result<(), StreamError>` - Ok if the file was truncated successfully
    /// 
    /// # Errors
    /// 
    /// * `StreamError` - If the file is a directory or the operation is not supported
    fn truncate(&self, size: u64) -> Result<(), StreamError> {
        let _ = size;
        Err(StreamError::NotSupported)
    }
}

/// Enum defining the type of file system
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FileSystemType {
    /// File system that operates on block devices (disk-based)
    Block,
    /// File system that operates on memory regions (RAM-based)
    Memory,
    /// File system that can operate on both block devices and memory regions
    Hybrid,
    /// Special or virtual file systems (e.g., procfs, sysfs)
    Virtual,
    /// Device file system (e.g., /dev)
    Device,
}

/// Trait for file system drivers
/// 
/// This trait is used to create file systems from block devices or memory areas.
/// It is not intended to be used directly by the VFS manager.
/// Instead, the VFS manager will use the appropriate creation method based on the source.
pub trait FileSystemDriver: Send + Sync {
    /// Get the name of the file system driver
    fn name(&self) -> &'static str;
    
    /// Get the type of the file system
    fn filesystem_type(&self) -> FileSystemType;
    
    /// Create a file system from a block device
    /// 
    /// When implementing this method, ensure that the file system driver can handle block device-based creation.
    /// If the driver does not support this, return an appropriate error.
    /// 
    /// # Arguments
    /// 
    /// * `_block_device` - The block device to use for creating the file system
    /// * `_block_size` - The block size of the device
    /// 
    fn create_from_block(&self, _block_device: Box<dyn BlockDevice>, _block_size: usize) -> Result<Arc<dyn crate::fs::vfs_v2::core::FileSystemOperations>, FileSystemError> {
        if self.filesystem_type() == FileSystemType::Memory || self.filesystem_type() == FileSystemType::Virtual {
            return Err(FileSystemError {
                kind: FileSystemErrorKind::NotSupported,
                message: "This file system driver does not support block device-based creation".to_string(),
            });
        }
        
        Err(FileSystemError {
            kind: FileSystemErrorKind::NotSupported,
            message: "create_from_block() not implemented for this file system driver".to_string(),
        })
    }
    
    /// Create a file system from a memory area
    /// 
    /// When implementing this method, ensure that the file system driver can handle memory-based creation.
    /// If the driver does not support this, return an appropriate error.
    /// 
    /// # Notes
    /// 
    /// File system drivers must validate the provided MemoryArea to ensure it is valid.
    /// If the MemoryArea is invalid, the driver should return an appropriate error.
    /// 
    /// # Arguments
    /// 
    /// * `_memory_area` - The memory area to use for creating the file system
    /// 
    /// # Returns
    /// 
    /// * `Result<Arc<dyn FileSystemOperations>, FileSystemError>` - The created file system
    /// 
    fn create_from_memory(&self, _memory_area: &crate::vm::vmem::MemoryArea) -> Result<Arc<dyn crate::fs::vfs_v2::core::FileSystemOperations>, FileSystemError> {
        if self.filesystem_type() == FileSystemType::Block {
            return Err(FileSystemError {
                kind: FileSystemErrorKind::NotSupported,
                message: "This file system driver does not support memory-based creation".to_string(),
            });
        }
        
        Err(FileSystemError {
            kind: FileSystemErrorKind::NotSupported,
            message: "create_from_memory() not implemented for this file system driver".to_string(),
        })
    }

    fn create(&self) -> Result<Arc<dyn crate::fs::vfs_v2::core::FileSystemOperations>, FileSystemError> {
        // Default implementation that can be overridden by specific drivers
        // This is a convenience method for drivers that do not need to handle block or memory creation
        Err(FileSystemError {
            kind: FileSystemErrorKind::NotSupported,
            message: "create() not implemented for this file system driver".to_string(),
        })
    }

    /// Create a file system with option string
    /// 
    /// This method creates a filesystem instance based on an option string, which
    /// is typically passed from the mount() system call. The option string format
    /// is filesystem-specific and should be parsed by the individual driver.
    /// 
    /// # Arguments
    /// 
    /// * `options` - Option string containing filesystem-specific parameters
    /// 
    /// # Returns
    /// 
    /// * `Result<Arc<dyn FileSystemOperations>, FileSystemError>` - The created file system
    /// 
    /// # Note
    /// 
    /// This method allows the filesystem driver to handle its own option parsing,
    /// keeping the mount syscall generic and delegating filesystem-specific logic
    /// to the appropriate driver.
    fn create_from_option_string(&self, options: &str) -> Result<Arc<dyn crate::fs::vfs_v2::core::FileSystemOperations>, FileSystemError> {
        let _ = options; // Suppress unused parameter warning
        // Default implementation falls back to create()
        self.create()
    }

    /// Create a file system with structured parameters
    /// 
    /// This method creates file systems using type-safe structured parameters
    /// that implement the FileSystemParams trait. This approach replaces the
    /// old BTreeMap<String, String> approach with better type safety.
    /// 
    /// # Arguments
    /// 
    /// * `params` - Structured parameter implementing FileSystemParams
    /// 
    /// # Returns
    /// 
    /// * `Result<Arc<dyn FileSystemOperations>, FileSystemError>` - The created file system
    /// 
    /// # Note
    /// 
    /// This method uses dynamic dispatch for parameter handling to support
    /// future dynamic filesystem module loading while maintaining type safety.
    /// 
    fn create_from_params(&self, params: &dyn crate::fs::params::FileSystemParams) -> Result<Arc<dyn crate::fs::vfs_v2::core::FileSystemOperations>, FileSystemError> {
        // Default implementation falls back to create()
        let _ = params; // Suppress unused parameter warning
        self.create()
    }
}

/// Singleton for global access to the FileSystemDriverManager
static mut FS_DRIVER_MANAGER: Option<FileSystemDriverManager> = None;

/// Global filesystem driver manager singleton
/// 
/// Provides global access to the FileSystemDriverManager instance.
/// This function ensures thread-safe initialization of the singleton
/// and returns a mutable reference for driver registration and filesystem creation.
/// 
/// # Returns
/// 
/// Mutable reference to the global FileSystemDriverManager instance
/// 
/// # Thread Safety
/// 
/// This function is marked as unsafe due to static mutable access, but
/// the returned manager uses internal synchronization for thread safety.
#[allow(static_mut_refs)]
pub fn get_fs_driver_manager() -> &'static mut FileSystemDriverManager {
    unsafe {
        if FS_DRIVER_MANAGER.is_none() {
            FS_DRIVER_MANAGER = Some(FileSystemDriverManager::new());
        }
        FS_DRIVER_MANAGER.as_mut().unwrap()
    }
}

/// Global filesystem driver manager singleton
/// 
/// Provides global access to the FileSystemDriverManager instance.
/// This function ensures thread-safe initialization of the singleton
/// and returns a mutable reference for driver registration and filesystem creation.
/// 
/// # Returns
/// 
/// Mutable reference to the global FileSystemDriverManager instance
/// 
/// # Thread Safety
/// 
/// This function is marked as unsafe due to static mutable access, but
/// Filesystem driver manager for centralized driver registration and management
/// 
/// The FileSystemDriverManager provides a centralized system for managing filesystem
/// drivers in the kernel. It separates driver management responsibilities from individual
/// VfsManager instances, enabling shared driver access across multiple VFS namespaces.
/// 
/// # Features
/// 
/// - **Driver Registration**: Register filesystem drivers for system-wide use
/// - **Type-Safe Creation**: Create filesystems with structured parameter validation
/// - **Multi-Source Support**: Support for block device, memory, and virtual filesystems
/// - **Thread Safety**: All operations are thread-safe using RwLock protection
/// - **Future Extensibility**: Designed for dynamic filesystem module loading
/// 
/// # Architecture
/// 
/// The manager maintains a registry of drivers identified by name, with each driver
/// implementing the FileSystemDriver trait. Drivers specify their supported source
/// types (block, memory, virtual) and provide creation methods for each type.
/// 
/// # Usage
/// 
/// ```rust
/// // Register a filesystem driver
/// let manager = get_fs_driver_manager();
/// manager.register_driver(Box::new(MyFSDriver));
/// 
/// // Create filesystem from block device
/// let device = get_block_device();
/// let fs = manager.create_from_block("myfs", device, 512)?;
/// 
/// // Create filesystem with structured parameters
/// let params = MyFSParams::new();
/// let fs = manager.create_with_params("myfs", &params)?;
/// ```
pub struct FileSystemDriverManager {
    /// Registered file system drivers indexed by name
    drivers: RwLock<BTreeMap<String, Box<dyn FileSystemDriver>>>,
}

impl FileSystemDriverManager {
    /// Create a new filesystem driver manager
    /// 
    /// Initializes an empty driver manager with no registered drivers.
    /// Drivers must be registered using register_driver() before they
    /// can be used to create filesystems.
    /// 
    /// # Returns
    /// 
    /// A new FileSystemDriverManager instance
    pub fn new() -> Self {
        Self {
            drivers: RwLock::new(BTreeMap::new()),
        }
    }

    /// Register a filesystem driver
    /// 
    /// Adds a new filesystem driver to the manager's registry. The driver
    /// will be indexed by its name() method return value. If a driver with
    /// the same name already exists, it will be replaced.
    /// 
    /// # Arguments
    /// 
    /// * `driver` - The filesystem driver to register, implementing FileSystemDriver trait
    /// 
    /// # Example
    /// 
    /// ```rust
    /// let manager = get_fs_driver_manager();
    /// manager.register_driver(Box::new(MyFileSystemDriver));
    /// ```
    pub fn register_driver(&mut self, driver: Box<dyn FileSystemDriver>) {
        self.drivers.write().insert(driver.name().to_string(), driver);
    }

    /// Get a list of registered driver names
    /// 
    /// Returns the names of all currently registered filesystem drivers.
    /// This is useful for debugging and system introspection.
    /// 
    /// # Returns
    /// 
    /// Vector of driver names in alphabetical order
    pub fn list_drivers(&self) -> Vec<String> {
        self.drivers.read().keys().cloned().collect()
    }

    /// Check if a driver with the specified name is registered
    /// 
    /// Performs a quick lookup to determine if a named driver exists
    /// in the registry without attempting to use it.
    /// 
    /// # Arguments
    /// 
    /// * `driver_name` - The name of the driver to check for
    /// 
    /// # Returns
    /// 
    /// `true` if the driver is registered, `false` otherwise
    pub fn has_driver(&self, driver_name: &str) -> bool {
        self.drivers.read().contains_key(driver_name)
    }

    /// Create a filesystem from a block device
    /// 
    /// Creates a new filesystem instance using the specified driver and block device.
    /// The driver must support block device-based filesystem creation. This method
    /// validates that the driver supports block devices before attempting creation.
    /// 
    /// # Arguments
    /// 
    /// * `driver_name` - The name of the registered driver to use
    /// * `block_device` - The block device that will store the filesystem data
    /// * `block_size` - The block size for I/O operations (typically 512, 1024, or 4096 bytes)
    /// 
    /// # Returns
    /// 
    /// * `Ok(Arc<dyn FileSystemOperations>)` - Successfully created filesystem instance
    /// * `Err(FileSystemError)` - If driver not found, doesn't support block devices, or creation fails
    /// 
    /// # Errors
    /// 
    /// - `NotFound` - Driver with the specified name is not registered
    /// - `NotSupported` - Driver doesn't support block device-based filesystems
    /// - Driver-specific errors during filesystem creation
    /// 
    /// # Example
    /// 
    /// ```rust
    /// let device = get_block_device();
    /// let fs = manager.create_from_block("ext4", device, 4096)?;
    /// ```
    pub fn create_from_block(
        &self,
        driver_name: &str,
        block_device: Box<dyn BlockDevice>,
        block_size: usize,
    ) -> Result<Arc<dyn crate::fs::vfs_v2::core::FileSystemOperations>, FileSystemError> {
        let binding = self.drivers.read();
        let driver = binding.get(driver_name).ok_or(FileSystemError {
            kind: FileSystemErrorKind::NotFound,
            message: format!("File system driver '{}' not found", driver_name),
        })?;

        if driver.filesystem_type() == FileSystemType::Memory || driver.filesystem_type() == FileSystemType::Virtual {
            return Err(FileSystemError {
                kind: FileSystemErrorKind::NotSupported,
                message: format!("File system driver '{}' does not support block devices", driver_name),
            });
        }

        driver.create_from_block(block_device, block_size)
    }

    /// Create a filesystem from a memory area
    /// 
    /// Creates a new filesystem instance using the specified driver and memory region.
    /// This is typically used for RAM-based filesystems like tmpfs or for mounting
    /// filesystem images stored in memory (e.g., initramfs).
    /// 
    /// # Arguments
    /// 
    /// * `driver_name` - The name of the registered driver to use
    /// * `memory_area` - The memory region containing filesystem data or available for use
    /// 
    /// # Returns
    /// 
    /// * `Ok(Arc<dyn FileSystemOperations>)` - Successfully created filesystem instance
    /// * `Err(FileSystemError)` - If driver not found, doesn't support memory-based creation, or creation fails
    /// 
    /// # Errors
    /// 
    /// - `NotFound` - Driver with the specified name is not registered
    /// - `NotSupported` - Driver only supports block device-based filesystems
    /// - Driver-specific errors during filesystem creation
    /// 
    /// # Example
    /// 
    /// ```rust
    /// let memory_area = MemoryArea::new(addr, size);
    /// let fs = manager.create_from_memory("cpiofs", &memory_area)?;
    /// ```
    pub fn create_from_memory(
        &self,
        driver_name: &str,
        memory_area: &MemoryArea,
    ) -> Result<Arc<dyn crate::fs::vfs_v2::core::FileSystemOperations>, FileSystemError> {
        let binding = self.drivers.read();
        let driver = binding.get(driver_name).ok_or(FileSystemError {
            kind: FileSystemErrorKind::NotFound,
            message: format!("File system driver '{}' not found", driver_name),
        })?;

        if driver.filesystem_type() == FileSystemType::Block {
            return Err(FileSystemError {
                kind: FileSystemErrorKind::NotSupported,
                message: format!("File system driver '{}' does not support memory-based filesystems", driver_name),
            });
        }

        driver.create_from_memory(memory_area)
    }

    /// Create a filesystem with structured parameters
    /// 
    /// This method creates filesystems using type-safe structured parameters that
    /// implement the FileSystemParams trait. This approach replaces the old BTreeMap<String, String>
    /// configuration method with better type safety and validation.
    /// 
    /// The method uses dynamic dispatch to handle different parameter types, enabling
    /// future dynamic filesystem module loading while maintaining type safety at the
    /// driver level.
    /// 
    /// # Arguments
    /// 
    /// * `driver_name` - The name of the registered driver to use
    /// * `params` - Parameter structure implementing FileSystemParams
    /// 
    /// # Returns
    /// 
    /// * `Ok(Arc<dyn FileSystemOperations>)` - Successfully created filesystem instance
    /// * `Err(FileSystemError)` - If driver not found, parameters invalid, or creation fails
    /// 
    /// # Errors
    /// 
    /// - `NotFound` - Driver with the specified name is not registered
    /// - `NotSupported` - Driver doesn't support the provided parameter type
    /// - Driver-specific parameter validation errors
    /// 
    /// # Example
    /// 
    /// ```rust
    /// use crate::fs::params::TmpFSParams;
    /// 
    /// let params = TmpFSParams::new(1048576, 0); // 1MB limit
    /// let fs = manager.create_with_params("tmpfs", &params)?;
    /// ```
    /// 
    /// # Note
    /// 
    /// This method uses dynamic dispatch for parameter handling to support
    /// future dynamic filesystem module loading while maintaining type safety.
    pub fn create_from_params(
        &self, 
        driver_name: &str, 
        params: &dyn crate::fs::params::FileSystemParams
    ) -> Result<Arc<dyn crate::fs::vfs_v2::core::FileSystemOperations>, FileSystemError> {
        let binding = self.drivers.read();
        let driver = binding.get(driver_name)
            .ok_or_else(|| FileSystemError {
                kind: FileSystemErrorKind::NotFound,
                message: format!("File system driver '{}' not found", driver_name),
            })?;
        driver.create_from_params(params)
    }

    /// Create a filesystem from option string
    /// 
    /// Creates a new filesystem instance using the specified driver and option string.
    /// This method delegates option parsing to the individual filesystem driver,
    /// allowing each driver to handle its own specific option format.
    /// 
    /// # Arguments
    /// 
    /// * `driver_name` - The name of the registered driver to use
    /// * `options` - Option string containing filesystem-specific parameters
    /// 
    /// # Returns
    /// 
    /// * `Ok(Arc<dyn FileSystemOperations>)` - Successfully created filesystem instance
    /// * `Err(FileSystemError)` - If driver not found or creation fails
    /// 
    /// # Errors
    /// 
    /// - `NotFound` - Driver with the specified name is not registered
    /// - Driver-specific option parsing or creation errors
    /// 
    /// # Example
    /// 
    /// ```rust
    /// let fs = manager.create_from_option_string("tmpfs", "size=64M")?;
    /// let fs = manager.create_from_option_string("overlay", "upperdir=/upper,lowerdir=/lower1:/lower2")?;
    /// ```
    pub fn create_from_option_string(
        &self,
        driver_name: &str,
        options: &str,
    ) -> Result<Arc<dyn crate::fs::vfs_v2::core::FileSystemOperations>, FileSystemError> {
        let binding = self.drivers.read();
        let driver = binding.get(driver_name)
            .ok_or_else(|| FileSystemError {
                kind: FileSystemErrorKind::NotFound,
                message: format!("File system driver '{}' not found", driver_name),
            })?;

        driver.create_from_option_string(options)
    }

    /// Get filesystem driver information by name
    /// 
    /// Retrieves the filesystem type supported by a registered driver.
    /// This is useful for validating driver capabilities before attempting
    /// to create filesystems.
    /// 
    /// # Arguments
    /// 
    /// * `driver_name` - The name of the driver to query
    /// 
    /// # Returns
    /// 
    /// * `Some(FileSystemType)` - The filesystem type if driver exists
    /// * `None` - If no driver with the specified name is registered
    /// 
    /// # Example
    /// 
    /// ```rust
    /// if let Some(fs_type) = manager.get_driver_type("tmpfs") {
    ///     match fs_type {
    ///         FileSystemType::Virtual => println!("TmpFS is a virtual filesystem"),
    ///         _ => println!("Unexpected filesystem type"),
    ///     }
    /// }
    /// ```
    pub fn get_driver_type(&self, driver_name: &str) -> Option<FileSystemType> {
        self.drivers.read().get(driver_name).map(|driver| driver.filesystem_type())
    }
}

impl Default for FileSystemDriverManager {
    fn default() -> Self {
        Self::new()
    }
}