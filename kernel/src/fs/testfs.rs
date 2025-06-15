use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use spin::rwlock::RwLock;
use spin::Mutex;

use super::*;
use crate::device::manager::{BorrowedDeviceGuard, DeviceManager};
use crate::device::{Device, DeviceType, char::CharDevice, block::BlockDevice};
use crate::object::capability::{StreamOps, StreamError};

// Simple file system implementation for testing
pub struct TestFileSystem {
    name: &'static str,
    block_device: Mutex<Box<dyn BlockDevice>>,
    block_size: usize,
    mounted: bool,
    mount_point: String,
    // Simulate a simple directory structure
    directories: Mutex<Vec<(String, Vec<DirectoryEntry>)>>,
    // File ID management for hardlink support
    next_file_id: Mutex<u64>,
    file_data_table: Mutex<BTreeMap<u64, FileData>>,
}

/// Internal file data structure for hardlink management
#[derive(Debug, Clone)]
struct FileData {
    content: Vec<u8>,
    link_count: u32,
    file_type: FileType,
    metadata: FileMetadata,
}

impl TestFileSystem {
    pub fn new(name: &'static str, block_device: Box<dyn BlockDevice>, block_size: usize) -> Self {
        // Initialize the root directory
        let mut dirs = Vec::new();
        dirs.push((
            "/".to_string(),
            vec![
                DirectoryEntry {
                    name: "test.txt".to_string(),
                    file_type: FileType::RegularFile,
                    size: 10,
                    file_id: 1,
                    metadata: None,
                },
                DirectoryEntry {
                    name: "testdir".to_string(),
                    file_type: FileType::Directory,
                    size: 0,
                    file_id: 2,
                    metadata: None,
                },
            ],
        ));
        
        Self {
            name,
            block_device: Mutex::new(block_device),
            block_size,
            mounted: false,
            mount_point: String::new(),
            directories: Mutex::new(dirs),
            next_file_id: Mutex::new(3), // Start from 3 since 1,2 are used
            file_data_table: Mutex::new(BTreeMap::new()),
        }
    }
    
    /// Generate the next file ID
    fn generate_file_id(&self) -> u64 {
        let mut next_id = self.next_file_id.lock();
        let id = *next_id;
        *next_id += 1;
        id
    }
    
    // Helper method for path normalization
    fn normalize_path(&self, path: &str) -> String {
        if path.is_empty() || path == "/" {
            return "/".to_string();
        }
        
        let mut normalized = path.to_string();
        if !normalized.starts_with('/') {
            normalized = format!("/{}", normalized);
        }
        
        if normalized.ends_with('/') && normalized.len() > 1 {
            normalized.pop();
        }
        
        normalized
    }
    
    // Helper method for directory search
    fn find_directory(&self, path: &str) -> Option<Vec<DirectoryEntry>> {
        let normalized = self.normalize_path(path);
        for (dir_path, entries) in self.directories.lock().iter() {
            if *dir_path == normalized {
                return Some(entries.clone());
            }
        }
        None
    }
}

