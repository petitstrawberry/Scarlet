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
//! #### Pattern 1: Independent Mount Points with Shared Content
//! ```rust
//! // Clone VfsManager to share filesystem objects but maintain separate mount points
//! let shared_vfs = original_vfs.clone();
//! 
//! // Each VfsManager maintains its own mount tree
//! // but shares the underlying filesystem drivers and inode caches
//! shared_vfs.mount("/proc", proc_fs)?;  // Only affects shared_vfs mount tree
//! 
//! // Useful for:
//! // - Container-like isolation with selective sharing
//! // - Process-specific mount namespaces
//! // - Independent filesystem views with shared storage backends
//! ```
//!
//! #### Pattern 2: Complete VFS Sharing via Arc
//! ```rust
//! // Share entire VfsManager instance including mount points
//! let shared_vfs = Arc::new(original_vfs);
//! let task_vfs = Arc::clone(&shared_vfs);
//! 
//! // All mount operations affect the shared mount tree
//! shared_vfs.mount("/tmp", tmpfs)?;  // Visible to all references
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
pub mod tmpfs;
pub mod params;
pub mod mount_tree;

use alloc::{boxed::Box, collections::BTreeMap, format, string::{String, ToString}, sync::Arc, vec::Vec};
use alloc::vec;
use core::fmt;
use crate::{device::{block::{request::{BlockIORequest, BlockIORequestType}, BlockDevice}, DeviceType}, task::Task, vm::vmem::MemoryArea};

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
    ReadOnly,
    DeviceError,
    NotSupported,
    BrokenFileSystem,
    Busy,
}

pub struct FileSystemError {
    pub kind: FileSystemErrorKind,
    pub message: String,
}

impl fmt::Debug for FileSystemError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "FileSystemError {{ kind: {:?}, message: {} }}", self.kind, self.message)
    }
}

/// Result type for file system operations
pub type Result<T> = core::result::Result<T, FileSystemError>;

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
}

#[derive(Clone)]
pub struct File {
    // pub path: String,
    handle: Arc<dyn FileHandle>,
}
impl File {
    /// Open a file using a specific VFS manager
    /// 
    /// # Arguments
    /// 
    /// * `path` - The path to the file
    /// * `manager` - The VFS manager to use
    /// 
    /// # Returns
    /// 
    /// * `Result<File>` - The opened file object
    /// 
    pub fn open_with_manager(path: String, manager: &VfsManager) -> Result<Self> {
        manager.open(&path, 0)
    }

    /// Read data from the file
    /// 
    /// # Arguments
    /// 
    /// * `buffer` - The buffer to read data into
    /// 
    /// # Returns
    /// 
    /// * `Result<usize>` - The number of bytes read
    /// 
    pub fn read(&mut self, buffer: &mut [u8]) -> Result<usize> {
        self.handle.read(buffer)
    }
    
    /// Write data to the file
    /// 
    /// # Arguments
    /// 
    /// * `buffer` - The buffer containing data to write
    /// 
    /// # Returns
    /// 
    /// * `Result<usize>` - The number of bytes written
    /// 
    pub fn write(&mut self, buffer: &[u8]) -> Result<usize> { 
        self.handle.write(buffer)
    }
    
    /// Change the position within the file
    pub fn seek(&mut self, whence: SeekFrom) -> Result<u64> {
        self.handle.seek(whence)
    }
    
    /// Get the metadata of the file
    pub fn metadata(&self) -> Result<FileMetadata> {
        self.handle.metadata()
    }
    
    /// Get the size of the file
    pub fn size(&self) -> Result<usize> {
        let metadata = self.metadata()?;
        Ok(metadata.size)
    }
    
    /// Read the entire contents of the file
    pub fn read_all(&mut self) -> Result<Vec<u8>> {
        let size = self.size()?;
        let mut buffer = vec![0u8; size];
        
        self.seek(SeekFrom::Start(0))?;
        let read_bytes = self.read(&mut buffer)?;
        
        if read_bytes != size {
            buffer.truncate(read_bytes);
        }
        
        Ok(buffer)
    }
}

impl Drop for File {
    fn drop(&mut self) {
        self.handle.release().unwrap();
    }
}
/// Structure representing a directory entry
#[derive(Debug, Clone)]
pub struct DirectoryEntry {
    pub name: String,
    pub file_type: FileType,
    pub size: usize,
    pub metadata: Option<FileMetadata>,
}

