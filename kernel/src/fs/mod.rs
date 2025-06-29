//! Virtual File System (VFS) module.
//!
//! This module provides a flexible Virtual File System implementation that supports
//! per-task isolated filesystems, containerization, and bind mount functionality.
//!
//! # Architecture Overview
//!
//! The VFS architecture has evolved to support containerization, process isolation,
//! and advanced mount operations including bind mounts:
//!
//! ## VfsManager Distribution
//!
//! - **Per-Task VfsManager**: Each task can have its own isolated `VfsManager` instance
//!   stored as `Option<Arc<VfsManager>>` in the task structure
//! - **Shared Filesystems**: Multiple VfsManager instances can share underlying filesystem
//!   objects while maintaining independent mount points
//! - **Bind Mounts**: Support for mounting directories from one location to another,
//!   including cross-VFS bind mounting for container orchestration
//!
//! ## Key Components
//!
//! - `VfsManager`: Main VFS management structure supporting both isolation and sharing
//! - `FileSystemDriverManager`: Global singleton for filesystem driver registration
//! - `VirtualFileSystem`: Trait combining filesystem and file operation interfaces
//! - `MountPoint`: Associates filesystem instances with mount paths
//! - `MountTree`: Hierarchical mount tree structure supporting bind mounts
//!
//! ## Bind Mount Functionality
//!
//! The VFS provides comprehensive bind mount support for flexible directory mapping:
//!
//! ### Basic Bind Mounts
//! ```rust
//! let mut vfs = VfsManager::new();
//! // Mount a directory at another location
//! vfs.bind_mount("/source/dir", "/target/dir", false)?;
//! ```
//!
//! ### Read-Only Bind Mounts
//! ```rust
//! // Create read-only bind mount for security
//! vfs.bind_mount("/source/dir", "/readonly/dir", true)?;
//! ```
//!
//! ### Cross-VFS Bind Mounts
//! ```rust
//! // Share directories between isolated VFS instances
//! let host_vfs = Arc::new(vfs_manager);
//! container_vfs.bind_mount_from(&host_vfs, "/host/data", "/container/data", false)?;
//! ```
//!

//! ### Thread-Safe Access
//! Bind mount operations are thread-safe and can be called from system call context:
//! ```rust
//! // Use shared reference method for system calls
//! vfs_arc.bind_mount_shared_ref("/source", "/target", false)?;
//! ```
//!
//! ## Usage Patterns
//!
//! ### Container Isolation with Bind Mounts
//! ```rust
//! // Create isolated VfsManager for container
//! let mut container_vfs = VfsManager::new();
//! container_vfs.mount(fs_id, "/");
//! 
//! // Bind mount host resources into container
//! let host_vfs = Arc::new(host_vfs_manager);
//! container_vfs.bind_mount_from(&host_vfs, "/host/shared", "/shared", true)?;
//! 
//! // Assign to task
//! task.vfs = Some(Arc::new(container_vfs));
//! ```
//!
//! ### Shared Filesystem Access
//!
//! The VFS supports two distinct patterns for sharing filesystem resources:
//!
//! #### VFS Sharing via Arc
//! ```rust
//! // Share entire VfsManager instance including mount points
//! let shared_vfs = Arc::new(original_vfs);
//! let task_vfs = Arc::clone(&shared_vfs);
//! 
//! // All mount operations affect the shared mount tree
//! shared_vfs.mount(tmpfs_id, "/tmp")?;  // Visible to all references
//! 
//! // Useful for:
//! // - Fork-like behavior where child inherits parent's full filesystem view
//! // - Thread-like sharing where all threads see the same mount points
//! // - System-wide mount operations
//! ```
//!
//! The design enables flexible deployment scenarios from simple shared filesystems
//! to complete filesystem isolation with selective resource sharing for containerized
//! applications through bind mounts.

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

/// Trait defining basic file system operations
pub trait FileSystem: Send + Sync {
    /// Mount operation
    fn mount(&mut self, mount_point: &str) -> Result<(), FileSystemError>;

    /// Unmount operation
    fn unmount(&mut self) -> Result<(), FileSystemError>;

    /// Get the name of the file system
    fn name(&self) -> &str;
}

/// Trait defining file operations
pub trait FileOperations: Send + Sync {
    /// Open a file
    fn open(&self, path: &str, flags: u32) -> Result<Arc<dyn FileObject>, FileSystemError>;

    /// Read directory entries
    fn readdir(&self, path: &str) -> Result<Vec<DirectoryEntryInternal>, FileSystemError>;
    