impl FileSystem for TestFileSystem {
    fn mount(&mut self, mount_point: &str) -> Result<(), FileSystemError> {
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

    fn unmount(&mut self) -> Result<(), FileSystemError> {
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

// File object for testing
struct TestFileObject {
    path: String,
    position: RwLock<u64>,
    content: RwLock<Vec<u8>>,
    file_type: FileType,
    device_guard: Option<BorrowedDeviceGuard>,
    fs: *const TestFileSystem, // Pointer to the file system for device access
}

unsafe impl Send for TestFileObject {}
unsafe impl Sync for TestFileObject {}

impl StreamOps for TestFileObject {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        // Handle device files
        if let Some(ref device_guard) = self.device_guard {
            // For device files, delegate to the device's read operation
            let device_guard_ref = device_guard.device();
            let mut device_write = device_guard_ref.write();
            
            match device_write.device_type() {
                DeviceType::Char => {
                    if let Some(char_device) = device_write.as_char_device() {
                        let mut bytes_read = 0;
                        for i in 0..buffer.len() {
                            if let Some(byte) = char_device.read_byte() {
                                buffer[i] = byte;
                                bytes_read += 1;
                            } else {
                                break; // No more data available
                            }
                        }
                        return Ok(bytes_read);
                    } else {
                        return Err(StreamError::NotSupported);
                    }
                },
                DeviceType::Block => {
                    if let Some(block_device) = device_write.as_block_device() {
                        // For block devices, read from sector 0 (simplified implementation)
                        let request = Box::new(crate::device::block::request::BlockIORequest {
                            request_type: crate::device::block::request::BlockIORequestType::Read,
                            sector: 0,
                            sector_count: 1,
                            head: 0,
                            cylinder: 0,
                            buffer: vec![0; buffer.len().min(512)],
                        });
                        
                        block_device.enqueue_request(request);
                        let results = block_device.process_requests();
                        
                        if let Some(result) = results.first() {
                            match &result.result {
                                Ok(_) => {
                                    let bytes_to_copy = buffer.len().min(result.request.buffer.len());
                                    buffer[..bytes_to_copy].copy_from_slice(&result.request.buffer[..bytes_to_copy]);
                                    return Ok(bytes_to_copy);
                                },
                                Err(_) => {
                                    return Err(StreamError::IoError);
                                }
                            }
                        }
                        return Ok(0);
                    } else {
                        return Err(StreamError::NotSupported);
                    }
                },
                _ => {
                    return Err(StreamError::NotSupported);
                }
            }
        }
        
        // Handle regular files
        let mut position = self.position.write();
        let content = self.content.read();
        if *position as usize >= content.len() {
            return Ok(0); // EOF
        }
        
        let available = content.len() - *position as usize;
        let to_read = buffer.len().min(available);
        
        buffer[..to_read].copy_from_slice(&content[*position as usize..*position as usize + to_read]);
        *position += to_read as u64;
        
        Ok(to_read)
    }

    fn write(&self, buffer: &[u8]) -> Result<usize, StreamError> {
        // Handle device files
        if let Some(ref device_guard) = self.device_guard {
            // For device files, delegate to the device's write operation
            let device_guard_ref = device_guard.device();
            let mut device_write = device_guard_ref.write();
            
            match device_write.device_type() {
                DeviceType::Char => {
                    if let Some(char_device) = device_write.as_char_device() {
                        let mut bytes_written = 0;
                        for &byte in buffer {
                            match char_device.write_byte(byte) {
                                Ok(_) => bytes_written += 1,
                                Err(_) => break, // Stop on first error
                            }
                        }
                        return Ok(bytes_written);
                    } else {
                        return Err(StreamError::NotSupported);
                    }
                },
                DeviceType::Block => {
                    if let Some(block_device) = device_write.as_block_device() {
                        // For block devices, write to sector 0 (simplified implementation)
                        let request = Box::new(crate::device::block::request::BlockIORequest {
                            request_type: crate::device::block::request::BlockIORequestType::Write,
                            sector: 0,
                            sector_count: 1,
                            head: 0,
                            cylinder: 0,
                            buffer: buffer.to_vec(),
                        });
                        
                        block_device.enqueue_request(request);
                        let results = block_device.process_requests();
                        
                        if let Some(result) = results.first() {
                            match &result.result {
                                Ok(_) => return Ok(buffer.len()),
                                Err(_) => {
                                    return Err(StreamError::IoError);
                                }
                            }
                        }
                        return Ok(0);
                    } else {
                        return Err(StreamError::NotSupported);
                    }
                },
                _ => {
                    return Err(StreamError::NotSupported);
                }
            }
        }
        
        // Handle regular files
        let mut position = self.position.write();
        let mut content = self.content.write();
        
        // Expand file size if necessary
        if *position as usize + buffer.len() > content.len() {
            content.resize(*position as usize + buffer.len(), 0);
        }
        
        // Write data
        content[*position as usize..*position as usize + buffer.len()].copy_from_slice(buffer);
        *position += buffer.len() as u64;
        
        Ok(buffer.len())
    }
}


impl FileObject for TestFileObject {
fn seek(&self, whence: SeekFrom) -> Result<u64, StreamError> {
        let mut position = self.position.write();
        match whence {
            SeekFrom::Start(offset) => {
                *position = offset;
            },
            SeekFrom::Current(offset) => {
                if offset >= 0 {
                    *position = position.saturating_add(offset as u64);
                } else {
                    *position = position.saturating_sub((-offset) as u64);
                }
            },
            SeekFrom::End(offset) => {
                let end = self.content.read().len() as u64;
                if offset >= 0 {
                    *position = end.saturating_add(offset as u64);
                } else {
                    *position = end.saturating_sub((-offset) as u64);
                }
            },
        }
        
        Ok(*position)
    }

    fn readdir(&self) -> Result<Vec<DirectoryEntry>, StreamError> {
        Err(StreamError::NotSupported)
    }    

    fn metadata(&self) -> Result<FileMetadata, StreamError> {
        Ok(FileMetadata {
            file_type: self.file_type.clone(),
            size: self.content.read().len(),
            permissions: FilePermission {
                read: true,
                write: true,
                execute: false,
            },
            created_time: 0,
            modified_time: 0,
            accessed_time: 0,
            file_id: 0, // TestFileObject doesn't know file_id
            link_count: 1,
        })
    }
}

impl FileOperations for TestFileSystem {
    fn open(&self, path: &str, _flags: u32) -> Result<Arc<dyn FileObject>, FileSystemError> {
        let normalized = self.normalize_path(path);
        
        // Simple implementation for testing (only check the beginning of the path)
        if normalized == "/test.txt" {
            return Ok(Arc::new(TestFileObject {
                path: normalized,
                position: RwLock::new(0),
                content: RwLock::new(b"Hello, world!".to_vec()),
                file_type: FileType::RegularFile,
                device_guard: None,
                fs: self as *const TestFileSystem, // Store a pointer to the file system
            }));
        }

        // Search for dynamically created files
        let parent_path = normalized.rfind('/').map_or("/", |idx| &normalized[..idx]);
        let parent_path = if parent_path.is_empty() { "/" } else { parent_path };
        let name = normalized.rfind('/').map_or(normalized.as_str(), |idx| &normalized[idx + 1..]);
        
        for (dir_path, entries) in self.directories.lock().iter() {
            if dir_path == parent_path {
                // Check if it's a regular file
                if let Some(entry) = entries.iter().find(|e| e.name == name && (e.file_type == FileType::RegularFile || e.file_type == FileType::Directory)) {
                    return Ok(Arc::new(TestFileObject {
                        path: normalized,
                        position: RwLock::new(0),
                        content: RwLock::new(Vec::new()), // Newly created files are empty
                        file_type: entry.file_type,
                        device_guard: None,
                        fs: self as *const TestFileSystem, // Store a pointer to the file system
                    }));
                }
                
                // Check if it's a device file
                if let Some(entry) = entries.iter().find(|e| e.name == name && 
                    (matches!(e.file_type, FileType::CharDevice(_)) || matches!(e.file_type, FileType::BlockDevice(_)))) {
                    
                    // Extract device ID from the FileType
                    let device_id = match entry.file_type {
                        FileType::CharDevice(ref info) | FileType::BlockDevice(ref info) => info.device_id,
                        _ => unreachable!(),
                    };
                    
                    // Try to borrow the device from DeviceManager
                    match DeviceManager::get_manager().borrow_device(device_id) {
                        Ok(guard) => {
                            return Ok(Arc::new(TestFileObject {
                                path: normalized,
                                position: RwLock::new(0),
                                content: RwLock::new(Vec::new()), // Device files don't have content
                                file_type: entry.file_type,
                                device_guard: Some(guard),
                                fs: self as *const TestFileSystem, // Store a pointer to the file system
                            }));
                        },
                        Err(_) => {
                            return Err(FileSystemError {
                                kind: FileSystemErrorKind::PermissionDenied,
                                message: "Failed to access device".to_string(),
                            });
                        }
                    }
                }
            }
        }
        
        Err(FileSystemError {
            kind: FileSystemErrorKind::NotFound,
            message: "File not found".to_string(),
        })
    }
    
    fn read_dir(&self, path: &str) -> Result<Vec<DirectoryEntry>, FileSystemError> {
        let normalized = self.normalize_path(path);
    
        // First check if the path is a file
        let parent_path = normalized.rfind('/').map_or("/", |idx| &normalized[..idx]);
        let parent_path = if parent_path.is_empty() { "/" } else { parent_path };
        let name = normalized.rfind('/').map_or(normalized.as_str(), |idx| &normalized[idx + 1..]);
        
        // Check if there is a file with the same name in the parent directory
        for (dir_path, entries) in self.directories.lock().iter_mut() {
            if dir_path == parent_path {
                if let Some(_) = entries.iter().find(|e| e.name == name && e.file_type != FileType::Directory) {
                    return Err(FileSystemError {
                        kind: FileSystemErrorKind::NotADirectory,
                        message: "Not a directory".to_string(),
                    });
                }
            }
        }

        if let Some(entries) = self.find_directory(path) {
            Ok(entries)
        } else {
            Err(FileSystemError {
                kind: FileSystemErrorKind::NotFound,
                message: "Directory not found".to_string(),
            })
        }
    }
    
    fn create_file(&self, path: &str, file_type: FileType) -> Result<(), FileSystemError> {
        let normalized = self.normalize_path(path);
        let parent_path = normalized.rfind('/').map_or("/", |idx| &normalized[..idx]);
        let parent_path = if parent_path.is_empty() { "/" } else { parent_path };
        
        let file_name = normalized.rfind('/').map_or(normalized.as_str(), |idx| &normalized[idx + 1..]);
        
        // Check if the parent directory exists
        for (dir_path, entries) in self.directories.lock().iter_mut() {
            if dir_path == parent_path {
                // Check if a file with the same name already exists
                if entries.iter().any(|e| e.name == file_name) {
                    return Err(FileSystemError {
                        kind: FileSystemErrorKind::AlreadyExists,
                        message: "File already exists".to_string(),
                    });
                }
                
                // Add the new file to the entries with specified type
                entries.push(DirectoryEntry {
                    name: file_name.to_string(),
                    file_type,
                    size: 0,
                    file_id: 0, // TestFileSystem doesn't implement proper file_id yet
                    metadata: Some(FileMetadata {
                        file_type: file_type.clone(),
                        size: 0,
                        permissions: FilePermission {
                            read: true,
                            write: true,
                            execute: false,
                        },
                        created_time: 0,
                        modified_time: 0,
                        accessed_time: 0,
                        file_id: 0,
                        link_count: 1,
                    }),
                });
                
                return Ok(());
            }
        }
        
        Err(FileSystemError {
            kind: FileSystemErrorKind::NotFound,
            message: "Parent directory not found".to_string(),
        })
    }
    
    fn create_dir(&self, path: &str) -> Result<(), FileSystemError> {
        let normalized = self.normalize_path(path);
        let parent_path = normalized.rfind('/').map_or("/", |idx| &normalized[..idx]);
        let parent_path = if parent_path.is_empty() { "/" } else { parent_path };
        let mut parent_found = false;
        
        let dir_name = normalized.rfind('/').map_or(normalized.as_str(), |idx| &normalized[idx + 1..]);
        let mut directories = self.directories.lock();

        // Check if the parent directory exists
        for (dir_path, entries) in directories.iter_mut() {
            if dir_path == parent_path {
                parent_found = true;
                // Check if a directory with the same name already exists
                if entries.iter().any(|e| e.name == dir_name) {
                    return Err(FileSystemError {
                        kind: FileSystemErrorKind::AlreadyExists,
                        message: "Directory already exists".to_string(),
                    });
                }
                
                // Add the new directory to the entries
                entries.push(DirectoryEntry {
                    name: dir_name.to_string(),
                    file_type: FileType::Directory,
                    size: 0,
                    file_id: self.generate_file_id(),
                    metadata: None,
                });
                break;
            }
        }
        if parent_found {
            // Also create the new directory structure
            directories.push((
                normalized.clone(),
                Vec::new(),
            ));
            return Ok(());
        }

        Err(FileSystemError {
            kind: FileSystemErrorKind::NotFound,
            message: "Parent directory not found".to_string(),
        })
    }
    
    fn remove(&self, path: &str) -> Result<(), FileSystemError> {
        let normalized = self.normalize_path(path);
        let parent_path = normalized.rfind('/').map_or("/", |idx| &normalized[..idx]);
        let parent_path = if parent_path.is_empty() { "/" } else { parent_path };
        let name = normalized.rfind('/').map_or(normalized.as_str(), |idx| &normalized[idx + 1..]);
        
        // First check if the target directory is empty
        let mut directories = self.directories.lock();
        
        // Find the entry corresponding to the path
        let mut parent_entries_index = None;
        let mut entry_position = None;
        let mut is_directory = false;
        let mut full_path = String::new();
        
        // 1. First collect the path and entry information
        for (i, (dir_path, entries)) in directories.iter().enumerate() {
            if dir_path == parent_path {
                parent_entries_index = Some(i);
                entry_position = entries.iter().position(|e| e.name == name);
                
                if let Some(pos) = entry_position {
                    let entry = &entries[pos];
                    is_directory = entry.file_type == FileType::Directory;
                    
                    if is_directory {
                        full_path = if parent_path == "/" {
                            format!("/{}", name)
                        } else {
                            format!("{}/{}", parent_path, name)
                        };
                    }
                }
                break;
            }
        }
        
        // 2. Perform the check and deletion based on the collected information
        if let (Some(parent_idx), Some(pos)) = (parent_entries_index, entry_position) {
            // If it is a directory, check if it is empty
            if is_directory {
                let is_empty = directories
                    .iter()
                    .find(|(p, _)| p == &full_path)
                    .map(|(_, e)| e.is_empty())
                    .unwrap_or(true);
                
                if !is_empty {
                    return Err(FileSystemError {
                        kind: FileSystemErrorKind::NotSupported,
                        message: "Cannot remove non-empty directory".to_string(),
                    });
                }
                
                // Also delete the directory entry
                if let Some(dir_idx) = directories.iter().position(|(p, _)| p == &full_path) {
                    directories.remove(dir_idx);
                }
            }
            
            // Remove the entry from the parent directory
            directories[parent_idx].1.remove(pos);
            return Ok(());
        }
        
        Err(FileSystemError {
            kind: FileSystemErrorKind::NotFound,
            message: "File or directory not found".to_string(),
        })
    }
    
    fn metadata(&self, path: &str) -> Result<FileMetadata, FileSystemError> {
        let normalized = self.normalize_path(path);
        
        // Special handling for the root directory
        if normalized == "/" {
            return Ok(FileMetadata {
                file_type: FileType::Directory,
                size: 0,
                permissions: FilePermission {
                    read: true,
                    write: true,
                    execute: true,
                },
                created_time: 0,
                modified_time: 0,
                accessed_time: 0,
                file_id: 1, // Root directory has fixed file_id = 1
                link_count: 1,
            });
        }
        
        let parent_path = normalized.rfind('/').map_or("/", |idx| &normalized[..idx]);
        let parent_path = if parent_path.is_empty() { "/" } else { parent_path };
        
        let name = normalized.rfind('/').map_or(normalized.as_str(), |idx| &normalized[idx + 1..]);
        
        for (dir_path, entries) in self.directories.lock().iter() {
            if dir_path == parent_path {
                if let Some(entry) = entries.iter().find(|e| e.name == name) {
                    return Ok(
                        entry.metadata.clone().unwrap_or(FileMetadata {
                            file_type: entry.file_type.clone(),
                            size: 0,
                            permissions: FilePermission {
                                read: true,
                                write: true,
                                execute: false,
                            },
                            created_time: 0,
                            modified_time: 0,
                            accessed_time: 0,
                            file_id: entry.file_id,
                            link_count: 1,
                        })
                    )
                }
            }
        }
        
        Err(FileSystemError {
            kind: FileSystemErrorKind::NotFound,
            message: "File or directory not found".to_string(),
        })
    }
    
    fn root_dir(&self) -> Result<Directory, FileSystemError> {
        Ok(Directory::open("/".to_string()))
    }
}

// Create a mock file system driver
pub struct TestFileSystemDriver;

impl FileSystemDriver for TestFileSystemDriver {
    fn name(&self) -> &'static str {
        "testfs"
    }
    
    fn filesystem_type(&self) -> FileSystemType {
        FileSystemType::Block  // This driver is for block device based filesystem
    }
    
    fn create_from_block(&self, block_device: Box<dyn BlockDevice>, block_size: usize) -> Result<Box<dyn VirtualFileSystem>, FileSystemError> {
        Ok(Box::new(TestFileSystem::new("testfs", block_device, block_size)))
    }
    
    fn create_with_params(&self, params: &dyn crate::fs::params::FileSystemParams) -> Result<Box<dyn VirtualFileSystem>, FileSystemError> {
        use crate::fs::params::*;
        use crate::device::block::mockblk::MockBlockDevice;
        
        // Try to downcast to BasicFSParams
        if let Some(basic_params) = params.as_any().downcast_ref::<BasicFSParams>() {
            // Create a mock block device for testing
            let block_device = Box::new(MockBlockDevice::new(1, "test_block", 512, 100));
            let block_size = basic_params.block_size.unwrap_or(512);
            return Ok(Box::new(TestFileSystem::new("testfs", block_device, block_size)));
        }
        
        // If downcast fails, return error
        Err(FileSystemError {
            kind: FileSystemErrorKind::NotSupported,
            message: "TestFS requires BasicFSParams parameter type".to_string(),
        })
    }
}

#[cfg(test)]
mod device_tests {
    use super::*;
    use crate::{device::{block::mockblk::MockBlockDevice, char::mockchar::MockCharDevice}, fs::{DeviceFileInfo, FileType}};

