pub mod drivers;
pub mod syscall;
pub mod helper;

use alloc::{boxed::Box, collections::BTreeMap, format, string::{String, ToString}, sync::Arc, vec::Vec};
use alloc::vec;
use core::fmt;
use crate::{device::block::{request::{BlockIORequest, BlockIORequestType}, BlockDevice}, task::Task};

use spin::{Mutex, RwLock};

extern crate alloc;

pub const MAX_PATH_LENGTH: usize = 1024;

// Singleton for global access to the VFS manager
static mut VFS_MANAGER: Option<VfsManager> = None;

#[allow(static_mut_refs)]
pub fn get_vfs_manager() -> &'static mut VfsManager {
    unsafe {
        if VFS_MANAGER.is_none() {
            VFS_MANAGER = Some(VfsManager::new());
        }
        VFS_MANAGER.as_mut().unwrap()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FileSystemErrorKind {
    NotFound,
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
pub enum FileType {
    RegularFile,
    Directory,
    CharDevice,
    BlockDevice,
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
    //// Open a file using the global VFS manager
    /// 
    /// # Arguments
    /// 
    /// * `path` - The path to the file
    /// 
    /// # Returns
    ///
    /// * `Result<File>` - The opened file object
    /// 
    pub fn open(path: String) -> Result<Self>{
        let handle = get_vfs_manager().open(&path, 0)?;
        Ok(Self {
            handle,
        })
    }
    
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
        let handle = manager.open(&path, 0)?;
        Ok(Self {
            handle,
        })
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
    
    fn get_manager(&self) -> &VfsManager {
        match &self.manager_ref {
            ManagerRef::Global => get_vfs_manager(),
            ManagerRef::Local(manager) => manager,
        }
    }

    pub fn read_entries(&self) -> Result<Vec<DirectoryEntry>> {
        // Read directory entries via the VFS manager
        self.get_manager().read_dir(&self.path)
    }
    
    pub fn create_file(&self, name: &str) -> Result<()> {
        let path = if self.path.ends_with('/') {
            format!("{}{}", self.path, name)
        } else {
            format!("{}/{}", self.path, name)
        };
        self.get_manager().create_file(&path)
    }
    
    pub fn create_dir(&self, name: &str) -> Result<()> {
        let path = if self.path.ends_with('/') {
            format!("{}{}", self.path, name)
        } else {
            format!("{}/{}", self.path, name)
        };
        self.get_manager().create_dir(&path)
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

    /// Set the ID of the file system
    fn set_id(&mut self, id: usize);

    /// Get the identifier of the file system
    fn get_id(&self) -> usize;
}

/// Trait defining file operations
pub trait FileOperations: Send + Sync {
    /// Open a file
    fn open(&self, path: &str, flags: u32) -> Result<Arc<dyn FileHandle>>;
    
    /// Read directory entries
    fn read_dir(&self, path: &str) -> Result<Vec<DirectoryEntry>>;
    
    /// Create a file
    fn create_file(&self, path: &str) -> Result<()>;
    
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
}

pub type FileSystemRef = Arc<RwLock<Box<dyn VirtualFileSystem>>>;

/// Mount point information
pub struct MountPoint {
    pub path: String,
    pub fs: FileSystemRef,
}

pub enum ManagerRef<'a> {
    Global, // Use the global manager
    Local(&'a mut VfsManager), // Use a specific manager
}


/// VFS manager
pub struct VfsManager {
    filesystems: RwLock<Vec<FileSystemRef>>,
    mount_points: RwLock<BTreeMap<String, MountPoint>>,
    drivers: RwLock<BTreeMap<String, Box<dyn FileSystemDriver>>>,
    next_fs_id: RwLock<usize>,
}

impl VfsManager {
    pub fn new() -> Self {
        Self {
            filesystems: RwLock::new(Vec::new()),
            mount_points: RwLock::new(BTreeMap::new()),
            drivers: RwLock::new(BTreeMap::new()),
            next_fs_id: RwLock::new(0),
        }
    }

    /// Register a file system driver
    pub fn register_fs_driver(&mut self, driver: Box<dyn FileSystemDriver>) {
        self.drivers.write().insert(driver.name().to_string(), driver);
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
    pub fn register_fs(&mut self, mut fs: Box<dyn VirtualFileSystem>) -> usize {
        let mut filesystems = self.filesystems.write();
        // Assign a unique ID to the file system
        let mut next_id = self.next_fs_id.write();
        fs.set_id(*next_id);
        // Increment the ID for the next file system
        *next_id += 1;
        let lock = Arc::new(RwLock::new(fs));
        filesystems.push(lock);
        // Return the ID
        *next_id - 1
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
        
        // Create the file system using the driver
        let fs = {
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
            
            driver.create_from_block(block_device, block_size)?
        };

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
        
        // Create the file system using the driver
        let fs = {
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
            
            driver.create_from_memory(memory_area)?
        };

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
        // Search for the specified file system by ID
        let fs_idx = filesystems.iter().position(|fs| fs.read().get_id() == fs_id)
            .ok_or(FileSystemError {
                kind: FileSystemErrorKind::NotFound,
                message: format!("File system with ID {} not found", fs_id),
            })?;
            
        // Retrieve the file system (ownership transfer)
        let fs = filesystems.remove(fs_idx);
        {
            let mut fs = fs.write();
            
            // Perform the mount operation
            fs.mount(mount_point)?;
        }
        
        // Register the mount point
        let mount_point_entry = MountPoint {
            path: mount_point.to_string(),
            fs,
        };
        
        self.mount_points.write().insert(mount_point.to_string(), mount_point_entry);
        
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
        // Search for the mount point
        let mp = self.mount_points.write().remove(mount_point)
            .ok_or(FileSystemError {
                kind: FileSystemErrorKind::NotFound,
                message: "Mount point not found".to_string(),
            })?;
    
        // Return the file system to the registration list
        self.filesystems.write().push(mp.fs);
        
        Ok(())
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
        let path = Self::normalize_path(path);
        let mut best_match = "";
        let mount_points = self.mount_points.read();
        
        // First try exact matching of mount points
        for (mp_path, _) in mount_points.iter() {
            // If there's an exact match
            if path == *mp_path {
                best_match = mp_path;
                break; // Exact match has highest priority
            }
            
            // Match at directory boundaries
            if mp_path == "/" || // Root always matches
                (path.starts_with(mp_path) && 
                mp_path.len() > best_match.len() &&
                (mp_path.len() == path.len() || path.as_bytes().get(mp_path.len()) == Some(&b'/'))) {
                best_match = mp_path;
            }
        }
        
        if best_match.is_empty() {
            return Err(FileSystemError {
                kind: FileSystemErrorKind::NotFound,
                message: format!("No filesystem mounted for path: {}", path),
            });
        }
        
        let relative_path = if path == best_match || path.len() == best_match.len() {
            // If it points to the mount point itself
            "/".to_string()
        } else {
            // For paths under the mount point, normalize the leading /
            let suffix = &path[best_match.len()..];
            format!("/{}", suffix.trim_start_matches('/'))
        };
        
        let fs = mount_points.get(best_match).unwrap().fs.clone();
        Ok((fs, relative_path))
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
    pub fn open(&self, path: &str, flags: u32) -> Result<Arc<dyn FileHandle>> {
        self.with_resolve_path(path, |fs, relative_path| fs.read().open(relative_path, flags))
    }
    
    // Read directory entries
    pub fn read_dir(&self, path: &str) -> Result<Vec<DirectoryEntry>> {
        self.with_resolve_path(path, |fs, relative_path| fs.read().read_dir(relative_path))
    }
    
    // Create a file
    pub fn create_file(&self, path: &str) -> Result<()> {
        self.with_resolve_path(path, |fs, relative_path| fs.read().create_file(relative_path))
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
}

// Template for a basic file system implementation
pub struct GenericFileSystem {
    id: usize,
    name: &'static str,
    block_device: Mutex<Box<dyn BlockDevice>>,
    block_size: usize,
    mounted: bool,
    mount_point: String,
}

impl GenericFileSystem {
    pub fn new(id: usize, name: &'static str, block_device: Box<dyn BlockDevice>, block_size: usize) -> Self {
        Self {
            id,
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

    fn set_id(&mut self, id: usize) {
        self.id = id;
    }
    
    fn get_id(&self) -> usize {
        self.id
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
pub mod testfs;