/// Structure representing a directory
pub struct Directory<'a> {
    pub path: String,
    manager_ref: ManagerRef<'a>,  // Added
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

/// Trait for file handlers
pub trait FileHandle: Send + Sync {
    /// Read from the file
    fn read(&self, buffer: &mut [u8]) -> Result<usize>;
    
    /// Write to the file
    fn write(&self, buffer: &[u8]) -> Result<usize>;
    
    /// Move the position within the file
    fn seek(&self, whence: SeekFrom) -> Result<u64>;
    
    /// Release the file resource
    fn release(&self) -> Result<()>;
    
    /// Get the metadata
    fn metadata(&self) -> Result<FileMetadata>;
}

/// Trait defining basic file system operations
pub trait FileSystem: Send + Sync {
    /// Mount operation
    fn mount(&mut self, mount_point: &str) -> Result<()>;

    /// Unmount operation
    fn unmount(&mut self) -> Result<()>;
    
    /// Get the name of the file system
    fn name(&self) -> &str;
}

/// Trait defining file operations
pub trait FileOperations: Send + Sync {
    /// Open a file
    fn open(&self, path: &str, flags: u32) -> Result<Arc<dyn FileHandle>>;
    
    /// Read directory entries
    fn read_dir(&self, path: &str) -> Result<Vec<DirectoryEntry>>;
    
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
    /// * `Result<()>` - `Ok` if the file was created successfully, or an error if the operation failed. Errors may include `PermissionDenied`, `InvalidPath`, or `DeviceError` for device files.
    fn create_file(&self, path: &str, file_type: FileType) -> Result<()>;
    
    /// Create a directory
    fn create_dir(&self, path: &str) -> Result<()>;
    
    /// Remove a file/directory
    fn remove(&self, path: &str) -> Result<()>;
    
    /// Get the metadata
    fn metadata(&self, path: &str) -> Result<FileMetadata>;