    #[test_case]
    fn test_device_file_char_device() {
        // Create a test character device
        let test_char_device = Box::new(MockCharDevice::new(1, "test_char"));
        
        // Register the device with DeviceManager
        let device_id = DeviceManager::get_mut_manager().register_device(test_char_device as Box<dyn Device>);

        // Create device file info
        let device_info = DeviceFileInfo {
            device_id,
            device_type: DeviceType::Char,
        };

        // Test device file creation
        let metadata = FileMetadata {
            file_type: FileType::CharDevice(device_info),
            size: 0,
            permissions: FilePermission {
                read: true,
                write: true,
                execute: false,
            },
            created_time: 0,
            modified_time: 0,
            accessed_time: 0,
            file_id: 1, // Test file_id
            link_count: 1,
        };

        assert_eq!(metadata.file_type, FileType::CharDevice(device_info));
        assert_eq!(metadata.size, 0);
        assert!(metadata.permissions.read);
        assert!(metadata.permissions.write);
    }

    #[test_case]
    fn test_device_file_block_device() {
        // Create a test block device
        let test_block_device = Box::new(MockBlockDevice::new(2, "test_block", 512, 100));
        
        // Register the device with DeviceManager
        let device_id = DeviceManager::get_mut_manager().register_device(test_block_device as Box<dyn Device>);

        // Create device file info
        let device_info = DeviceFileInfo {
            device_id,
            device_type: DeviceType::Block,
        };

        // Test device file creation
        let metadata = FileMetadata {
            file_type: FileType::BlockDevice(device_info),
            size: 0,
            permissions: FilePermission {
                read: true,
                write: true,
                execute: false,
            },
            created_time: 0,
            modified_time: 0,
            accessed_time: 0,
            file_id: 2, // Test file_id
            link_count: 1,
        };

        assert_eq!(metadata.file_type, FileType::BlockDevice(device_info));
        assert_eq!(metadata.size, 0);
        assert!(metadata.permissions.read);
        assert!(metadata.permissions.write);
    }