    /// Create a file with the specified type.
    /// 
    /// # Arguments
    /// 
    /// * `path` - The path to the file to create.
    /// * `file_type` - The type of file to create. This can be a regular file, a device file, or other supported types.
    /// 
    /// # Behavior
    /// 
    /// - **Regular Files**: These are standard files used for storing data. They are created in the filesystem and can be read from or written to using standard file operations.
    /// - **Device Files**: These represent hardware devices and are typically used for interacting with device drivers. Creating a device file may involve additional steps, such as associating the file with a specific device driver or hardware resource.
    /// 
    /// # Side Effects
    /// 
    /// - Creating a device file may require elevated permissions or specific system configurations.
    /// - If a file already exists at the specified path, the function will return an error of type `FileSystemErrorKind::AlreadyExists`.
    /// 
    /// # Returns
    /// 
    /// * `Result<(), FileSystemError>` - `Ok` if the file was created successfully, or an error if the operation failed. Errors may include `PermissionDenied`, `InvalidPath`, or `DeviceError` for device files.
    fn create_file(&self, path: &str, file_type: FileType) -> Result<(), FileSystemError>;
    
    /// Create a directory
    fn create_dir(&self, path: &str) -> Result<(), FileSystemError>;

    /// Remove a file/directory
    /// 
    /// This method should handle link count management internally.
    /// For hardlinks, it decrements link_count and only removes
    /// actual file data when link_count reaches zero.
    /// For directories, implementation may require them to be empty.
    fn remove(&self, path: &str) -> Result<(), FileSystemError>;

    /// Get the metadata
    fn metadata(&self, path: &str) -> Result<FileMetadata, FileSystemError>;

    /// Truncate a file to the specified size
    /// 
    /// This method changes the size of a file to the specified length.
    /// If the new size is smaller than the current size, the file is truncated.
    /// If the new size is larger, the file is extended with zero bytes.
    /// 
    /// # Arguments
    /// 
    /// * `path` - Path to the file to truncate
    /// * `size` - New size of the file in bytes
    /// 
    /// # Returns
    /// 
    /// * `Result<(), FileSystemError>` - Ok if the file was truncated successfully
    /// 
    /// # Errors
    /// 
    /// * `FileSystemError` - If the file doesn't exist, is a directory,
    ///   or the operation is not supported by this filesystem
    fn truncate(&self, path: &str, size: u64) -> Result<(), FileSystemError> {
        let _ = (path, size);
        Err(FileSystemError {
            kind: FileSystemErrorKind::NotSupported,
            message: "Truncate not supported by this filesystem".to_string(),
        })
    }

    /// Create a hard link
    /// 
    /// Creates a new directory entry that points to the same file data
    /// as the target. Both entries will have the same file_id and the
    /// link_count will be incremented.
    /// 
    /// # Arguments
    /// 
    /// * `target_path` - Path to the existing file to link to
    /// * `link_path` - Path where the new hard link should be created
    /// 
    /// # Returns
    /// 
    /// * `Result<(), FileSystemError>` - Ok if the hard link was created successfully
    /// 
    /// # Errors
    /// 
    /// * `FileSystemError` - If the target doesn't exist, link creation fails,
    ///   or hard links are not supported by this filesystem
    fn create_hardlink(&self, target_path: &str, link_path: &str) -> Result<(), FileSystemError> {
        let _ = (target_path, link_path);
        Err(FileSystemError {
            kind: FileSystemErrorKind::NotSupported,
            message: "Hard links not supported by this filesystem".to_string(),
        })
    }

    /// Get the root directory of the file system
    fn root_dir(&self) -> Result<Directory, FileSystemError> {
        Err(FileSystemError {
            kind: FileSystemErrorKind::NotSupported,
            message: "Root directory not supported by this filesystem".to_string(),
        })
    }
}

/// Trait combining the complete VFS interface
pub trait VirtualFileSystem: FileSystem + FileOperations {}

// Automatically implement VirtualFileSystem if both FileSystem and FileOperations are implemented
impl<T: FileSystem + FileOperations> VirtualFileSystem for T {}

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
    /// * `Result<Box<dyn VirtualFileSystem>, FileSystemError>` - The created file system
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
    /// * `Result<Box<dyn VirtualFileSystem>, FileSystemError>` - The created file system
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
    /// * `Result<Box<dyn VirtualFileSystem>, FileSystemError>` - The created file system
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
    /// * `Ok(Box<dyn VirtualFileSystem>)` - Successfully created filesystem instance
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
    /// * `Ok(Box<dyn VirtualFileSystem>)` - Successfully created filesystem instance
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
    /// * `Ok(Box<dyn VirtualFileSystem>)` - Successfully created filesystem instance
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
    /// * `Ok(Box<dyn VirtualFileSystem>)` - Successfully created filesystem instance
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