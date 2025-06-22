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

pub mod drivers;
pub mod syscall;
pub mod helper;
pub mod params;
pub mod mount_tree;

use alloc::{boxed::Box, collections::BTreeMap, format, string::{String, ToString}, sync::Arc, vec::Vec};
use alloc::vec;
use core::fmt;
use crate::{device::{block::{request::{BlockIORequest, BlockIORequestType}, BlockDevice}, DeviceType}, task::Task, vm::vmem::MemoryArea};
use crate::object::capability::{StreamOps, StreamError};

use spin::{Mutex, RwLock};
use mount_tree::{MountTree, MountPoint as TreeMountPoint, MountType, MountOptions};

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
pub struct Directory<'a> {
    pub path: String,
    #[allow(dead_code)]
    manager_ref: ManagerRef<'a>,
}

impl<'a> Directory<'a> {
    pub fn open(path: String) -> Self {
        Self {
            path,
            manager_ref: ManagerRef::Global,
        }
    }
    
    pub fn open_with_manager(path: String, manager: &'a mut VfsManager) -> Self {
        Self {
            path,
            manager_ref: ManagerRef::Local(manager),
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
    fn create_from_block(&self, _block_device: Box<dyn BlockDevice>, _block_size: usize) -> Result<Box<dyn VirtualFileSystem>, FileSystemError> {
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
    fn create_from_memory(&self, _memory_area: &crate::vm::vmem::MemoryArea) -> Result<Box<dyn VirtualFileSystem>, FileSystemError> {
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

    fn create(&self) -> Result<Box<dyn VirtualFileSystem>, FileSystemError> {
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
    fn create_from_option_string(&self, options: &str) -> Result<Box<dyn VirtualFileSystem>, FileSystemError> {
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
    fn create_from_params(&self, params: &dyn crate::fs::params::FileSystemParams) -> Result<Box<dyn VirtualFileSystem>, FileSystemError> {
        // Default implementation falls back to create()
        let _ = params; // Suppress unused parameter warning
        self.create()
    }
}

// Singleton for global access to the FileSystemDriverManager
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
    ) -> Result<Box<dyn VirtualFileSystem>, FileSystemError> {
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
    ) -> Result<Box<dyn VirtualFileSystem>, FileSystemError> {
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
    ) -> Result<Box<dyn VirtualFileSystem>, FileSystemError> {
        let binding = self.drivers.read();
        let driver = binding.get(driver_name)
            .ok_or_else(|| FileSystemError {
                kind: FileSystemErrorKind::NotFound,
                message: format!("File system driver '{}' not found", driver_name),
            })?;

        // Use dynamic dispatch for structured parameters
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
    ) -> Result<Box<dyn VirtualFileSystem>, FileSystemError> {
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

pub type FileSystemRef = Arc<RwLock<Box<dyn VirtualFileSystem>>>;

pub enum ManagerRef<'a> {
    Global, // Use the global manager
    Local(&'a mut VfsManager), // Use a specific manager
}


/// VFS manager for per-task or shared filesystem management.
///
/// `VfsManager` provides flexible virtual filesystem management supporting both
/// process isolation and filesystem sharing scenarios.
///
/// # Architecture
///
/// Each `VfsManager` instance maintains:
/// - Independent mount point namespace using hierarchical MountTree
/// - Reference-counted filesystem objects that can be shared between managers
/// - Thread-safe operations via RwLock protection
/// - Security-enhanced path resolution with protection against directory traversal
///
/// # Usage Scenarios
///
/// ## 1. Container Isolation
/// Each container gets its own `VfsManager` with completely isolated mount points:
/// ```rust
/// let mut container_vfs = VfsManager::new();
/// let fs_index = container_vfs.register_fs(container_fs);
/// container_vfs.mount(fs_index, "/");
/// task.vfs = Some(Arc::new(container_vfs));
/// ```
///
/// ## 2. Shared Filesystem Access
/// Multiple tasks can share VfsManager objects using Arc:
/// ```rust
/// let shared_vfs = Arc::new(original_vfs); // Shares filesystem objects and mount points
/// let fs_index = shared_vfs.register_fs(shared_fs);
/// shared_vfs.mount(fs_index, "/mnt/shared"); // Shared mount points
/// ```
///
/// # Performance Improvements
///
/// The new MountTree implementation provides:
/// - O(log k) path resolution where k is path depth
/// - Efficient mount point hierarchy management
/// - Security-enhanced path normalization
/// - Reduced memory usage through Trie structure
///
/// # Thread Safety
///
/// All internal data structures use RwLock for thread-safe concurrent access.
/// VfsManager can be shared between threads using Arc for cases requiring
/// shared filesystem access across multiple tasks.

/// Global VFS manager instance for system-wide filesystem operations
/// 
/// This provides a unified namespace for:
/// - System directories (`/system/{abi}/`)
/// - Configuration data (`/data/config/{abi}/`)
/// - Shared resources that span across all ABIs
/// 
/// ABI modules use cross-VFS operations to overlay/bind mount from global_vfs
/// into their task-specific VFS instances.
static GLOBAL_VFS: spin::Once<Arc<VfsManager>> = spin::Once::new();

/// Initialize the global VFS with system directories
/// 
/// This should be called during kernel initialization to set up the system-wide
/// filesystem structure before any ABIs or tasks are created.
pub fn init_global_vfs() -> Result<(), FileSystemError> {
    let global_vfs = VfsManager::new();
    // Initialize the global singleton
    GLOBAL_VFS.call_once(|| Arc::new(global_vfs));
    
    Ok(())
}

/// Get reference to the global VFS instance
/// 
/// Returns the system-wide VFS manager that contains:
/// - System image directories (`/system/{abi}/`)
/// - Configuration overlay directories (`/data/config/{abi}/`)
/// - Shared directories accessible to all ABIs
/// 
/// # Panics
/// 
/// Panics if called before `init_global_vfs()` has been called.
pub fn get_global_vfs() -> &'static Arc<VfsManager> {
    GLOBAL_VFS.get().expect("Global VFS not initialized - call init_global_vfs() first")
}

pub struct VfsManager {
    filesystems: RwLock<BTreeMap<usize, FileSystemRef>>,
    mount_tree: RwLock<MountTree>,
    next_fs_id: RwLock<usize>,
}

impl VfsManager {
    /// Create a new VFS manager instance
    /// 
    /// This method creates a new VfsManager with empty filesystem registry
    /// and mount tree. Each VfsManager instance provides an isolated
    /// filesystem namespace, making it suitable for containerization and
    /// process isolation scenarios.
    /// 
    /// # Returns
    /// 
    /// * `Self` - A new VfsManager instance ready for filesystem registration and mounting
    /// 
    /// # Examples
    /// 
    /// ```rust
    /// // Create isolated VFS for a container
    /// let container_vfs = VfsManager::new();
    /// 
    /// // Create shared VFS for multiple tasks
    /// let shared_vfs = Arc::new(VfsManager::new());
    /// ```
    pub fn new() -> Self {
        Self {
            filesystems: RwLock::new(BTreeMap::new()),
            mount_tree: RwLock::new(MountTree::new()),
            next_fs_id: RwLock::new(1), // Start from 1 to avoid zero ID
        }
    }

    /// Register a filesystem with the VFS manager
    /// 
    /// This method adds a filesystem instance to the VfsManager's registry,
    /// assigning it a unique ID for future operations. The filesystem remains
    /// available for mounting until it's actually mounted on a mount point.
    /// 
    /// # Arguments
    /// 
    /// * `fs` - The filesystem instance to register (must implement VirtualFileSystem)
    /// 
    /// # Returns
    /// 
    /// * `usize` - Unique filesystem ID for use in mount operations
    /// 
    /// # Thread Safety
    /// 
    /// This method is thread-safe and can be called concurrently from multiple threads.
    /// 
    /// # Examples
    /// 
    /// ```rust
    /// // Register a block-based filesystem
    /// let device = Box::new(SomeBlockDevice::new());
    /// let fs = Box::new(SomeFileSystem::new("myfs", device, 512));
    /// let fs_id = vfs_manager.register_fs(fs);
    /// 
    /// // Later mount the filesystem
    /// vfs_manager.mount(fs_id, "/mnt")?;
    /// ```
    pub fn register_fs(&self, fs: Box<dyn VirtualFileSystem>) -> usize {
        let mut next_fs_id = self.next_fs_id.write();
        let fs_id = *next_fs_id;
        *next_fs_id += 1;
        
        // Do not set ID on filesystem - VfsManager manages it
        let fs_ref = Arc::new(RwLock::new(fs));
        self.filesystems.write().insert(fs_id, fs_ref);
        
        fs_id
    }

    /// Create and register a block device-based filesystem
    /// 
    /// This convenience method combines filesystem creation and registration in a single
    /// operation. It uses the global FileSystemDriverManager to create a filesystem
    /// from the specified block device and automatically registers it with this VfsManager.
    /// 
    /// # Arguments
    /// 
    /// * `driver_name` - The name of the registered filesystem driver to use
    /// * `block_device` - The block device that will store the filesystem data
    /// * `block_size` - The block size for I/O operations (typically 512, 1024, or 4096 bytes)
    /// 
    /// # Returns
    /// 
    /// * `Ok(usize)` - The filesystem ID assigned by this VfsManager
    /// * `Err(FileSystemError)` - If driver not found, creation fails, or registration fails
    /// 
    /// # Example
    /// 
    /// ```rust
    /// let device = get_block_device();
    /// let fs_id = vfs_manager.create_and_register_block_fs("ext4", device, 4096)?;
    /// vfs_manager.mount(fs_id, "/mnt")?;
    /// ```
    pub fn create_and_register_block_fs(
        &self,
        driver_name: &str,
        block_device: Box<dyn BlockDevice>,
        block_size: usize,
    ) -> Result<usize, FileSystemError> {
        
        // Create the file system using the driver manager
        let fs = get_fs_driver_manager().create_from_block(driver_name, block_device, block_size)?;

        Ok(self.register_fs(fs))
    }

    /// Create and register a memory-based filesystem
    /// 
    /// This convenience method combines filesystem creation and registration in a single
    /// operation for memory-based filesystems like tmpfs or initramfs. It uses the global
    /// FileSystemDriverManager to create a filesystem and automatically registers it.
    /// 
    /// # Arguments
    /// 
    /// * `driver_name` - The name of the registered filesystem driver to use
    /// * `memory_area` - The memory region containing filesystem data or available for use
    /// 
    /// # Returns
    /// 
    /// * `Ok(usize)` - The filesystem ID assigned by this VfsManager
    /// * `Err(FileSystemError)` - If driver not found, creation fails, or registration fails
    /// 
    /// # Example
    /// 
    /// ```rust
    /// let memory_area = MemoryArea::new(initramfs_addr, initramfs_size);
    /// let fs_id = vfs_manager.create_and_register_memory_fs("cpiofs", &memory_area)?;
    /// vfs_manager.mount(fs_id, "/")?;
    /// ```
    pub fn create_and_register_memory_fs(
        &self,
        driver_name: &str,
        memory_area: &crate::vm::vmem::MemoryArea,
    ) -> Result<usize, FileSystemError> {
        
        // Create the file system using the driver manager
        let fs = get_fs_driver_manager().create_from_memory(driver_name, memory_area)?;

        Ok(self.register_fs(fs))
    }

    /// Create and register a file system with structured parameters
    /// 
    /// This method allows creating file systems with structured configuration
    /// parameters. It uses dynamic dispatch to handle different parameter types,
    /// enabling future dynamic filesystem module loading.
    /// 
    /// # Arguments
    /// 
    /// * `driver_name` - The name of the file system driver
    /// * `params` - Parameter structure implementing FileSystemParams
    /// 
    /// # Returns
    /// 
    /// * `Result<usize, FileSystemError>` - The ID of the registered file system
    /// 
    /// # Errors
    /// 
    /// * `FileSystemError` - If the driver is not found or if the file system cannot be created
    /// 
    /// # Example
    /// 
    /// ```rust
    /// use crate::fs::params::TmpFSParams;
    /// 
    /// let params = TmpFSParams::with_memory_limit(1048576); // 1MB limit
    /// let fs_id = manager.create_and_register_fs_from_params("tmpfs", &params)?;
    /// ```
    pub fn create_and_register_fs_from_params(
        &self,
        driver_name: &str,
        params: &dyn crate::fs::params::FileSystemParams,
    ) -> Result<usize, FileSystemError> {
        
        // Create the file system using the driver manager with structured parameters
        let fs = get_fs_driver_manager().create_from_params(driver_name, params)?;

        Ok(self.register_fs(fs))
    }
    
    /// Mount a file system at a specified mount point  
    /// 
    /// # Arguments
    /// 
    /// * `fs_id` - The ID of the file system to mount
    /// * `mount_point` - The mount point for the file system
    /// 
    /// # Returns
    /// 
    /// * `Result<(), FileSystemError>` - Ok if the mount was successful, Err if there was an error
    /// 
    pub fn mount(&self, fs_id: usize, mount_point: &str) -> Result<(), FileSystemError> {
        let mut filesystems = self.filesystems.write();
        // Remove the file system from available pool using BTreeMap
        let fs = filesystems.remove(&fs_id)
            .ok_or(FileSystemError {
                kind: FileSystemErrorKind::NotFound,
                message: format!("File system with ID {} not found", fs_id),
            })?;
            
        {
            let mut fs_write = fs.write();
            
            // Perform the mount operation
            fs_write.mount(mount_point)?;
        }
        
        // Create mount point entry with enhanced metadata
        let mount_point_entry = TreeMountPoint {
            path: mount_point.to_string(),
            fs: fs.clone(),
            fs_id,  // Store VfsManager's ID in mount point
            mount_type: MountType::Regular,
            mount_options: MountOptions::default(),
            parent: None,
            children: Vec::new(),
            mount_time: 0, // TODO: Get actual timestamp
        };
        
        // Register with MountTree
        self.mount_tree.write().mount(mount_point, mount_point_entry)?;
        
        Ok(())
    }
    
    /// Unmount a file system from a specified mount point
    /// 
    /// # Arguments
    /// 
    /// * `mount_point` - The mount point to unmount
    /// 
    /// # Returns
    /// 
    /// * `Result<(), FileSystemError>` - Ok if the unmount was successful, Err if there was an error
    /// 
    pub fn unmount(&self, mount_point: &str) -> Result<(), FileSystemError> {
        // Remove the mount point from MountTree
        let mp = self.mount_tree.write().remove(mount_point)?;
    
        match &mp.mount_type {
            mount_tree::MountType::Bind { .. } => {
                // Bind mounts do not need to unmount the underlying filesystem
                // They are just references to existing filesystems
            },
            mount_tree::MountType::Regular => {
                // For regular mounts, we need to call unmount on the filesystem
                let mut fs_write = mp.fs.write();
                fs_write.unmount()?;

                // Return the file system to the registration list only if it has a valid fs_id
                // Overlay filesystems (fs_id = 0) are not returned to the pool
                if mp.fs_id != 0 {
                    self.filesystems.write().insert(mp.fs_id, mp.fs.clone());
                }
                // If fs_id == 0, this is likely an overlay filesystem that doesn't need
                // to be returned to the pool
            },
            mount_tree::MountType::Overlay { .. } => {
                // For overlay mounts, we need to call unmount on the overlay filesystem
                let mut fs_write = mp.fs.write();
                fs_write.unmount()?;

                // Overlay filesystems use fs_id = 0 and are not returned to the pool
                // They are created dynamically and should be cleaned up after unmount
            }
        }
        
        Ok(())
    }

    /// Bind mount a source path to a target path
    /// 
    /// This creates a bind mount where the target path will provide access to the same
    /// content as the source path. The bind mount can be read-only or read-write.
    /// 
    /// # Arguments
    /// 
    /// * `source_path` - The source path to bind from
    /// * `target_path` - The target mount point where the source will be accessible
    /// * `read_only` - Whether the bind mount should be read-only
    /// 
    /// # Returns
    /// 
    /// * `Result<(), FileSystemError>` - Ok if the bind mount was successful, Err otherwise
    /// 
    /// # Example
    /// 
    /// ```rust
    /// // Bind mount /mnt/source to /mnt/target as read-only
    /// vfs_manager.bind_mount("/mnt/source", "/mnt/target", true)?;
    /// ```
    pub fn bind_mount(&self, source_path: &str, target_path: &str, read_only: bool) -> Result<(), FileSystemError> {
        let normalized_source = Self::normalize_path(source_path);
        let normalized_target = Self::normalize_path(target_path);

        // Get the source MountNode
        let mount_tree = self.mount_tree.read();
        let (source_mount_node, source_relative_path) = mount_tree.resolve(&normalized_source)?;
        drop(mount_tree);
        
        // Get the source filesystem (for caching)
        let source_mount_point = source_mount_node.get_mount_point()?;
        let source_fs = source_mount_point.fs.clone();
        
        // Create the bind mount point
        let bind_mount_point = mount_tree::MountPoint {
            path: normalized_target.clone(),
            fs: source_fs,
            fs_id: 0, // Special ID for bind mounts
            mount_type: mount_tree::MountType::Bind {
                source_mount_node,
                source_relative_path,
                bind_type: if read_only { mount_tree::BindType::ReadOnly } else { mount_tree::BindType::ReadWrite },
            },
            mount_options: mount_tree::MountOptions {
                read_only,
                ..Default::default()
            },
            parent: None,
            children: Vec::new(),
            mount_time: 0, // TODO: actual timestamp
        };
        
        // Insert the bind mount into the MountTree
        self.mount_tree.write().insert(&normalized_target, bind_mount_point)?;
        
        Ok(())
    }

    /// Bind mount from another VFS manager
    /// 
    /// This creates a bind mount where the target path in this VFS manager
    /// will provide access to content from a different VFS manager instance.
    /// This is useful for sharing filesystem content between containers.
    /// 
    /// # Arguments
    /// 
    /// * `source_vfs` - The source VFS manager containing the source path
    /// * `source_path` - The source path in the source VFS manager
    /// * `target_path` - The target mount point in this VFS manager
    /// * `read_only` - Whether the bind mount should be read-only
    /// 
    /// # Returns
    /// 
    /// * `Result<(), FileSystemError>` - Ok if the bind mount was successful, Err otherwise
    /// 
    /// # Example
    /// 
    /// ```rust
    /// // Bind mount /data from host_vfs to /mnt/shared in container_vfs
    /// container_vfs.bind_mount_from(&host_vfs, "/data", "/mnt/shared", false)?;
    /// ```
    pub fn bind_mount_from(
        &self, 
        source_vfs: &Arc<VfsManager>, 
        source_path: &str, 
        target_path: &str, 
        read_only: bool
    ) -> Result<(), FileSystemError> {
        let normalized_source = Self::normalize_path(source_path);
        let normalized_target = Self::normalize_path(target_path);
        
        // Get MountNode from source VFS
        let source_mount_tree = source_vfs.mount_tree.read();
        let (source_mount_node, source_relative_path) = source_mount_tree.resolve(&normalized_source)?;
        drop(source_mount_tree);
        
        // Get the source filesystem
        let source_mount_point = source_mount_node.get_mount_point()?;
        let source_fs = source_mount_point.fs.clone();

        // Create the bind mount point
        let bind_mount_point = mount_tree::MountPoint {
            path: normalized_target.clone(),
            fs: source_fs,
            fs_id: 0,
            mount_type: mount_tree::MountType::Bind {
                source_mount_node,
                source_relative_path,
                bind_type: if read_only { mount_tree::BindType::ReadOnly } else { mount_tree::BindType::ReadWrite },
            },
            mount_options: mount_tree::MountOptions {
                read_only,
                ..Default::default()
            },
            parent: None,
            children: Vec::new(),
            mount_time: 0,
        };

        // Insert the bind mount into the MountTree
        self.mount_tree.write().insert(&normalized_target, bind_mount_point)?;
        
        Ok(())
    }

    /// Create a shared bind mount
    /// 
    /// This creates a shared bind mount where changes to mount propagation
    /// will be shared between the source and target. This is useful for
    /// scenarios where you want mount events to propagate between namespaces.
    /// 
    /// # Arguments
    /// 
    /// * `source_path` - The source path to bind from
    /// * `target_path` - The target mount point
    /// 
    /// # Returns
    /// 
    /// * `Result<(), FileSystemError>` - Ok if the shared bind mount was successful, Err otherwise
    pub fn bind_mount_shared(&self, source_path: &str, target_path: &str) -> Result<(), FileSystemError> {
        // Normalize the source path to prevent directory traversal
        let normalized_source_path = Self::normalize_path(source_path);
        
        // Resolve the source path to get the mount node and relative path within that mount
        let (source_mount_node, source_relative_path) = self.mount_tree.read().resolve(&normalized_source_path)?;
        
        let mount_point_entry = TreeMountPoint {
            path: target_path.to_string(),
            fs: source_mount_node.get_mount_point()?.fs.clone(),
            fs_id: 0, // Special ID for bind mounts
            mount_type: MountType::Bind {
                source_mount_node,
                source_relative_path,
                bind_type: mount_tree::BindType::Shared,
            },
            mount_options: MountOptions::default(),
            parent: None,
            children: Vec::new(),
            mount_time: 0, // TODO: Get actual timestamp
        };
        
        self.mount_tree.write().mount(target_path, mount_point_entry)?;
        
        Ok(())
    }

    /// Thread-safe bind mount for use from system calls
    /// 
    /// This method can be called on a shared VfsManager (Arc<VfsManager>)
    /// from system call context where &mut self is not available.
    /// 
    /// # Arguments
    /// 
    /// * `source_path` - The source path to bind from
    /// * `target_path` - The target mount point where the source will be accessible
    /// * `read_only` - Whether the bind mount should be read-only
    /// 
    /// # Returns
    /// 
    /// * `Result<(), FileSystemError>` - Ok if the bind mount was successful, Err otherwise
    pub fn bind_mount_shared_ref(&self, source_path: &str, target_path: &str, read_only: bool) -> Result<(), FileSystemError> {
        // Normalize the source path to prevent directory traversal
        let normalized_source_path = Self::normalize_path(source_path);
        
        // Resolve the source path to get the mount node and relative path within that mount
        let (source_mount_node, source_relative_path) = self.mount_tree.read().resolve(&normalized_source_path)?;
        
        // Create a bind mount point entry
        let bind_type = if read_only {
            mount_tree::BindType::ReadOnly
        } else {
            mount_tree::BindType::ReadWrite
        };
        
        let mount_point_entry = TreeMountPoint {
            path: target_path.to_string(),
            fs: source_mount_node.get_mount_point()?.fs.clone(),
            fs_id: 0, // Special ID for bind mounts - they don't consume fs_id
            mount_type: MountType::Bind {
                source_mount_node,
                source_relative_path,
                bind_type,
            },
            mount_options: MountOptions {
                read_only,
                ..Default::default()
            },
            parent: None,
            children: Vec::new(),
            mount_time: 0, // TODO: Get actual timestamp
        };
        
        // Register with MountTree
        self.mount_tree.write().mount(target_path, mount_point_entry)?;
        
        Ok(())
    }

    /// Check if a path is a bind mount
    /// 
    /// # Arguments
    /// 
    /// * `path` - The path to check
    /// 
    /// # Returns
    /// 
    /// * `bool` - True if the path is a bind mount, false otherwise
    pub fn is_bind_mount(&self, path: &str) -> bool {
        // Use non-transparent resolution to check the mount node itself
        if let Ok((mount_node, _)) = self.mount_tree.read().resolve_non_transparent(path) {
            if let Ok(mount_point) = mount_node.get_mount_point() {
                matches!(mount_point.mount_type, MountType::Bind { .. })
            } else {
                false
            }
        } else {
            false
        }
    }

    /// Normalize a path
    /// 
    /// # Arguments
    /// 
    /// * `path` - The path to normalize
    /// 
    /// # Returns
    /// 
    /// * `String` - The normalized path
    /// 
    fn normalize_path(path: &str) -> String {
        // Remember if the path is absolute
        let is_absolute = path.starts_with('/');
        
        // Decompose and normalize the path
        let mut components = Vec::new();
        
        // Split the path into components and process them
        for component in path.split('/') {
            match component {
                "" => continue,   // Skip empty components (consecutive slashes)
                "." => continue,  // Ignore current directory
                ".." => {
                    // For parent directory, remove the previous component
                    // However, cannot go above root for absolute paths
                    if !components.is_empty() && *components.last().unwrap() != ".." {
                        components.pop();
                    } else if !is_absolute {
                        // Keep '..' for relative paths
                        components.push("..");
                    }
                },
                _ => components.push(component), // Normal directory name
            }
        }
        
        // Construct the result
        let normalized = if is_absolute {
            // Add / to the beginning for absolute paths
            format!("/{}", components.join("/"))
        } else if components.is_empty() {
            // Current directory if the result is empty for a relative path
            ".".to_string()
        } else {
            // Normal relative path
            components.join("/")
        };
        
        // Return root for empty path
        if normalized.is_empty() {
            "/".to_string()
        } else {
            normalized
        }
    }
    
    /// Execute a function with the resolved file system and path
    /// 
    /// # Arguments
    /// 
    /// * `path` - The path to resolve
    /// * `f` - The function to execute with the resolved file system and path
    /// 
    /// # Returns
    /// 
    /// * `Result<T, FileSystemError>` - The result of the function execution
    /// 
    /// # Errors
    /// 
    /// * `FileSystemError` - If no file system is mounted for the specified path
    /// 
    fn with_resolve_path<F, T>(&self, path: &str, f: F) -> Result<T, FileSystemError>
    where
        F: FnOnce(&FileSystemRef, &str) -> Result<T, FileSystemError>
    {
        let (fs, relative_path) = self.resolve_path(path)?;
        f(&fs, &relative_path)
    }

    /// Resolve the path to the file system and relative path
    /// 
    /// This method performs path resolution within the VfsManager's mount tree,
    /// handling bind mounts, security validation, and path normalization.
    /// 
    /// # Path Resolution Process
    /// 
    /// 1. **Path Normalization**: Remove `.` and `..` components, validate against directory traversal
    /// 2. **Mount Point Lookup**: Find the most specific mount point for the given path
    /// 3. **Bind Mount Resolution**: Transparently handle bind mounts by resolving to source
    /// 4. **Relative Path Calculation**: Calculate the path relative to the filesystem root
    /// 
    /// # Arguments
    /// 
    /// * `path` - The absolute path to resolve (must start with `/`)
    /// 
    /// # Returns
    /// 
    /// * `Result<(FileSystemRef, String), FileSystemError>` - Tuple containing:
    ///   - `FileSystemRef`: Arc-wrapped filesystem that handles this path
    ///   - `String`: Path relative to the filesystem root (always starts with `/`)
    /// 
    /// # Errors
    /// 
    /// * `FileSystemErrorKind::NotFound` - No filesystem mounted for the path
    /// * `FileSystemErrorKind::InvalidPath` - Path validation failed (e.g., directory traversal attempt)
    /// 
    /// # Examples
    /// 
    /// ```rust
    /// // Mount filesystem at /mnt
    /// let fs_id = vfs.register_fs(filesystem);
    /// vfs.mount(fs_id, "/mnt")?;
    /// 
    /// // Resolve paths
    /// let (fs, rel_path) = vfs.resolve_path("/mnt/dir/file.txt")?;
    /// assert_eq!(rel_path, "/dir/file.txt");
    /// 
    /// // Bind mount example
    /// vfs.bind_mount("/mnt/data", "/data", false)?;
    /// let (fs2, rel_path2) = vfs.resolve_path("/data/file.txt")?;
    /// // fs2 points to the same filesystem as fs, rel_path2 is "/data/file.txt"
    /// ```
    /// 
    /// # Security
    /// 
    /// This method includes protection against directory traversal attacks:
    /// - Normalizes `..` and `.` components
    /// - Prevents escaping mount point boundaries
    /// - Validates all path components for security
    /// # Arguments
    /// 
    /// * `path` - The path to resolve (must be absolute)
    /// 
    /// # Returns
    /// 
    /// * `Result<(FileSystemRef, String), FileSystemError>` - The resolved file system and relative path
    /// 
    /// # Errors
    /// 
    /// * `FileSystemError` - If no file system is mounted for the specified path
    /// 
    fn resolve_path(&self, path: &str) -> Result<(FileSystemRef, String), FileSystemError> {
        // Check if the path is absolute
        if !path.starts_with('/') {
            return Err(FileSystemError {
                kind: FileSystemErrorKind::InvalidPath,
                message: format!("Path must be absolute: {}", path),
            });
        }
        
        // Phase 1: Get MountNode and relative path from MountTree
        let mount_tree = self.mount_tree.read();
        let (mount_node, relative_path) = mount_tree.resolve(path)?;
        drop(mount_tree);

        // Phase 2: Get MountPoint from MountNode
        let mount_point = mount_node.get_mount_point()?;

        // Phase 3: Get filesystem and internal path from MountPoint
        mount_point.resolve_fs(&relative_path)
    }

    /// Get absolute path from relative path and current working directory
    /// 
    /// # Arguments
    /// Convert a relative path to an absolute path using the task's current working directory
    /// 
    /// This method provides path resolution for system calls that accept relative paths.
    /// It combines the task's current working directory with the relative path to
    /// create an absolute path suitable for VFS operations.
    /// 
    /// # Arguments
    /// 
    /// * `task` - The task containing the current working directory
    /// * `path` - The relative path to convert (if already absolute, returns as-is)
    /// 
    /// # Returns
    /// 
    /// * `Result<String, FileSystemError>` - The absolute path ready for VFS operations
    /// 
    /// # Examples
    /// 
    /// ```rust
    /// // If task cwd is "/home/user" and path is "documents/file.txt"
    /// let abs_path = VfsManager::to_absolute_path(&task, "documents/file.txt")?;
    /// assert_eq!(abs_path, "/home/user/documents/file.txt");
    /// 
    /// // Absolute paths are returned unchanged
    /// let abs_path = VfsManager::to_absolute_path(&task, "/etc/config")?;
    /// assert_eq!(abs_path, "/etc/config");
    /// ```
    pub fn to_absolute_path(task: &Task, path: &str) -> Result<String, FileSystemError> {
        if path.starts_with('/') {
            // If the path is already absolute, return it as is
            Ok(path.to_string())
        } else {
            let cwd = task.cwd.clone();
            if cwd.is_none() {
                return Err(FileSystemError {
                    kind: FileSystemErrorKind::InvalidPath,
                    message: "Current working directory is not set".to_string(),
                });
            }
            // Combine the current working directory and the relative path to create an absolute path
            let mut absolute_path = cwd.unwrap();
            if !absolute_path.ends_with('/') {
                absolute_path.push('/');
            }
            absolute_path.push_str(path);
            // Normalize and return the result
            Ok(Self::normalize_path(&absolute_path))
        }
    }

    /// Open a file for reading/writing
    /// 
    /// This method opens a file through the VFS layer, automatically resolving
    /// the path to the appropriate filesystem and handling mount points and
    /// bind mounts transparently. The returned KernelObject provides unified
    /// resource management and automatic cleanup through its Drop implementation.
    /// 
    /// # Arguments
    /// 
    /// * `path` - The absolute path to the file to open
    /// * `flags` - File open flags (read, write, create, etc.)
    /// 
    /// # Returns
    /// 
    /// * `Result<KernelObject, FileSystemError>` - A kernel object wrapping the file for performing I/O operations
    /// 
    /// # Errors
    /// 
    /// * `FileSystemError` - If the file cannot be opened or the path is invalid
    /// 
    /// # Examples
    /// 
    /// ```rust
    /// // Open an existing file for reading
    /// let kernel_obj = vfs.open("/etc/config.txt", 0)?;
    /// let file = kernel_obj.as_file().unwrap();
    /// 
    /// // Use the file for I/O operations
    /// let mut buffer = [0u8; 1024];
    /// let bytes_read = file.read(&mut buffer)?;
    /// 
    /// // Create and open a new file for writing
    /// let kernel_obj = vfs.open("/tmp/output.txt", O_WRONLY | O_CREATE)?;
    /// let file = kernel_obj.as_file().unwrap();
    /// ```
    pub fn open(&self, path: &str, flags: u32) -> Result<crate::object::KernelObject, FileSystemError> {
        let file_object = self.with_resolve_path(path, |fs, relative_path| fs.read().open(relative_path, flags))?;
        Ok(crate::object::KernelObject::File(file_object))
    }
    /// Read directory entries
    /// 
    /// This method reads all entries from a directory, returning a vector of
    /// directory entry structures containing file names, types, and metadata.
    /// 
    /// # Arguments
    /// 
    /// * `path` - The absolute path to the directory to read
    /// 
    /// # Returns
    /// 
    /// * `Result<Vec<DirectoryEntry>, FileSystemError>` - Vector of directory entries
    /// 
    /// # Errors
    /// 
    /// * `FileSystemError` - If the directory cannot be read or doesn't exist
    /// 
    /// # Examples
    /// 
    /// ```rust
    /// // List files in a directory
    /// let entries = vfs.readdir("/home/user")?;
    /// for entry in entries {
    ///     println!("{}: {:?}", entry.name, entry.file_type);
    /// }
    /// ```
    pub fn readdir(&self, path: &str) -> Result<Vec<DirectoryEntryInternal>, FileSystemError> {
        self.with_resolve_path(path, |fs, relative_path| fs.read().readdir(relative_path))
    }
    
    /// Create a file with specified type
    /// 
    /// # Arguments
    /// 
    /// * `path` - The path to the file to create
    /// * `file_type` - The type of file to create
    /// 
    /// # Returns
    /// 
    /// * `Result<(), FileSystemError>` - Ok if the file was created successfully, Err otherwise
    pub fn create_file(&self, path: &str, file_type: FileType) -> Result<(), FileSystemError> {
        self.with_resolve_path(path, |fs, relative_path| fs.read().create_file(relative_path, file_type))
    }
    /// Create a directory at the specified path
    /// 
    /// This method creates a new directory in the filesystem, handling
    /// parent directory creation if necessary (depending on filesystem implementation).
    /// 
    /// # Arguments
    /// 
    /// * `path` - The absolute path where the directory should be created
    /// 
    /// # Returns
    /// 
    /// * `Result<(), FileSystemError>` - Ok if the directory was created successfully
    /// 
    /// # Errors
    /// 
    /// * `FileSystemError` - If the directory cannot be created or already exists
    /// 
    /// # Examples
    /// 
    /// ```rust
    /// // Create a new directory
    /// vfs.create_dir("/tmp/new_directory")?;
    /// 
    /// // Create nested directories (if supported by filesystem)
    /// vfs.create_dir("/tmp/path/to/directory")?;
    /// ```
    pub fn create_dir(&self, path: &str) -> Result<(), FileSystemError> {
        self.with_resolve_path(path, |fs, relative_path| fs.read().create_dir(relative_path))
    }
    /// Remove a file or directory
    /// 
    /// This method removes a file or directory from the filesystem.
    /// For files with hard links, this decrements the link_count and only
    /// removes the actual file data when link_count reaches zero.
    /// For directories, the behavior depends on the filesystem implementation
    /// (some may require the directory to be empty).
    /// 
    /// # Arguments
    /// 
    /// * `path` - The absolute path to the file or directory to remove
    /// 
    /// # Returns
    /// 
    /// * `Result<(), FileSystemError>` - Ok if the item was removed successfully
    /// 
    /// # Errors
    /// 
    /// * `FileSystemError` - If the item cannot be removed or doesn't exist
    /// 
    /// # Examples
    /// 
    /// ```rust
    /// // Remove a file (or decrement hard link count)
    /// vfs.remove("/tmp/old_file.txt")?;
    /// 
    /// // Remove a directory
    /// vfs.remove("/tmp/empty_directory")?;
    /// ```
    pub fn remove(&self, path: &str) -> Result<(), FileSystemError> {
        self.with_resolve_path(path, |fs, relative_path| fs.read().remove(relative_path))
    }

    /// Truncate a file to the specified size
    /// 
    /// This method changes the size of a file to the specified length.
    /// If the new size is smaller than the current size, the file is truncated.
    /// If the new size is larger, the file is extended with zero bytes.
    /// 
    /// # Arguments
    /// 
    /// * `path` - The absolute path to the file to truncate
    /// * `size` - New size of the file in bytes
    /// 
    /// # Returns
    /// 
    /// * `Result<(), FileSystemError>` - Ok if the file was truncated successfully
    /// 
    /// # Errors
    /// 
    /// * `FileSystemError` - If the file doesn't exist, is a directory,
    ///   or the operation is not supported by the filesystem
    /// 
    /// # Examples
    /// 
    /// ```rust
    /// // Truncate a file to 1024 bytes
    /// vfs.truncate("/tmp/large_file.txt", 1024)?;
    /// 
    /// // Extend a file to 2048 bytes (fills with zeros)
    /// vfs.truncate("/tmp/small_file.txt", 2048)?;
    /// 
    /// // Truncate a file to 0 bytes (empty the file)
    /// vfs.truncate("/tmp/file.txt", 0)?;
    /// ```
    pub fn truncate(&self, path: &str, size: u64) -> Result<(), FileSystemError> {
        self.with_resolve_path(path, |fs, relative_path| fs.read().truncate(relative_path, size))
    }

    /// Get metadata about a file or directory
    /// 
    /// This method retrieves metadata information about a file or directory,
    /// including file type, size, permissions, and timestamps.
    /// 
    /// # Arguments
    /// 
    /// * `path` - The absolute path to the file or directory
    /// 
    /// # Returns
    /// 
    /// * `Result<FileMetadata, FileSystemError>` - Metadata structure containing file information
    /// 
    /// # Errors
    /// 
    /// * `FileSystemError` - If the file doesn't exist or metadata cannot be retrieved
    /// 
    /// # Examples
    /// 
    /// ```rust
    /// // Get file metadata
    /// let metadata = vfs.metadata("/etc/config.txt")?;
    /// println!("File size: {} bytes", metadata.size);
    /// println!("File type: {:?}", metadata.file_type);
    /// ```
    pub fn metadata(&self, path: &str) -> Result<FileMetadata, FileSystemError> {
        self.with_resolve_path(path, |fs, relative_path| fs.read().metadata(relative_path))
    }

    /// Create a hard link
    /// 
    /// Creates a hard link from an existing file to a new path.
    /// Both paths will refer to the same file_id and file content.
    /// The link_count will be incremented for the target file.
    /// 
    /// Hard links must be created within the same filesystem.
    /// 
    /// # Arguments
    /// 
    /// * `target_path` - Path to the existing file
    /// * `link_path` - Path where the hard link should be created
    /// 
    /// # Returns
    /// 
    /// * `Result<(), FileSystemError>` - Ok if the hard link was created successfully
    /// 
    /// # Errors
    /// 
    /// * `FileSystemError` - If the target doesn't exist, link creation fails,
    ///   or hard links are not supported by this filesystem
    /// 
    /// # Examples
    /// 
    /// ```rust
    /// // Create a hard link
    /// vfs.create_hardlink("/home/user/file.txt", "/tmp/link_to_file.txt")?;
    /// ```
    pub fn create_hardlink(&self, target_path: &str, link_path: &str) -> Result<(), FileSystemError> {
        // Resolve both paths to ensure they're on the same filesystem
        let (target_fs, target_relative) = self.resolve_path(target_path)?;
        let (link_fs, link_relative) = self.resolve_path(link_path)?;
        
        // Hard links must be on the same filesystem
        if !Arc::ptr_eq(&target_fs, &link_fs) {
            return Err(FileSystemError {
                kind: FileSystemErrorKind::NotSupported,
                message: "Hard links cannot span different filesystems".to_string(),
            });
        }
        
        target_fs.read().create_hardlink(&target_relative, &link_relative)
    }

    /// Create a regular file
    /// 
    /// This method creates a new regular file at the specified path.
    /// It's a convenience method that creates a file with FileType::RegularFile.
    /// 
    /// # Arguments
    /// 
    /// * `path` - The absolute path where the file should be created
    /// 
    /// # Returns
    /// 
    /// * `Result<(), FileSystemError>` - Ok if the file was created successfully
    /// 
    /// # Errors
    /// 
    /// * `FileSystemError` - If the file cannot be created or already exists
    /// 
    /// # Examples
    /// 
    /// ```rust
    /// // Create a new regular file
    /// vfs.create_regular_file("/tmp/new_file.txt")?;
    /// ```
    pub fn create_regular_file(&self, path: &str) -> Result<(), FileSystemError> {
        self.with_resolve_path(path, |fs, relative_path| fs.read().create_file(relative_path, FileType::RegularFile))
    }
    
    /// Create a character device file
    /// 
    /// This method creates a character device file in the filesystem.
    /// Character devices provide unbuffered access to hardware devices
    /// and are accessed through character-based I/O operations.
    /// 
    /// In Scarlet's device architecture, devices are identified by a unique
    /// device ID rather than traditional major/minor number pairs, providing
    /// a simplified and unified device identification system.
    /// 
    /// # Arguments
    /// 
    /// * `path` - The absolute path where the character device file should be created
    /// * `device_info` - Device information including unique device ID and type
    /// 
    /// # Returns
    /// 
    /// * `Result<(), FileSystemError>` - Ok if the device file was created successfully
    /// 
    /// # Errors
    /// 
    /// * `FileSystemError` - If the device file cannot be created
    /// 
    /// # Examples
    /// 
    /// ```rust
    /// // Create a character device file for /dev/tty
    /// let device_info = DeviceFileInfo {
    ///     device_id: 1,
    ///     device_type: DeviceType::Char,
    /// };
    /// vfs.create_char_device("/dev/tty", device_info)?;
    /// ```
    pub fn create_char_device(&self, path: &str, device_info: DeviceFileInfo) -> Result<(), FileSystemError> {
        self.create_file(path, FileType::CharDevice(device_info))
    }
    
    /// Create a block device file
    /// 
    /// This method creates a block device file in the filesystem.
    /// Block devices provide buffered access to hardware devices
    /// and are accessed through block-based I/O operations.
    /// 
    /// In Scarlet's device architecture, devices are identified by a unique
    /// device ID rather than traditional major/minor number pairs, enabling
    /// simplified device management and registration.
    /// 
    /// # Arguments
    /// 
    /// * `path` - The absolute path where the block device file should be created
    /// * `device_info` - Device information including unique device ID and type
    /// 
    /// # Returns
    /// 
    /// * `Result<(), FileSystemError>` - Ok if the device file was created successfully
    /// 
    /// # Errors
    /// 
    /// * `FileSystemError` - If the device file cannot be created
    /// 
    /// # Examples
    /// 
    /// ```rust
    /// // Create a block device file for /dev/sda
    /// let device_info = DeviceFileInfo {
    ///     device_id: 8,
    ///     device_type: DeviceType::Block,
    /// };
    /// vfs.create_block_device("/dev/sda", device_info)?;
    /// ```
    pub fn create_block_device(&self, path: &str, device_info: DeviceFileInfo) -> Result<(), FileSystemError> {
        self.create_file(path, FileType::BlockDevice(device_info))
    }

    /// Create a named pipe (FIFO)
    /// 
    /// This method creates a named pipe in the filesystem, which provides
    /// inter-process communication through a FIFO queue mechanism.
    /// 
    /// # Arguments
    /// 
    /// * `path` - The absolute path where the pipe should be created
    /// 
    /// # Returns
    /// 
    /// * `Result<(), FileSystemError>` - Ok if the pipe was created successfully
    /// 
    /// # Errors
    /// 
    /// * `FileSystemError` - If the pipe cannot be created
    /// 
    /// # Examples
    /// 
    /// ```rust
    /// // Create a named pipe for IPC
    /// vfs.create_pipe("/tmp/my_pipe")?;
    /// ```
    pub fn create_pipe(&self, path: &str) -> Result<(), FileSystemError> {
        self.create_file(path, FileType::Pipe)
    }

    /// Create a symbolic link
    /// 
    /// This method creates a symbolic link (symlink) in the filesystem.
    /// A symbolic link is a file that contains a reference to another file or directory.
    /// 
    /// # Arguments
    /// 
    /// * `path` - The absolute path where the symbolic link should be created
    /// 
    /// # Returns
    /// 
    /// * `Result<(), FileSystemError>` - Ok if the symbolic link was created successfully
    /// 
    /// # Errors
    /// 
    /// * `FileSystemError` - If the symbolic link cannot be created
    /// 
    /// # Examples
    /// 
    /// ```rust
    /// // Create a symbolic link
    /// vfs.create_symlink("/tmp/link_to_file")?;
    /// ```
    pub fn create_symlink(&self, path: &str) -> Result<(), FileSystemError> {
        self.create_file(path, FileType::SymbolicLink)
    }

    /// Create a socket file
    /// 
    /// This method creates a Unix domain socket file in the filesystem.
    /// Socket files provide local inter-process communication endpoints.
    /// 
    /// # Arguments
    /// 
    /// * `path` - The absolute path where the socket file should be created
    /// 
    /// # Returns
    /// 
    /// * `Result<(), FileSystemError>` - Ok if the socket file was created successfully
    /// 
    /// # Errors
    /// 
    /// * `FileSystemError` - If the socket file cannot be created
    /// 
    /// # Examples
    /// 
    /// ```rust
    /// // Create a Unix domain socket
    /// vfs.create_socket("/tmp/my_socket")?;
    /// ```
    pub fn create_socket(&self, path: &str) -> Result<(), FileSystemError> {
        self.create_file(path, FileType::Socket)
    }

    /// Create a device file of any type
    /// 
    /// This is a convenience method that automatically determines the appropriate
    /// FileType based on the DeviceType in the DeviceFileInfo.
    /// 
    /// # Arguments
    /// 
    /// * `path` - The path to the device file to create
    /// * `device_info` - Information about the device including its type
    /// 
    /// # Returns
    /// 
    /// * `Result<(), FileSystemError>` - Ok if the device file was created successfully, Err otherwise
    /// 
    /// # Example
    /// 
    /// ```rust
    /// use crate::device::{DeviceType, DeviceFileInfo};
    /// 
    /// let device_info = DeviceFileInfo {
    ///     device_id: 1,
    ///     device_type: DeviceType::Char,
    /// };
    /// 
    /// vfs_manager.create_device_file("/dev/tty0", device_info)?;
    /// ```
    pub fn create_device_file(&self, path: &str, device_info: DeviceFileInfo) -> Result<(), FileSystemError> {
        match device_info.device_type {
            crate::device::DeviceType::Char => {
                self.create_file(path, FileType::CharDevice(device_info))
            },
            crate::device::DeviceType::Block => {
                self.create_file(path, FileType::BlockDevice(device_info))
            },
            _ => {
                Err(FileSystemError {
                    kind: FileSystemErrorKind::NotSupported,
                    message: "Unsupported device type for file creation".to_string(),
                })
            },
        }
    }

    /// Get the number of mount points
    pub fn mount_count(&self) -> usize {
        self.mount_tree.read().len()
    }

    /// Check if a specific mount point exists
    pub fn has_mount_point(&self, path: &str) -> bool {
        self.mount_tree.read().resolve(path).is_ok()
    }

    /// List all mount points
    pub fn list_mount_points(&self) -> Vec<String> {
        self.mount_tree.read().list_all()
    }

    /// Create an overlay mount from multiple source VFS managers
    /// 
    /// This creates an overlay filesystem that combines multiple layers into a unified view.
    /// The overlay uses MountNode references for clean isolation and no global VFS dependency.
    /// OverlayFS is treated as a regular filesystem (MountType::Regular) for simplicity.
    /// 
    /// # Overlay Semantics
    /// 
    /// - **Upper Layer**: Read-write layer for new files and modifications (optional for read-only overlays)
    /// - **Lower Layers**: Read-only layers with decreasing priority (highest priority first)
    /// - **Copy-on-Write**: Files from lower layers are copied to upper layer when modified
    /// - **Directory Merging**: Directory listings merge all layers with upper taking precedence
    /// 
    /// # Arguments
    /// 
    /// * `upper_vfs` - VFS manager containing the upper layer (None for read-only overlay)
    /// * `upper_path` - Path within the upper VFS (ignored if upper_vfs is None)
    /// * `lower_vfs_list` - List of (VFS manager, path) tuples for lower layers (highest priority first)
    /// * `target_path` - Target mount point in this VFS
    /// 
    /// # Returns
    /// 
    /// * `Result<(), FileSystemError>` - Ok if the overlay mount was successful, Err otherwise
    /// 
    /// # Examples
    /// 
    /// ```rust
    /// // Create a read-write overlay with one lower layer
    /// container_vfs.overlay_mount_from(
    ///     Some(&upper_vfs), "/upper/data",
    ///     vec![(&base_vfs, "/base/data")],
    ///     "/overlay"
    /// )?;
    /// 
    /// // Create a read-only overlay with multiple lower layers
    /// container_vfs.overlay_mount_from(
    ///     None, "",
    ///     vec![(&layer1_vfs, "/layer1"), (&layer2_vfs, "/layer2")],
    ///     "/readonly_overlay"
    /// )?;
    /// ```
    pub fn overlay_mount_from(
        &self,
        upper_vfs: Option<&Arc<VfsManager>>,
        upper_path: &str,
        lower_vfs_list: Vec<(&Arc<VfsManager>, &str)>,
        target_path: &str,
    ) -> Result<(), FileSystemError> {
        let normalized_target = Self::normalize_path(target_path);

        // Resolve upper layer if present
        let (upper_mount_node, upper_relative_path) = if let Some(upper_vfs_ref) = upper_vfs {
            let normalized_upper = Self::normalize_path(upper_path);
            let mount_tree = upper_vfs_ref.mount_tree.read();
            let (node, relative) = mount_tree.resolve(&normalized_upper)?;
            drop(mount_tree);
            (Some(node), relative)
        } else {
            (None, String::new())
        };

        // Resolve lower layers
        let mut lower_mount_nodes = Vec::new();
        let mut lower_relative_paths = Vec::new();

        for (lower_vfs_ref, lower_path) in lower_vfs_list {
            let normalized_lower = Self::normalize_path(lower_path);
            let lower_mount_tree = lower_vfs_ref.mount_tree.read();
            let (node, relative) = lower_mount_tree.resolve(&normalized_lower)?;
            drop(lower_mount_tree);
            
            lower_mount_nodes.push(node);
            lower_relative_paths.push(relative);
        }

        // Create the OverlayFS instance
        let overlay_fs = drivers::overlayfs::OverlayFS::new(
            upper_mount_node,
            upper_relative_path,
            lower_mount_nodes,
            lower_relative_paths,
        )?;

        // Wrap the overlay filesystem in the VirtualFileSystem trait
        let overlay_fs_boxed: Box<dyn VirtualFileSystem> = Box::new(overlay_fs);
        let overlay_fs_arc = Arc::new(spin::RwLock::new(overlay_fs_boxed));

        // Create the overlay mount point as a regular filesystem
        let overlay_mount_point = mount_tree::MountPoint {
            path: normalized_target.clone(),
            fs: overlay_fs_arc,
            fs_id: 0, // Overlay filesystems don't need a registered ID
            mount_type: mount_tree::MountType::Regular,
            mount_options: mount_tree::MountOptions {
                read_only: upper_vfs.is_none(), // Read-only if no upper layer
                ..Default::default()
            },
            parent: None,
            children: Vec::new(),
            mount_time: 0,
        };

        // Insert the overlay mount into the MountTree
        self.mount_tree.write().insert(&normalized_target, overlay_mount_point)?;

        Ok(())
    }

    /// Create an overlay mount within the same VFS manager
    /// 
    /// This method creates an overlay filesystem by combining an upper directory
    /// (for writes) with one or more lower directories (read-only) within the
    /// same VFS manager. This is useful for creating overlay mounts within a
    /// single filesystem namespace.
    /// 
    /// # Arguments
    /// 
    /// * `upper_path` - Optional path to the upper directory (writable layer). If None, creates read-only overlay.
    /// * `lower_paths` - List of paths to lower directories (read-only layers), in order of priority
    /// * `target_path` - Path where the overlay should be mounted
    /// 
    /// # Returns
    /// 
    /// * `Ok(())` - Overlay mount was successful
    /// * `Err(FileSystemError)` - If any path doesn't exist, or mount operation fails
    /// 
    /// # Examples
    /// 
    /// ```rust
    /// // Create a writable overlay with one lower layer
    /// vfs_manager.overlay_mount(
    ///     Some("/upper"),
    ///     vec!["/lower"],
    ///     "/overlay"
    /// )?;
    /// 
    /// // Create a read-only overlay with multiple lower layers
    /// vfs_manager.overlay_mount(
    ///     None,
    ///     vec!["/layer1", "/layer2"],
    ///     "/readonly_overlay"
    /// )?;
    /// ```
    pub fn overlay_mount(
        &self,
        upper_path: Option<&str>,
        lower_paths: Vec<&str>,
        target_path: &str,
    ) -> Result<(), FileSystemError> {
        let normalized_target = Self::normalize_path(target_path);

        // Resolve upper layer if present
        let (upper_mount_node, upper_relative_path) = if let Some(upper_path_str) = upper_path {
            let normalized_upper = Self::normalize_path(upper_path_str);
            let mount_tree_guard = self.mount_tree.read();
            let (node, relative) = mount_tree_guard.resolve(&normalized_upper)?;
            drop(mount_tree_guard);
            (Some(node), relative)
        } else {
            (None, String::new())
        };

        // Resolve lower layers
        let mut lower_mount_nodes = Vec::new();
        let mut lower_relative_paths = Vec::new();

        for lower_path in lower_paths {
            let normalized_lower = Self::normalize_path(lower_path);
            let mount_tree_guard = self.mount_tree.read();
            let (node, relative) = mount_tree_guard.resolve(&normalized_lower)?;
            drop(mount_tree_guard);
            
            lower_mount_nodes.push(node);
            lower_relative_paths.push(relative);
        }

        // Create the OverlayFS instance
        let overlay_fs = drivers::overlayfs::OverlayFS::new(
            upper_mount_node,
            upper_relative_path,
            lower_mount_nodes,
            lower_relative_paths,
        )?;

        // Wrap the overlay filesystem in the VirtualFileSystem trait
        let overlay_fs_boxed: Box<dyn VirtualFileSystem> = Box::new(overlay_fs);
        let overlay_fs_arc = Arc::new(spin::RwLock::new(overlay_fs_boxed));

        // Create the overlay mount point as a regular filesystem
        let overlay_mount_point = mount_tree::MountPoint {
            path: normalized_target.clone(),
            fs: overlay_fs_arc,
            fs_id: 0, // Overlay filesystems don't need a registered ID
            mount_type: mount_tree::MountType::Regular,
            mount_options: mount_tree::MountOptions {
                read_only: upper_path.is_none(), // Read-only if no upper layer
                ..Default::default()
            },
            parent: None,
            children: Vec::new(),
            mount_time: 0,
        };

        // Insert the overlay mount into the MountTree
        self.mount_tree.write().insert(&normalized_target, overlay_mount_point)?;

        Ok(())
    }
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
        let copy_len = core::cmp::min(name_bytes.len(), 255); // Reserve 1 byte for null terminator
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
    pub fn name_str(&self) -> Result<&str, core::str::Utf8Error> {
        let name_bytes = &self.name[..self.name_len as usize];
        core::str::from_utf8(name_bytes)
    }

    /// Get the actual size of this entry
    pub fn entry_size(&self) -> usize {
        // Fixed size of the entry structure
        core::mem::size_of::<Self>()  as usize
    }

    /// Parse a DirectoryEntry from raw bytes
    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < core::mem::size_of::<Self>() {
            return None;
        }

        // Safety: We've checked the size above
        let entry = unsafe {
            core::ptr::read(data.as_ptr() as *const Self)
        };

        // Basic validation
        if entry.name_len as usize > 255 {
            return None;
        }

        Some(entry)
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
pub mod testfs;