    #[test_case]
    fn test_device_file_through_filesystem() {
        // Create a test file system
        let block_device = Box::new(MockBlockDevice::new(3, "fs_block", 512, 100));
        let fs = TestFileSystem::new("testfs", block_device, 512);
        
        // Verify the filesystem was created properly
        assert_eq!(fs.name(), "testfs");

        // Create a test character device for device file
        let mut test_char_device = Box::new(MockCharDevice::new(4, "fs_char"));
        test_char_device.set_read_data(vec![b'T', b'e', b's', b't']);

        let device_id = DeviceManager::get_mut_manager().register_device(test_char_device as Box<dyn Device>);

        // Test device access through filesystem interface
        // This would normally be done through the VFS layer, but we can test the basic structure
        let device_info = DeviceFileInfo {
            device_id,
            device_type: DeviceType::Char,
        };

        // Verify that we can create proper device file metadata
        let device_metadata = FileMetadata {
            file_type: FileType::CharDevice(device_info),
            size: 0,
            permissions: FilePermission {
                read: true,
                write: true,
                execute: false,
            },
            created_time: 0,
            modified_time: 0,
            accessed_time: 0,
            file_id: 3, // Test file_id
            link_count: 1,
        };

        // Test that the device file info contains correct data
        if let FileType::CharDevice(info) = device_metadata.file_type {
            assert_eq!(info.device_id, device_id);
            assert_eq!(info.device_type, DeviceType::Char);
        } else {
            panic!("Expected CharDevice file type");
        }

        // Create device file in the filesystem
        fs.create_dir("/dev").unwrap();
        fs.create_file("/dev/test_char", FileType::CharDevice(device_info)).unwrap();
        // Verify that the file can be opened
        let file_handle = fs.open("/dev/test_char", 0).unwrap();
        assert!(file_handle.metadata().is_ok());
        assert_eq!(file_handle.metadata().unwrap().file_type, FileType::CharDevice(device_info));

        // Test reading from the device file
        let mut read_buffer = [0u8; 4];
        let bytes_read = file_handle.read(&mut read_buffer).unwrap();
        assert_eq!(bytes_read, 4);
        assert_eq!(&read_buffer[..bytes_read], b"Test");


    }

