use alloc::{boxed::Box, collections::BTreeMap, format, string::{String, ToString}, vec::Vec};
use alloc::vec;
use core::fmt;
use crate::device::block::{request::{BlockIORequest, BlockIORequestType}, BlockDevice};

use spin::Mutex;

extern crate alloc;

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
    AlreadyExists,
    NotADirectory,
    NotAFile,
    ReadOnly,
    DeviceError,
    NotSupported,
    BrokenFileSystem,
}

pub struct FileSystemError {
    pub kind: FileSystemErrorKind,
    pub message: &'static str,
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

pub struct File<'a> {
    pub path: String,
    pub fs_id: usize,
    handle: Option<Box<dyn FileHandle>>,
    is_open: bool,
    manager_ref: ManagerRef<'a>,
}
impl<'a> File<'a> {
    //// Create a new file object (use the global manager by default)
    pub fn new(path: String, fs_id: usize) -> Self {
        Self {
            path,
            fs_id,
            handle: None,
            is_open: false,
            manager_ref: ManagerRef::Global,
        }
    }
    
    /// Create a file object that uses a specific manager
    pub fn with_manager(path: String, fs_id: usize, manager: &'a mut VfsManager) -> Self {
        Self {
            path,
            fs_id,
            handle: None,
            is_open: false,
            manager_ref: ManagerRef::Local(manager),
        }
    }
    
    // Internal method to get the manager to use
    fn get_manager(&self) -> &VfsManager {
        match &self.manager_ref {
            ManagerRef::Global => get_vfs_manager(),
            ManagerRef::Local(manager) => manager,
        }
    }
    
    /// Open the file
    pub fn open(&mut self, flags: u32) -> Result<()> {
        if self.is_open {
            return Ok(());
        }
        
        let handle = self.get_manager().open(&self.path, flags)?;
        self.handle = Some(handle);
        self.is_open = true;
        Ok(())
    }
    
    /// Close the file
    pub fn close(&mut self) -> Result<()> {
        if !self.is_open {
            return Ok(());
        }
        
        if let Some(mut handle) = self.handle.take() {
            let result = handle.close();
            self.is_open = false;
            result
        } else {
            Ok(())
        }
    }
    
    /// Read data from the file
    pub fn read(&mut self, buffer: &mut [u8]) -> Result<usize> {
        if !self.is_open {
            return Err(FileSystemError {
                kind: FileSystemErrorKind::IoError,
                message: "File not open",
            });
        }
        
        if let Some(handle) = self.handle.as_mut() {
            handle.read(buffer)
        } else {
            Err(FileSystemError {
                kind: FileSystemErrorKind::IoError,
                message: "Invalid file handle",
            })
        }
    }
    
    /// Write data to the file
    pub fn write(&mut self, buffer: &[u8]) -> Result<usize> {
        if !self.is_open {
            return Err(FileSystemError {
                kind: FileSystemErrorKind::IoError,
                message: "File not open",
            });
        }
        
        if let Some(handle) = self.handle.as_mut() {
            handle.write(buffer)
        } else {
            Err(FileSystemError {
                kind: FileSystemErrorKind::IoError,
                message: "Invalid file handle",
            })
        }
    }
    
    /// Change the position within the file
    pub fn seek(&mut self, whence: SeekFrom) -> Result<u64> {
        if !self.is_open {
            return Err(FileSystemError {
                kind: FileSystemErrorKind::IoError,
                message: "File not open",
            });
        }
        
        if let Some(handle) = self.handle.as_mut() {
            handle.seek(whence)
        } else {
            Err(FileSystemError {
                kind: FileSystemErrorKind::IoError,
                message: "Invalid file handle",
            })
        }
    }
    
    /// Get the metadata of the file
    pub fn metadata(&self) -> Result<FileMetadata> {
        if !self.is_open {
            // Metadata can be obtained even if the file is not open
            return self.get_manager().metadata(&self.path);
        }
        
        if let Some(handle) = self.handle.as_ref() {
            handle.metadata()
        } else {
            Err(FileSystemError {
                kind: FileSystemErrorKind::IoError,
                message: "Invalid file handle",
            })
        }
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
        
        if (read_bytes != size) {
            buffer.truncate(read_bytes);
        }
        
        Ok(buffer)
    }
    