    /// Get the root directory of the file system
    fn root_dir(&self) -> Result<Directory>;
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
    fn create_from_block(&self, _block_device: Box<dyn BlockDevice>, _block_size: usize) -> Result<Box<dyn VirtualFileSystem>> {
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
    /// * `Result<Box<dyn VirtualFileSystem>>` - The created file system
    /// 
    fn create_from_memory(&self, _memory_area: &crate::vm::vmem::MemoryArea) -> Result<Box<dyn VirtualFileSystem>> {
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

    fn create(&self) -> Result<Box<dyn VirtualFileSystem>> {
        // Default implementation that can be overridden by specific drivers
        // This is a convenience method for drivers that do not need to handle block or memory creation
        Err(FileSystemError {
            kind: FileSystemErrorKind::NotSupported,
            message: "create() not implemented for this file system driver".to_string(),
        })
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
    /// * `Result<Box<dyn VirtualFileSystem>>` - The created file system
    /// 
    /// # Note
    /// 
    /// This method uses dynamic dispatch for parameter handling to support
    /// future dynamic filesystem module loading while maintaining type safety.
    /// 
    fn create_with_params(&self, params: &dyn crate::fs::params::FileSystemParams) -> Result<Box<dyn VirtualFileSystem>> {
        // Default implementation falls back to create()
        let _ = params; // Suppress unused parameter warning
        self.create()
    }
}

// Singleton for global access to the FileSystemDriverManager
static mut FS_DRIVER_MANAGER: Option<FileSystemDriverManager> = None;

#[allow(static_mut_refs)]
pub fn get_fs_driver_manager() -> &'static mut FileSystemDriverManager {
    unsafe {
        if FS_DRIVER_MANAGER.is_none() {
            FS_DRIVER_MANAGER = Some(FileSystemDriverManager::new());
        }
        FS_DRIVER_MANAGER.as_mut().unwrap()
    }
}

/// File system driver manager responsible for managing file system drivers
/// 
/// Separates the responsibility of driver management from VfsManager,
/// handling registration, search, and creation of file systems
pub struct FileSystemDriverManager {
    /// Registered file system drivers
    drivers: RwLock<BTreeMap<String, Box<dyn FileSystemDriver>>>,
}

impl FileSystemDriverManager {
    /// Create a new file system driver manager
    pub fn new() -> Self {
        Self {
            drivers: RwLock::new(BTreeMap::new()),
        }
    }

    /// Register a file system driver
    /// 
    /// # Arguments
    /// 
    /// * `driver` - The file system driver to register
    pub fn register_driver(&mut self, driver: Box<dyn FileSystemDriver>) {
        self.drivers.write().insert(driver.name().to_string(), driver);
    }

    /// Get a list of registered driver names
    /// 
    /// # Returns
    /// 
    /// * `Vec<String>` - List of registered driver names
    pub fn list_drivers(&self) -> Vec<String> {
        self.drivers.read().keys().cloned().collect()
    }

    /// Check if a driver with the specified name is registered
    /// 
    /// # Arguments
    /// 
    /// * `driver_name` - The driver name to check
    /// 
    /// # Returns
    /// 
    /// * `bool` - true if the driver is registered
    pub fn has_driver(&self, driver_name: &str) -> bool {
        self.drivers.read().contains_key(driver_name)
    }

    /// Create a file system from a block device
    /// 
    /// # Arguments
    /// 
    /// * `driver_name` - The driver name to use
    /// * `block_device` - The block device
    /// * `block_size` - The block size
    /// 
    /// # Returns
    /// 
    /// * `Result<Box<dyn VirtualFileSystem>>` - The created file system
    /// 
    /// # Errors
    /// 
    /// * `FileSystemError` - If the driver is not found or the file system cannot be created
    pub fn create_from_block(
        &self,
        driver_name: &str,
        block_device: Box<dyn BlockDevice>,
        block_size: usize,
    ) -> Result<Box<dyn VirtualFileSystem>> {
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

    /// Create a file system from a memory area
    /// 
    /// # Arguments
    /// 
    /// * `driver_name` - The driver name to use
    /// * `memory_area` - The memory area
    /// 
    /// # Returns
    /// 
    /// * `Result<Box<dyn VirtualFileSystem>>` - The created file system
    /// 
    /// # Errors
    /// 
    /// * `FileSystemError` - If the driver is not found or the file system cannot be created
    pub fn create_from_memory(
        &self,
        driver_name: &str,
        memory_area: &MemoryArea,
    ) -> Result<Box<dyn VirtualFileSystem>> {
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

    /// Create a file system with structured parameters
    /// 
    /// This method accepts any type implementing FileSystemParams and uses
    /// dynamic dispatch to handle it. This replaces the previous generic approach
    /// to enable future dynamic filesystem module loading.
    /// 
    /// # Arguments
    /// 
    /// * `driver_name` - The name of the driver to use
    /// * `params` - Parameter structure implementing FileSystemParams
    /// 
    /// # Returns
    /// 
    /// * `Result<Box<dyn VirtualFileSystem>>` - The created file system
    /// 
    /// # Errors
    /// 
    /// * `FileSystemError` - If the driver is not found or creation fails
    /// 
    /// # Example
    /// 
    /// ```rust
    /// use crate::fs::params::TmpFSParams;
    /// 
    /// let params = TmpFSParams::new(1048576, 0); // 1MB limit, fs_id=0
    /// let fs = manager.create_with_params("tmpfs", &params)?;
    /// ```
    pub fn create_with_params(
        &self, 
        driver_name: &str, 
        params: &dyn crate::fs::params::FileSystemParams
    ) -> Result<Box<dyn VirtualFileSystem>> {
        let binding = self.drivers.read();
        let driver = binding.get(driver_name)
            .ok_or_else(|| FileSystemError {
                kind: FileSystemErrorKind::NotFound,
                message: format!("File system driver '{}' not found", driver_name),
            })?;

        // Use dynamic dispatch for structured parameters
        driver.create_with_params(params)
    }

    /// Get driver information
    /// 
    /// # Arguments
    /// 
    /// * `driver_name` - The driver name
    /// 
    /// # Returns
    /// 
    /// * `Option<FileSystemType>` - The file system type of the driver
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
/// Multiple tasks can share filesystem objects while maintaining independent mount points:
/// ```rust
/// let shared_vfs = original_vfs.clone(); // Shares filesystem objects
/// let fs_index = shared_vfs.register_fs(shared_fs);
/// shared_vfs.mount(fs_index, "/mnt/shared"); // Independent mount points
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
/// The `Clone` implementation creates independent mount point namespaces while
/// sharing the underlying filesystem objects through Arc references.
pub struct VfsManager {
    filesystems: RwLock<BTreeMap<usize, FileSystemRef>>,
    mount_tree: RwLock<MountTree>,
    next_fs_id: RwLock<usize>,
}

impl VfsManager {
    pub fn new() -> Self {
        Self {
            filesystems: RwLock::new(BTreeMap::new()),
            mount_tree: RwLock::new(MountTree::new()),
            next_fs_id: RwLock::new(1), // Start from 1 to avoid zero ID
        }
    }

    /// Register a file system
    /// 
    /// # Arguments
    /// 
    /// * `fs` - The file system to register
    /// 
    /// # Returns
    /// 
    /// * `usize` - The ID of the registered file system
    /// 
    pub fn register_fs(&mut self, fs: Box<dyn VirtualFileSystem>) -> usize {
        let mut next_fs_id = self.next_fs_id.write();
        let fs_id = *next_fs_id;
        *next_fs_id += 1;
        
        // Do not set ID on filesystem - VfsManager manages it
        let fs_ref = Arc::new(RwLock::new(fs));
        self.filesystems.write().insert(fs_id, fs_ref);
        
        fs_id
    }

    /// Create and register a block-based file system by specifying the driver name
    /// 
    /// # Arguments
    /// 
    /// * `driver_name` - The name of the file system driver
    /// * `block_device` - The block device to use
    /// * `block_size` - The block size of the device
    /// 
    /// # Returns
    /// 
    /// * `Result<usize>` - The ID of the registered file system
    /// 
    /// # Errors
    /// 
    /// * `FileSystemError` - If the driver is not found or if the file system cannot be created
    /// 
    pub fn create_and_register_block_fs(
        &mut self,
        driver_name: &str,
        block_device: Box<dyn BlockDevice>,
        block_size: usize,
    ) -> Result<usize> {
        
        // Create the file system using the driver manager
        let fs = get_fs_driver_manager().create_from_block(driver_name, block_device, block_size)?;

        Ok(self.register_fs(fs))
    }

    /// Create and register a memory-based file system by specifying the driver name
    /// 
    /// # Arguments
    /// 
    /// * `driver_name` - The name of the file system driver
    /// * `memory_area` - The memory area containing the filesystem data
    /// 
    /// # Returns
    /// 
    /// * `Result<usize>` - The ID of the registered file system
    /// 
    /// # Errors
    /// 
    /// * `FileSystemError` - If the driver is not found or if the file system cannot be created
    /// 
    pub fn create_and_register_memory_fs(
        &mut self,
        driver_name: &str,
        memory_area: &crate::vm::vmem::MemoryArea,
    ) -> Result<usize> {
        
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
    /// * `Result<usize>` - The ID of the registered file system
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
    /// let params = TmpFSParams::new(1048576, 42); // 1MB limit, fs_id=42
    /// let fs_id = manager.create_and_register_fs_with_params("tmpfs", &params)?;
    /// ```
    pub fn create_and_register_fs_with_params(
        &mut self,
        driver_name: &str,
        params: &dyn crate::fs::params::FileSystemParams,
    ) -> Result<usize> {
        
        // Create the file system using the driver manager with structured parameters
        let fs = get_fs_driver_manager().create_with_params(driver_name, params)?;

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
    /// * `Result<()>` - Ok if the mount was successful, Err if there was an error
    /// 
    pub fn mount(&mut self, fs_id: usize, mount_point: &str) -> Result<()> {
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
    /// * `Result<()>` - Ok if the unmount was successful, Err if there was an error
    /// 
    pub fn unmount(&mut self, mount_point: &str) -> Result<()> {
        // Remove the mount point from MountTree
        let mp = self.mount_tree.write().remove(mount_point)?;
    
        match &mp.mount_type {
            MountType::Bind { source_vfs: _, source_path: _, bind_type: _ } => {
                // Bind mounts do not need to unmount the underlying filesystem
                // They are just references to existing filesystems
            },
            _ => {
                // For regular mounts, we need to call unmount on the filesystem
                let mut fs_write = mp.fs.write();
                fs_write.unmount()?;

                // Return the file system to the registration list using stored fs_id
                self.filesystems.write().insert(mp.fs_id, mp.fs.clone());
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
    /// * `Result<()>` - Ok if the bind mount was successful, Err otherwise
    /// 
    /// # Example
    /// 
    /// ```rust
    /// // Bind mount /mnt/source to /mnt/target as read-only
    /// vfs_manager.bind_mount("/mnt/source", "/mnt/target", true)?;
    /// ```
    pub fn bind_mount(&mut self, source_path: &str, target_path: &str, read_only: bool) -> Result<()> {
        // Normalize the source path to prevent directory traversal
        let normalized_source_path = Self::normalize_path(source_path);
        
        // Resolve the normalized source path to get the filesystem and relative path
        let (source_fs, _source_relative_path) = self.resolve_path(&normalized_source_path)?;
        
        // Create a bind mount point entry
        let bind_type = if read_only {
            mount_tree::BindType::ReadOnly
        } else {
            mount_tree::BindType::ReadWrite
        };
        
        let mount_point_entry = TreeMountPoint {
            path: target_path.to_string(),
            fs: source_fs.clone(),
            fs_id: 0, // Special ID for bind mounts - they don't consume fs_id
            mount_type: MountType::Bind {
                source_vfs: None, // Same VFS manager
                source_path: normalized_source_path,
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
    /// * `Result<()>` - Ok if the bind mount was successful, Err otherwise
    /// 
    /// # Example
    /// 
    /// ```rust
    /// // Bind mount /data from host_vfs to /mnt/shared in container_vfs
    /// container_vfs.bind_mount_from(&host_vfs, "/data", "/mnt/shared", false)?;
    /// ```
    pub fn bind_mount_from(
        &mut self, 
        source_vfs: &Arc<VfsManager>, 
        source_path: &str, 
        target_path: &str, 
        read_only: bool
    ) -> Result<()> {
        // Normalize the source path to prevent directory traversal
        let normalized_source_path = Self::normalize_path(source_path);
        let normalized_target_path = Self::normalize_path(target_path);
        // Resolve the normalized source path in the source VFS manager
        let (source_fs, source_relative_path) = source_vfs.resolve_path(&normalized_source_path)?;
        
        let bind_type = if read_only {
            mount_tree::BindType::ReadOnly
        } else {
            mount_tree::BindType::ReadWrite
        };
        
        let mount_point_entry = TreeMountPoint {
            path: normalized_target_path,
            fs: source_fs.clone(),
            fs_id: 0, // Special ID for bind mounts
            mount_type: MountType::Bind {
                source_vfs: Some(source_vfs.clone()),
                source_path: source_relative_path,
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
    /// * `Result<()>` - Ok if the shared bind mount was successful, Err otherwise
    pub fn bind_mount_shared(&mut self, source_path: &str, target_path: &str) -> Result<()> {
        // Normalize the source path to prevent directory traversal
        let normalized_source_path = Self::normalize_path(source_path);
        
        let (source_fs, _source_relative_path) = self.resolve_path(&normalized_source_path)?;
        
        let mount_point_entry = TreeMountPoint {
            path: target_path.to_string(),
            fs: source_fs.clone(),
            fs_id: 0, // Special ID for bind mounts
            mount_type: MountType::Bind {
                source_vfs: None,
                source_path: normalized_source_path,
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
    /// * `Result<()>` - Ok if the bind mount was successful, Err otherwise
    pub fn bind_mount_shared_ref(&self, source_path: &str, target_path: &str, read_only: bool) -> Result<()> {
        // Normalize the source path to prevent directory traversal
        let normalized_source_path = Self::normalize_path(source_path);
        
        // Resolve the normalized source path to get the filesystem and relative path
        let (source_fs, _source_relative_path) = self.resolve_path(&normalized_source_path)?;
        
        // Create a bind mount point entry
        let bind_type = if read_only {
            mount_tree::BindType::ReadOnly
        } else {
            mount_tree::BindType::ReadWrite
        };
        
        let mount_point_entry = TreeMountPoint {
            path: target_path.to_string(),
            fs: source_fs.clone(),
            fs_id: 0, // Special ID for bind mounts - they don't consume fs_id
            mount_type: MountType::Bind {
                source_vfs: None, // Same VFS manager
                source_path: normalized_source_path,
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

    /// List all bind mounts in this VFS manager
    /// 
    /// # Returns
    /// 
    /// * `Vec<(String, String, bool)>` - List of (source_path, target_path, is_read_only) tuples
    pub fn list_bind_mounts(&self) -> Vec<(String, String, bool)> {
        todo!()
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
        if let Ok((mount_point, _)) = self.mount_tree.read().resolve(path) {
            // matches!(mount_point.mount_type, MountType::Bind { .. })
            false
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
    /// * `Result<T>` - The result of the function execution
    /// 
    /// # Errors
    /// 
    /// * `FileSystemError` - If no file system is mounted for the specified path
    /// 
    fn with_resolve_path<F, T>(&self, path: &str, f: F) -> Result<T>
    where
        F: FnOnce(&FileSystemRef, &str) -> Result<T>
    {
        let (fs, relative_path) = self.resolve_path(path)?;
        f(&fs, &relative_path)
    }

    /// Resolve the path to the file system and relative path
    ///
    /// # Arguments
    /// 
    /// * `path` - The path to resolve (must be absolute)
    /// 
    /// # Returns
    /// 
    /// * `Result<(FileSystemRef, String)>` - The resolved file system and relative path
    /// 
    /// # Errors
    /// 
    /// * `FileSystemError` - If no file system is mounted for the specified path
    /// 
    fn resolve_path(&self, path: &str) -> Result<(FileSystemRef, String)> {
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
    /// 
    /// * `task` - The task containing the current working directory
    /// * `path` - The relative path to convert
    /// 
    pub fn to_absolute_path(task: &Task, path: &str) -> Result<String> {
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

    
    // Open a file
    pub fn open(&self, path: &str, flags: u32) -> Result<File> {
        let handle = self.with_resolve_path(path, |fs, relative_path| fs.read().open(relative_path, flags));
        match handle {
            Ok(handle) => Ok(File { handle }),
            Err(e) => Err(e),
        }
    }
    
    // Read directory entries
    pub fn read_dir(&self, path: &str) -> Result<Vec<DirectoryEntry>> {
        self.with_resolve_path(path, |fs, relative_path| fs.read().read_dir(relative_path))
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
    /// * `Result<()>` - Ok if the file was created successfully, Err otherwise
    pub fn create_file(&self, path: &str, file_type: FileType) -> Result<()> {
        self.with_resolve_path(path, |fs, relative_path| fs.read().create_file(relative_path, file_type))
    }
    
    // Create a directory
    pub fn create_dir(&self, path: &str) -> Result<()> {
        self.with_resolve_path(path, |fs, relative_path| fs.read().create_dir(relative_path))
    }
    
    // Remove a file/directory
    pub fn remove(&self, path: &str) -> Result<()> {
        self.with_resolve_path(path, |fs, relative_path| fs.read().remove(relative_path))
    }
    
    // Get the metadata
    pub fn metadata(&self, path: &str) -> Result<FileMetadata> {
        self.with_resolve_path(path, |fs, relative_path| fs.read().metadata(relative_path))
    }

    // Create a regular file
    pub fn create_regular_file(&self, path: &str) -> Result<()> {
        self.with_resolve_path(path, |fs, relative_path| fs.read().create_file(relative_path, FileType::RegularFile))
    }
    
    /// Create a character device file
    /// 
    /// # Arguments
    /// 
    /// * `path` - The path to the device file to create
    /// * `device_info` - Information about the device
    /// 
    /// # Returns
    /// 
    /// * `Result<()>` - Ok if the device file was created successfully, Err otherwise
    pub fn create_char_device(&self, path: &str, device_info: DeviceFileInfo) -> Result<()> {
        self.create_file(path, FileType::CharDevice(device_info))
    }
    
    /// Create a block device file
    /// 
    /// # Arguments
    /// 
    /// * `path` - The path to the device file to create
    /// * `device_info` - Information about the device
    /// 
    /// # Returns
    /// 
    /// * `Result<()>` - Ok if the device file was created successfully, Err otherwise
    pub fn create_block_device(&self, path: &str, device_info: DeviceFileInfo) -> Result<()> {
        self.create_file(path, FileType::BlockDevice(device_info))
    }

    /// Create a pipe file
    /// 
    /// # Arguments
    /// 
    /// * `path` - The path to the pipe to create
    /// 
    /// # Returns
    /// 
    /// * `Result<()>` - Ok if the pipe was created successfully, Err otherwise
    pub fn create_pipe(&self, path: &str) -> Result<()> {
        self.create_file(path, FileType::Pipe)
    }

    /// Create a symbolic link
    /// 
    /// # Arguments
    /// 
    /// * `path` - The path to the symbolic link to create
    /// 
    /// # Returns
    /// 
    /// * `Result<()>` - Ok if the symbolic link was created successfully, Err otherwise
    pub fn create_symlink(&self, path: &str) -> Result<()> {
        self.create_file(path, FileType::SymbolicLink)
    }

    /// Create a socket file
    /// 
    /// # Arguments
    /// 
    /// * `path` - The path to the socket to create
    /// 
    /// # Returns
    /// 
    /// * `Result<()>` - Ok if the socket was created successfully, Err otherwise
    pub fn create_socket(&self, path: &str) -> Result<()> {
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
    /// * `Result<()>` - Ok if the device file was created successfully, Err otherwise
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
    pub fn create_device_file(&self, path: &str, device_info: DeviceFileInfo) -> Result<()> {
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
}


// Template for a basic file system implementation
pub struct GenericFileSystem {
    name: &'static str,
    block_device: Mutex<Box<dyn BlockDevice>>,
    block_size: usize,
    mounted: bool,
    mount_point: String,
}

impl GenericFileSystem {
    pub fn new(name: &'static str, block_device: Box<dyn BlockDevice>, block_size: usize) -> Self {
        Self {
            name,
            block_device: Mutex::new(block_device),
            block_size,
            mounted: false,
            mount_point: String::new(),
        }
    }
    
    fn read_block_internal(&self, block_idx: usize, buffer: &mut [u8]) -> Result<()> {
        let mut device = self.block_device.lock();
        
        // Create the request
        
        let request = Box::new(BlockIORequest {
            request_type: BlockIORequestType::Read,
            sector: block_idx,
            sector_count: 1,
            head: 0,
            cylinder: 0,
            buffer: vec![0; self.block_size],
        });
        
        // Send the request
        device.enqueue_request(request);
        
        // Get the result
        let results = device.process_requests();
        
        if results.len() != 1 {
            return Err(FileSystemError {
                kind: FileSystemErrorKind::IoError,
                message: format!("Failed to process block request for block index {}", block_idx), // Updated
            });
        }
        
        match &results[0].result {
            Ok(_) => {
                // Copy the data to the buffer
                let request = &results[0].request;
                buffer.copy_from_slice(&request.buffer);
                Ok(())
            },
            Err(msg) => Err(FileSystemError {
                kind: FileSystemErrorKind::IoError,
                message: msg.to_string(),
            }),
        }
    }
    
    fn write_block_internal(&self, block_idx: usize, buffer: &[u8]) -> Result<()> {
        let mut device = self.block_device.lock();
        
        // Create the request
        let request = Box::new(BlockIORequest {
            request_type: BlockIORequestType::Write,
            sector: block_idx,
            sector_count: 1,
            head: 0,
            cylinder: 0,
            buffer: buffer.to_vec(),
        });
        
        // Send the request
        device.enqueue_request(request);
        
        // Get the result
        let results = device.process_requests();
        
        if results.len() != 1 {
            return Err(FileSystemError {
                kind: FileSystemErrorKind::IoError,
                message: format!("Failed to process block write request for block index {}", block_idx), // Updated
            });
        }
        
        match &results[0].result {
            Ok(_) => Ok(()),
            Err(msg) => Err(FileSystemError {
                kind: FileSystemErrorKind::IoError,
                message: msg.to_string(),
            }),
        }
    }
}

impl FileSystem for GenericFileSystem {
    fn mount(&mut self, mount_point: &str) -> Result<()> {
        if self.mounted {
            return Err(FileSystemError {
                kind: FileSystemErrorKind::AlreadyExists,
                message: "File system already mounted".to_string(),
            });
        }
        self.mounted = true;
        self.mount_point = mount_point.to_string();
        Ok(())
    }

    fn unmount(&mut self) -> Result<()> {
        if !self.mounted {
            return Err(FileSystemError {
                kind: FileSystemErrorKind::NotFound,
                message: "File system not mounted".to_string(),
            });
        }
        self.mounted = false;
        self.mount_point = String::new();
        Ok(())
    }
    
    fn name(&self) -> &str {
        self.name
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
pub mod testfs;