    #[test_case]
    fn test_serial_device_functionality() {
        // Create test character device that implements Serial
        let mut test_device = MockCharDevice::new(5, "serial_test");
        
        // Test write functionality
        test_device.set_read_data(vec![b'H', b'e', b'l', b'l', b'o']);
        
        // Test CharDevice interface
        assert_eq!(test_device.read_byte(), Some(b'H'));
        assert_eq!(test_device.read_byte(), Some(b'e'));

        test_device.write_byte(b'T').unwrap();
        test_device.write_byte(b'e').unwrap();
        test_device.write_byte(b's').unwrap();
        test_device.write_byte(b't').unwrap();

        // Check written data
        let written = test_device.get_written_data();
        assert_eq!(written, &vec![b'T', b'e', b's', b't']);
        
        // Test readiness
        assert!(test_device.can_read());
        assert!(test_device.can_write());
    }

    #[test_case]
    fn test_device_file_comprehensive_operations() {
        // Create a test file system
        let block_device = Box::new(MockBlockDevice::new(6, "test_block", 512, 100));
        let fs = TestFileSystem::new("testfs", block_device, 512);
        
        // Create test character device for comprehensive testing
        let mut test_char_device = Box::new(MockCharDevice::new(7, "comprehensive_char"));
        test_char_device.set_read_data(vec![b'H', b'e', b'l', b'l', b'o', b' ', b'W', b'o', b'r', b'l', b'd', b'!']);
        
        let device_id = DeviceManager::get_mut_manager().register_device(test_char_device as Box<dyn Device>);
        
        // Create device file info
        let device_info = DeviceFileInfo {
            device_id,
            device_type: DeviceType::Char,
        };
        
        // Create /dev directory and device file
        fs.create_dir("/dev").unwrap();
        fs.create_file("/dev/comprehensive_char", FileType::CharDevice(device_info)).unwrap();
        
        // Test file creation and opening
        let file_handle = fs.open("/dev/comprehensive_char", 0).unwrap();
        
        // Verify file metadata
        let metadata = file_handle.metadata().unwrap();
        assert_eq!(metadata.file_type, FileType::CharDevice(device_info));
        assert_eq!(metadata.size, 0); // Device files should have size 0
        
        // Test reading from device file
        let mut read_buffer = [0u8; 12];
        let bytes_read = file_handle.read(&mut read_buffer).unwrap();
        assert_eq!(bytes_read, 12);
        assert_eq!(&read_buffer[..bytes_read], b"Hello World!");
        
        // Test writing to device file
        let write_data = b"Test Write";
        let bytes_written = file_handle.write(write_data).unwrap();
        assert_eq!(bytes_written, 10);
        
        // Verify the write was successful by checking the device's internal state
        // Note: In a real test, we would need a way to verify the write operation
        // For MockCharDevice, this would require accessing the written data
        
        // Test multiple read/write operations
        let mut small_buffer = [0u8; 5];
        let bytes_read_2 = file_handle.read(&mut small_buffer).unwrap();
        // Should be 0 since we've already read all data
        assert_eq!(bytes_read_2, 0);
        
        // Test another write operation
        let write_data_2 = b"More data";
        let bytes_written_2 = file_handle.write(write_data_2).unwrap();
        assert_eq!(bytes_written_2, 9);
    }
    