    /// Check if the file is open
    pub fn is_open(&self) -> bool {
        self.is_open
    }
}

impl<'a> Drop for File<'a> {
    fn drop(&mut self) {
        // Automatically close the file when dropped
        if self.is_open {
            let _ = self.close();
        }
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
    pub fs_id: usize,
    manager_ref: ManagerRef<'a>,  // Added
}

impl<'a> Directory<'a> {
    pub fn new(path: String, fs_id: usize) -> Self {
        Self {
            path,
            fs_id,
            manager_ref: ManagerRef::Global,
        }
    }
    
    pub fn with_manager(path: String, fs_id: usize, manager: &'a mut VfsManager) -> Self {
        Self {
            path,
            fs_id,
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
    fn read(&mut self, buffer: &mut [u8]) -> Result<usize>;
    
    /// Write to the file
    fn write(&mut self, buffer: &[u8]) -> Result<usize>;
    
    /// Move the position within the file
    fn seek(&mut self, whence: SeekFrom) -> Result<u64>;
    
    /// Close the file
    fn close(&mut self) -> Result<()>;
    
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

    /// Get the block size
    fn get_block_size(&self) -> usize;

    /// Read from the disk
    fn read_block(&mut self, block_idx: usize, buffer: &mut [u8]) -> Result<()>;

    /// Write to the disk
    fn write_block(&mut self, block_idx: usize, buffer: &[u8]) -> Result<()>;
}

/// Trait defining file operations
pub trait FileOperations: Send + Sync {
    /// Open a file
    fn open(&self, path: &str, flags: u32) -> Result<Box<dyn FileHandle>>;
    
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

/// Trait for file system drivers
/// 
/// This trait is used to create file systems from block devices.
/// It is not intended to be used directly by the VFS manager.
/// Instead, the VFS manager will use the `create` method to create a file system
/// from a block device.
/// 
pub trait FileSystemDriver: Send + Sync {
    fn name(&self) -> &'static str;
    fn create(&self, block_device: Box<dyn BlockDevice>, block_size: usize) -> Box<dyn VirtualFileSystem>;
}

/// Mount point information
pub struct MountPoint {
    pub path: String,
    pub fs: Box<dyn VirtualFileSystem>,
}

pub enum ManagerRef<'a> {
    Global, // Use the global manager
    Local(&'a mut VfsManager), // Use a specific manager
}

/// VFS manager
pub struct VfsManager {
    filesystems: Vec<Box<dyn VirtualFileSystem>>,
    mount_points: BTreeMap<String, MountPoint>,
    drivers: BTreeMap<String, Box<dyn FileSystemDriver>>,
    next_fs_id: usize,
}

impl VfsManager {
    pub fn new() -> Self {
        Self {
            filesystems: Vec::new(),
            mount_points: BTreeMap::new(),
            drivers: BTreeMap::new(),
            next_fs_id: 0,
        }
    }

    /// Register a file system driver
    pub fn register_fs_driver(&mut self, driver: Box<dyn FileSystemDriver>) {
        self.drivers.insert(driver.name().to_string(), driver);
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
        // Assign a unique ID to the file system
        fs.set_id(self.next_fs_id);
        self.next_fs_id += 1;
        self.filesystems.push(fs);
        // Return the ID
        self.next_fs_id - 1
    }

    /// Create and register a file system by specifying the driver name
    /// 
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
    pub fn create_and_register_fs(
        &mut self,
        driver_name: &str,
        block_device: Box<dyn BlockDevice>,
        block_size: usize,
    ) -> Result<usize> {
        let driver = self.drivers.get(driver_name).ok_or(FileSystemError {
            kind: FileSystemErrorKind::NotFound,
            message: "File system driver not found",
        })?;

        // Create the file system using the driver
        let fs = driver.create(block_device, block_size);

        Ok(self.register_fs(fs))
    }
    
    pub fn mount(&mut self, fs_id: usize, mount_point: &str) -> Result<()> {
        // Search for the specified file system by ID
        let fs_idx = self.filesystems.iter().position(|fs| fs.get_id() == fs_id)
            .ok_or(FileSystemError {
                kind: FileSystemErrorKind::NotFound,
                message: "File system not found",
            })?;
            
        // Retrieve the file system (ownership transfer)
        let mut fs = self.filesystems.remove(fs_idx);
        
        // Perform the mount operation
        fs.mount(mount_point)?;
        
        // Register the mount point
        let mount_point_entry = MountPoint {
            path: mount_point.to_string(),
            fs,
        };
        
        self.mount_points.insert(mount_point.to_string(), mount_point_entry);
        
        Ok(())
    }
    
    pub fn unmount(&mut self, mount_point: &str) -> Result<()> {
        // Search for the mount point
        let mut mp = self.mount_points.remove(mount_point)
            .ok_or(FileSystemError {
                kind: FileSystemErrorKind::NotFound,
                message: "Mount point not found",
            })?;
        
        // Perform the unmount operation
        mp.fs.unmount()?;
        
        // Return the file system to the registration list
        self.filesystems.push(mp.fs);
        
        Ok(())
    }
    
    // Get the file system from the path
    pub fn resolve_path(&self, path: &str) -> Result<(&Box<dyn VirtualFileSystem>, String)> {
        // Find the longest matching mount point
        let mut best_match = "";
        
        for (mp_path, _) in &self.mount_points {
            if path.starts_with(mp_path) && mp_path.len() > best_match.len() {
                best_match = mp_path;
            }
        }
        
        if best_match.is_empty() {
            return Err(FileSystemError {
                kind: FileSystemErrorKind::NotFound,
                message: "No filesystem mounted for this path",
            });
        }
        
        // Calculate the relative path
        let relative_path = if path == best_match {
            "/"
        } else {
            &path[best_match.len()..]
        };
        
        Ok((&self.mount_points.get(best_match).unwrap().fs, relative_path.to_string()))
    }
    
    // Open a file
    pub fn open(&self, path: &str, flags: u32) -> Result<Box<dyn FileHandle>> {
        let (fs, relative_path) = self.resolve_path(path)?;
        fs.open(&relative_path, flags)
    }
    
    // Read directory entries
    pub fn read_dir(&self, path: &str) -> Result<Vec<DirectoryEntry>> {
        let (fs, relative_path) = self.resolve_path(path)?;
        fs.read_dir(&relative_path)
    }
    
    // Create a file
    pub fn create_file(&self, path: &str) -> Result<()> {
        let (fs, relative_path) = self.resolve_path(path)?;
        fs.create_file(&relative_path)
    }
    
    // Create a directory
    pub fn create_dir(&self, path: &str) -> Result<()> {
        let (fs, relative_path) = self.resolve_path(path)?;
        fs.create_dir(&relative_path)
    }
    
    // Remove a file/directory
    pub fn remove(&self, path: &str) -> Result<()> {
        let (fs, relative_path) = self.resolve_path(path)?;
        fs.remove(&relative_path)
    }
    
    // Get the metadata
    pub fn metadata(&self, path: &str) -> Result<FileMetadata> {
        let (fs, relative_path) = self.resolve_path(path)?;
        fs.metadata(&relative_path)
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
                message: "Failed to process block request",
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
                message: msg,
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
                message: "Failed to process block write request",
            });
        }
        
        match &results[0].result {
            Ok(_) => Ok(()),
            Err(msg) => Err(FileSystemError {
                kind: FileSystemErrorKind::IoError,
                message: msg,
            }),
        }
    }
}

impl FileSystem for GenericFileSystem {
    fn mount(&mut self, mount_point: &str) -> Result<()> {
        if self.mounted {
            return Err(FileSystemError {
                kind: FileSystemErrorKind::AlreadyExists,
                message: "File system already mounted",
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
                message: "File system not mounted",
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
    
    fn get_block_size(&self) -> usize {
        self.block_size
    }

    fn read_block(&mut self, block_idx: usize, buffer: &mut [u8]) -> Result<()> {
        self.read_block_internal(block_idx, buffer)
    }

    fn write_block(&mut self, block_idx: usize, buffer: &[u8]) -> Result<()> {
        self.write_block_internal(block_idx, buffer)
    }
}

#[cfg(test)]
mod tests;