    #[test_case]
    fn test_device_file_block_device_operations() {
        // Create a test file system
        let fs_block_device = Box::new(MockBlockDevice::new(8, "fs_block", 512, 100));
        let fs = TestFileSystem::new("testfs", fs_block_device, 512);
        
        // Create test block device for device file
        let test_block_device = Box::new(MockBlockDevice::new(9, "test_block_dev", 512, 100));
        let device_id = DeviceManager::get_mut_manager().register_device(test_block_device as Box<dyn Device>);
        
        // Create device file info
        let device_info = DeviceFileInfo {
            device_id,
            device_type: DeviceType::Block,
        };
        
        // Create /dev directory and block device file
        fs.create_dir("/dev").unwrap();
        fs.create_file("/dev/test_block", FileType::BlockDevice(device_info)).unwrap();
        
        // Test file creation and opening
        let file_handle = fs.open("/dev/test_block", 0).unwrap();
        
        // Verify file metadata
        let metadata = file_handle.metadata().unwrap();
        assert_eq!(metadata.file_type, FileType::BlockDevice(device_info));
        assert_eq!(metadata.size, 0); // Device files should have size 0
        
        // Test writing to block device file
        let write_data = vec![0xAA; 512];
        let bytes_written = file_handle.write(&write_data).unwrap();
        assert_eq!(bytes_written, 512);
        
        // Test reading from block device file
        let mut read_buffer = vec![0u8; 512];
        let bytes_read = file_handle.read(&mut read_buffer).unwrap();
        assert_eq!(bytes_read, 512);
        assert_eq!(read_buffer, write_data);
    }
    
    #[test_case]
    fn test_device_file_error_handling() {
        // Create a test file system
        let block_device = Box::new(MockBlockDevice::new(10, "error_test_block", 512, 100));
        let fs = TestFileSystem::new("testfs", block_device, 512);
        
        // Test opening non-existent device file
        let result = fs.open("/dev/nonexistent", 0);
        assert!(result.is_err());
        if let Err(error) = result {
            assert_eq!(error.kind, FileSystemErrorKind::NotFound);
        }
        
        // Create a device file but don't register the device
        let fake_device_info = DeviceFileInfo {
            device_id: 9999, // Non-existent device ID
            device_type: DeviceType::Char,
        };
        
        fs.create_dir("/dev").unwrap();
        fs.create_file("/dev/fake_device", FileType::CharDevice(fake_device_info)).unwrap();
        
        // Try to open the device file with non-existent device
        let result = fs.open("/dev/fake_device", 0);
        assert!(result.is_err());
        if let Err(error) = result {
            assert_eq!(error.kind, FileSystemErrorKind::PermissionDenied);
        }
    }
    
    #[test_case]
    fn test_mixed_file_types_in_filesystem() {
        // Create a test file system
        let block_device = Box::new(MockBlockDevice::new(11, "mixed_test_block", 512, 100));
        let fs = TestFileSystem::new("testfs", block_device, 512);
        
        // Create test devices
        let mut char_device = Box::new(MockCharDevice::new(12, "mixed_char"));
        char_device.set_read_data(vec![b'M', b'i', b'x', b'e', b'd']);
        let char_device_id = DeviceManager::get_mut_manager().register_device(char_device as Box<dyn Device>);
        
        let block_device_test = Box::new(MockBlockDevice::new(13, "mixed_block", 512, 100));
        let block_device_id = DeviceManager::get_mut_manager().register_device(block_device_test as Box<dyn Device>);
        
        // Create mixed file structure
        fs.create_dir("/dev").unwrap();
        fs.create_dir("/home").unwrap();
        
        // Create regular file
        fs.create_file("/home/regular.txt", FileType::RegularFile).unwrap();
        
        // Create device files
        fs.create_file("/dev/char_device", FileType::CharDevice(DeviceFileInfo {
            device_id: char_device_id,
            device_type: DeviceType::Char,
        })).unwrap();
        
        fs.create_file("/dev/block_device", FileType::BlockDevice(DeviceFileInfo {
            device_id: block_device_id,
            device_type: DeviceType::Block,
        })).unwrap();
        
        // Test opening and using different file types
        
        // Regular file
        let regular_file = fs.open("/home/regular.txt", 0).unwrap();
        let regular_metadata = regular_file.metadata().unwrap();
        assert_eq!(regular_metadata.file_type, FileType::RegularFile);
        
        // Character device file
        let char_file = fs.open("/dev/char_device", 0).unwrap();
        let char_metadata = char_file.metadata().unwrap();
        assert!(matches!(char_metadata.file_type, FileType::CharDevice(_)));
        
        // Test reading from character device
        let mut char_buffer = [0u8; 5];
        let char_bytes_read = char_file.read(&mut char_buffer).unwrap();
        assert_eq!(char_bytes_read, 5);
        assert_eq!(&char_buffer, b"Mixed");
        
        // Block device file
        let block_file = fs.open("/dev/block_device", 0).unwrap();
        let block_metadata = block_file.metadata().unwrap();
        assert!(matches!(block_metadata.file_type, FileType::BlockDevice(_)));
        
        // Test writing to and reading from block device
        let test_data = vec![0x55; 256];
        let block_bytes_written = block_file.write(&test_data).unwrap();
        assert_eq!(block_bytes_written, 256);
        
        let mut block_buffer = vec![0u8; 256];
        let block_bytes_read = block_file.read(&mut block_buffer).unwrap();
        assert_eq!(block_bytes_read, 256);
        assert_eq!(block_buffer, test_data);
    }
}
