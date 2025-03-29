use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use spin::Mutex;

use super::*;
use crate::device::block::request::BlockIOResult;


// Mock block device
struct MockBlockDevice {
    id: usize,
    disk_name: &'static str,
    disk_size: usize,
    data: Mutex<Vec<Vec<u8>>>,  // Changed RefCell to Mutex
    request_queue: Mutex<Vec<Box<BlockIORequest>>>,  // Changed RefCell to Mutex
}

impl MockBlockDevice {
    pub fn new(id: usize, disk_name: &'static str, sector_size: usize, sector_count: usize) -> Self {
        let mut data = Vec::with_capacity(sector_count);
        for _ in 0..sector_count {
            data.push(vec![0; sector_size]);
        }
        
        Self {
            id,
            disk_name,
            disk_size: sector_size * sector_count,
            data: Mutex::new(data),
            request_queue: Mutex::new(Vec::new()),
        }
    }
}


impl BlockDevice for MockBlockDevice {
    fn get_id(&self) -> usize {
        self.id
    }
    
    fn get_disk_name(&self) -> &'static str {
        self.disk_name
    }
    
    fn get_disk_size(&self) -> usize {
        self.disk_size
    }
    
    // Update method implementation
    fn enqueue_request(&mut self, request: Box<BlockIORequest>) {
        self.request_queue.lock().push(request);
    }
    
    fn process_requests(&mut self) -> Vec<BlockIOResult> {
        let mut results = Vec::new();
        let requests = {
            let mut queue = self.request_queue.lock();
            core::mem::replace(&mut *queue, Vec::new())
        };
        
        for mut request in requests {
            let result = match request.request_type {
                BlockIORequestType::Read => {
                    let sector = request.sector;
                    let data = self.data.lock();
                    if sector < data.len() {
                        request.buffer = data[sector].clone();
                        Ok(())
                    } else {
                        Err("Invalid sector")
                    }
                },
                BlockIORequestType::Write => {
                    let sector = request.sector;
                    let mut data = self.data.lock();
                    if sector < data.len() {
                        let buffer_len = request.buffer.len();
                        let sector_len = data[sector].len();
                        let len = buffer_len.min(sector_len);
                        
                        data[sector][..len].copy_from_slice(&request.buffer[..len]);
                        Ok(())
                    } else {
                        Err("Invalid sector")
                    }
                }
            };
            
            results.push(BlockIOResult {
                request,
                result,
            });
        }
        
        results
    }
}

// Simple file system implementation for testing
struct TestFileSystem {
    id: usize,
    name: &'static str,
    block_device: Mutex<Box<dyn BlockDevice>>,
    block_size: usize,
    mounted: bool,
    mount_point: String,
    // Simulate a simple directory structure
    directories: Mutex<Vec<(String, Vec<DirectoryEntry>)>>,
}

impl TestFileSystem {
    pub fn new(id: usize, name: &'static str, block_device: Box<dyn BlockDevice>, block_size: usize) -> Self {
        // Initialize the root directory
        let mut dirs = Vec::new();
        dirs.push((
            "/".to_string(),
            vec![
                DirectoryEntry {
                    name: "test.txt".to_string(),
                    file_type: FileType::RegularFile,
                    size: 10,
                    metadata: None,
                },
                DirectoryEntry {
                    name: "testdir".to_string(),
                    file_type: FileType::Directory,
                    size: 0,
                    metadata: None,
                },
            ],
        ));
        
        Self {
            id,
            name,
            block_device: Mutex::new(block_device),
            block_size,
            mounted: false,
            mount_point: String::new(),
            directories: Mutex::new(dirs),
        }
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
    
    fn get_id(&self) -> usize {
        self.id
    }
    
    fn get_block_size(&self) -> usize {
        self.block_size
    }

    fn read_block(&mut self, block_idx: usize, buffer: &mut [u8]) -> Result<()> {
        let mut device = self.block_device.lock();
        
        let request = Box::new(BlockIORequest {
            request_type: BlockIORequestType::Read,
            sector: block_idx,
            sector_count: 1,
            head: 0,
            cylinder: 0,
            buffer: vec![0; self.block_size],
        });
        
        device.enqueue_request(request);
        let results = device.process_requests();
        
        if results.len() != 1 {
            return Err(FileSystemError {
                kind: FileSystemErrorKind::IoError,
                message: "Failed to process block request",
            });
        }
        
        match &results[0].result {
            Ok(_) => {
                let request_buffer = &results[0].request.buffer;
                let len = buffer.len().min(request_buffer.len());
                buffer[..len].copy_from_slice(&request_buffer[..len]);
                Ok(())
            },
            Err(msg) => Err(FileSystemError {
                kind: FileSystemErrorKind::IoError,
                message: msg,
            }),
        }
    }

    fn write_block(&mut self, block_idx: usize, buffer: &[u8]) -> Result<()> {
        let mut device = self.block_device.lock();
        
        let request = Box::new(BlockIORequest {
            request_type: BlockIORequestType::Write,
            sector: block_idx,
            sector_count: 1,
            head: 0,
            cylinder: 0,
            buffer: buffer.to_vec(),
        });
        
        device.enqueue_request(request);
        let results = device.process_requests();
        
        if results.len() != 1 {
            return Err(FileSystemError {
                kind: FileSystemErrorKind::IoError,
                message: "Failed to process block request",
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

// File handle for testing
struct TestFileHandle {
    path: String,
    position: u64,
    content: Vec<u8>,
}

impl FileHandle for TestFileHandle {
    fn read(&mut self, buffer: &mut [u8]) -> Result<usize> {
        if self.position as usize >= self.content.len() {
            return Ok(0); // EOF
        }
        
        let available = self.content.len() - self.position as usize;
        let to_read = buffer.len().min(available);
        
        buffer[..to_read].copy_from_slice(&self.content[self.position as usize..self.position as usize + to_read]);
        self.position += to_read as u64;
        
        Ok(to_read)
    }
    
    fn write(&mut self, buffer: &[u8]) -> Result<usize> {
        let pos = self.position as usize;
        
        // Expand file size if necessary
        if pos + buffer.len() > self.content.len() {
            self.content.resize(pos + buffer.len(), 0);
        }
        
        // Write data
        self.content[pos..pos + buffer.len()].copy_from_slice(buffer);
        self.position += buffer.len() as u64;
        
        Ok(buffer.len())
    }
    
    fn seek(&mut self, whence: SeekFrom) -> Result<u64> {
        match whence {
            SeekFrom::Start(offset) => {
                self.position = offset;
            },
            SeekFrom::Current(offset) => {
                if offset >= 0 {
                    self.position = self.position.saturating_add(offset as u64);
                } else {
                    self.position = self.position.saturating_sub((-offset) as u64);
                }
            },
            SeekFrom::End(offset) => {
                let end = self.content.len() as u64;
                if offset >= 0 {
                    self.position = end.saturating_add(offset as u64);
                } else {
                    self.position = end.saturating_sub((-offset) as u64);
                }
            },
        }
        
        Ok(self.position)
    }
    
    fn close(&mut self) -> Result<()> {
        Ok(())
    }
    
    fn metadata(&self) -> Result<FileMetadata> {
        Ok(FileMetadata {
            file_type: FileType::RegularFile,
            size: self.content.len(),
            permissions: FilePermission {
                read: true,
                write: true,
                execute: false,
            },
            created_time: 0,
            modified_time: 0,
            accessed_time: 0,
        })
    }
}

impl FileOperations for TestFileSystem {
    fn open(&self, path: &str, _flags: u32) -> Result<Box<dyn FileHandle>> {
        let normalized = self.normalize_path(path);
        
        // Simple implementation for testing (only check the beginning of the path)
        if (normalized == "/test.txt") {
            return Ok(Box::new(TestFileHandle {
                path: normalized,
                position: 0,
                content: b"Hello, world!".to_vec(),
            }));
        }

        // Search for dynamically created files
        let parent_path = normalized.rfind('/').map_or("/", |idx| &normalized[..idx]);
        let parent_path = if parent_path.is_empty() { "/" } else { parent_path };
        let name = normalized.rfind('/').map_or(normalized.as_str(), |idx| &normalized[idx + 1..]);
        
        for (dir_path, entries) in self.directories.lock().iter() {
            if dir_path == parent_path {
                if let Some(entry) = entries.iter().find(|e| e.name == name && e.file_type == FileType::RegularFile) {
                    return Ok(Box::new(TestFileHandle {
                        path: normalized,
                        position: 0,
                        content: Vec::new(), // Newly created files are empty
                    }));
                }
            }
        }
        
        Err(FileSystemError {
            kind: FileSystemErrorKind::NotFound,
            message: "File not found",
        })
    }
    
    fn read_dir(&self, path: &str) -> Result<Vec<DirectoryEntry>> {
        let normalized = self.normalize_path(path);
    
        // First check if the path is a file
        let parent_path = normalized.rfind('/').map_or("/", |idx| &normalized[..idx]);
        let parent_path = if parent_path.is_empty() { "/" } else { parent_path };
        let name = normalized.rfind('/').map_or(normalized.as_str(), |idx| &normalized[idx + 1..]);
        
        // Check if there is a file with the same name in the parent directory
        for (dir_path, entries) in self.directories.lock().iter() {
            if dir_path == parent_path {
                if let Some(entry) = entries.iter().find(|e| e.name == name && e.file_type != FileType::Directory) {
                    return Err(FileSystemError {
                        kind: FileSystemErrorKind::NotADirectory,
                        message: "Not a directory",
                    });
                }
            }
        }

        if let Some(entries) = self.find_directory(path) {
            Ok(entries)
        } else {
            Err(FileSystemError {
                kind: FileSystemErrorKind::NotFound,
                message: "Directory not found",
            })
        }
    }
    
    fn create_file(&self, path: &str) -> Result<()> {
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
                        message: "File already exists",
                    });
                }
                
                // Add the new file to the entries
                entries.push(DirectoryEntry {
                    name: file_name.to_string(),
                    file_type: FileType::RegularFile,
                    size: 0,
                    metadata: None,
                });
                
                return Ok(());
            }
        }
        
        Err(FileSystemError {
            kind: FileSystemErrorKind::NotFound,
            message: "Parent directory not found",
        })
    }
    
    fn create_dir(&self, path: &str) -> Result<()> {
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
                        message: "Directory already exists",
                    });
                }
                
                // Add the new directory to the entries
                entries.push(DirectoryEntry {
                    name: dir_name.to_string(),
                    file_type: FileType::Directory,
                    size: 0,
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
            message: "Parent directory not found",
        })
    }
    
    fn remove(&self, path: &str) -> Result<()> {
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
                        message: "Cannot remove non-empty directory",
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
            message: "File or directory not found",
        })
    }
    
    fn metadata(&self, path: &str) -> Result<FileMetadata> {
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
            });
        }
        
        let parent_path = normalized.rfind('/').map_or("/", |idx| &normalized[..idx]);
        let parent_path = if parent_path.is_empty() { "/" } else { parent_path };
        
        let name = normalized.rfind('/').map_or(normalized.as_str(), |idx| &normalized[idx + 1..]);
        
        for (dir_path, entries) in self.directories.lock().iter() {
            if dir_path == parent_path {
                if let Some(entry) = entries.iter().find(|e| e.name == name) {
                    return Ok(FileMetadata {
                        file_type: entry.file_type,
                        size: entry.size,
                        permissions: FilePermission {
                            read: true,
                            write: true,
                            execute: false,
                        },
                        created_time: 0,
                        modified_time: 0,
                        accessed_time: 0,
                    });
                }
            }
        }
        
        Err(FileSystemError {
            kind: FileSystemErrorKind::NotFound,
            message: "File or directory not found",
        })
    }
    
    fn root_dir(&self) -> Result<Directory> {
        Ok(Directory::new("/".to_string(), self.id))
    }
}

// Test cases
#[test_case]
fn test_vfs_manager_creation() {
    let manager = VfsManager::new();
    assert_eq!(manager.filesystems.len(), 0);
    assert_eq!(manager.mount_points.len(), 0);
}

#[test_case]
fn test_fs_registration_and_mount() {
    let mut manager = VfsManager::new();
    let device = Box::new(MockBlockDevice::new(1, "test_disk", 512, 100));
    let fs = Box::new(TestFileSystem::new(0, "testfs", device, 512));
    
    manager.register_fs(fs);
    assert_eq!(manager.filesystems.len(), 1);
    
    let result = manager.mount("testfs", "/mnt");
    assert!(result.is_ok());
    assert_eq!(manager.filesystems.len(), 0);
    assert_eq!(manager.mount_points.len(), 1);
}

#[test_case]
fn test_path_resolution() {
    let mut manager = VfsManager::new();
    let device = Box::new(MockBlockDevice::new(1, "test_disk", 512, 100));
    let fs = Box::new(TestFileSystem::new(0, "testfs", device, 512));
    
    manager.register_fs(fs);
    let _ = manager.mount("testfs", "/mnt");
    
    // Resolve valid path
    let result = manager.resolve_path("/mnt/test.txt");
    assert!(result.is_ok());
    
    let (_, relative_path) = result.unwrap();
    assert_eq!(relative_path, "/test.txt");
    
    // Another valid path
    let result = manager.resolve_path("/mnt");
    assert!(result.is_ok());
    
    let (_, relative_path) = result.unwrap();
    assert_eq!(relative_path, "/");
    
    // Resolve invalid path
    let result = manager.resolve_path("/invalid/path");
    assert!(result.is_err());
}

#[test_case]
fn test_file_operations() {
    let mut manager = VfsManager::new();
    let device = Box::new(MockBlockDevice::new(1, "test_disk", 512, 100));
    let fs = Box::new(TestFileSystem::new(0, "testfs", device, 512));
    
    manager.register_fs(fs);
    let _ = manager.mount("testfs", "/mnt");
    
    // Open file
    let mut file = manager.open("/mnt/test.txt", 0).unwrap();
    
    // Read test
    let mut buffer = [0u8; 20];
    let bytes_read = file.read(&mut buffer).unwrap();
    assert_eq!(bytes_read, 13); // Length of "Hello, world!"
    assert_eq!(&buffer[..13], b"Hello, world!");
    
    // Write test
    file.seek(SeekFrom::Start(0)).unwrap();
    let bytes_written = file.write(b"Test data").unwrap();
    assert_eq!(bytes_written, 9);
    
    // Re-read test
    file.seek(SeekFrom::Start(0)).unwrap();
    let mut buffer2 = [0u8; 20];
    let bytes_read2 = file.read(&mut buffer2).unwrap();
    assert_eq!(bytes_read2, 13); // File length is still 13 (Hello, world!)
    assert_eq!(&buffer2[..9], b"Test data"); // The beginning part has been replaced
}

#[test_case]
fn test_directory_operations() {
    let mut manager = VfsManager::new();
    let device = Box::new(MockBlockDevice::new(1, "test_disk", 512, 100));
    let fs = Box::new(TestFileSystem::new(0, "testfs", device, 512));
    
    manager.register_fs(fs);
    let _ = manager.mount("testfs", "/mnt");
    
    // Get directory entries
    let entries = manager.read_dir("/mnt").unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].name, "test.txt");
    assert_eq!(entries[1].name, "testdir");
    assert_eq!(entries[0].file_type, FileType::RegularFile);
    assert_eq!(entries[1].file_type, FileType::Directory);
    
    // Create directory
    let result = manager.create_dir("/mnt/newdir");
    assert!(result.is_ok());
    
    // Verify
    let entries_after = manager.read_dir("/mnt").unwrap();
    assert_eq!(entries_after.len(), 3);
    assert!(entries_after.iter().any(|e| e.name == "newdir" && e.file_type == FileType::Directory));
    
    // Create file
    let result = manager.create_file("/mnt/newdir/newfile.txt");
    assert!(result.is_ok());
    
    // Verify
    let dir_entries = manager.read_dir("/mnt/newdir").unwrap();
    assert_eq!(dir_entries.len(), 1);
    assert_eq!(dir_entries[0].name, "newfile.txt");
    
    // Delete test
    let result = manager.remove("/mnt/newdir/newfile.txt");
    assert!(result.is_ok());
    
    // Delete empty directory
    let result = manager.remove("/mnt/newdir");
    assert!(result.is_ok());
}

#[test_case]
fn test_block_device_operations() {
    let device = MockBlockDevice::new(1, "test_disk", 512, 100);
    let fs = GenericFileSystem::new(0, "generic", Box::new(device), 512);
    
    // Prepare test data
    let test_data = [0xAA; 512];
    let mut read_buffer = [0; 512];
    
    // Write test
    let write_result = fs.write_block_internal(0, &test_data);
    assert!(write_result.is_ok());
    
    // Read test
    let read_result = fs.read_block_internal(0, &mut read_buffer);
    assert!(read_result.is_ok());
    
    // Verify data match
    assert_eq!(test_data, read_buffer);
}

#[test_case]
fn test_unmount() {
    let mut manager = VfsManager::new();
    let device = Box::new(MockBlockDevice::new(1, "test_disk", 512, 100));
    let fs = Box::new(TestFileSystem::new(0, "testfs", device, 512));
    
    manager.register_fs(fs);
    let _ = manager.mount("testfs", "/mnt");
    assert_eq!(manager.mount_points.len(), 1);
    
    // Unmount
    let result = manager.unmount("/mnt");
    assert!(result.is_ok());
    assert_eq!(manager.mount_points.len(), 0);
    assert_eq!(manager.filesystems.len(), 1); // The file system should be returned
    
    // Attempt to unmount an invalid mount point
    let result = manager.unmount("/invalid");
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().kind, FileSystemErrorKind::NotFound);
}

// Test file structure

#[test_case]
fn test_file_creation() {
    // Setup
    let mut manager = VfsManager::new();
    let device = Box::new(MockBlockDevice::new(1, "test_disk", 512, 100));
    let fs = Box::new(TestFileSystem::new(0, "testfs", device, 512));
    
    manager.register_fs(fs);
    let _ = manager.mount("testfs", "/mnt");


    // Create an instance of the file structure
    let mut file = File::with_manager("/mnt/test.txt".to_string(), 0, &mut manager);
    assert_eq!(file.path, "/mnt/test.txt");
    assert_eq!(file.fs_id, 0);
    assert!(!file.is_open());
}

#[test_case]
fn test_file_open_close() {
    // Setup
    let mut manager = VfsManager::new();
    let device = Box::new(MockBlockDevice::new(1, "test_disk", 512, 100));
    let fs = Box::new(TestFileSystem::new(0, "testfs", device, 512));
    
    manager.register_fs(fs);
    let _ = manager.mount("testfs", "/mnt");
    
    // Create and open a file object
    let mut file = File::with_manager("/mnt/test.txt".to_string(), 0, &mut manager);

    // Initially closed
    assert!(!file.is_open());
    
    // Open the file
    let result = file.open(0);
    assert!(result.is_ok());
    assert!(file.is_open());
    
    // Opening an already open file is fine
    let result = file.open(0);
    assert!(result.is_ok());
    
    // Close the file
    let result = file.close();
    assert!(result.is_ok());
    assert!(!file.is_open());
    
    // Closing an already closed file is fine
    let result = file.close();
    assert!(result.is_ok());
}

#[test_case]
fn test_file_read_write() {
    // Setup
    let mut manager = VfsManager::new();
    let device = Box::new(MockBlockDevice::new(1, "test_disk", 512, 100));
    let fs = Box::new(TestFileSystem::new(0, "testfs", device, 512));
    
    manager.register_fs(fs);
    let _ = manager.mount("testfs", "/mnt");
    
    let mut file = File::with_manager("/mnt/test.txt".to_string(), 0, &mut manager);
    
    // Reading and writing while closed results in an error
    let mut buffer = [0u8; 10];
    let read_result = file.read(&mut buffer);
    assert!(read_result.is_err());
    assert_eq!(read_result.unwrap_err().kind, FileSystemErrorKind::IoError);
    
    let write_result = file.write(b"test");
    assert!(write_result.is_err());
    assert_eq!(write_result.unwrap_err().kind, FileSystemErrorKind::IoError);
    
    // Open the file
    file.open(0).unwrap();
    
    // Read test
    let mut buffer = [0u8; 20];
    let bytes_read = file.read(&mut buffer).unwrap();
    assert_eq!(bytes_read, 13); // Length of "Hello, world!"
    assert_eq!(&buffer[..13], b"Hello, world!");
    
    // Write test
    file.seek(SeekFrom::Start(0)).unwrap();
    let bytes_written = file.write(b"Test data").unwrap();
    assert_eq!(bytes_written, 9);
    
    // Re-read test
    file.seek(SeekFrom::Start(0)).unwrap();
    let mut buffer2 = [0u8; 20];
    let bytes_read2 = file.read(&mut buffer2).unwrap();
    assert_eq!(bytes_read2, 13);
    assert_eq!(&buffer2[..9], b"Test data");
}

#[test_case]
fn test_file_seek() {
    // Setup
    let mut manager = VfsManager::new();
    let device = Box::new(MockBlockDevice::new(1, "test_disk", 512, 100));
    let fs = Box::new(TestFileSystem::new(0, "testfs", device, 512));
    
    manager.register_fs(fs);
    let _ = manager.mount("testfs", "/mnt");
    
    let mut file = File::with_manager("/mnt/test.txt".to_string(), 0, &mut manager);
    file.open(0).unwrap();
    
    // Seek from the start
    let pos = file.seek(SeekFrom::Start(5)).unwrap();
    assert_eq!(pos, 5);
    
    // Seek from the current position (forward)
    let pos = file.seek(SeekFrom::Current(3)).unwrap();
    assert_eq!(pos, 8);
    
    // Seek from the current position (backward)
    let pos = file.seek(SeekFrom::Current(-4)).unwrap();
    assert_eq!(pos, 4);
    
    // Seek from the end
    let pos = file.seek(SeekFrom::End(-5)).unwrap();
    assert_eq!(pos, 8); // 13 - 5 = 8
}

#[test_case]
fn test_file_metadata_and_size() {
    // Setup
    let mut manager = VfsManager::new();
    let device = Box::new(MockBlockDevice::new(1, "test_disk", 512, 100));
    let fs = Box::new(TestFileSystem::new(0, "testfs", device, 512));
    
    manager.register_fs(fs);
    let _ = manager.mount("testfs", "/mnt");
    
    let mut file = File::with_manager("/mnt/test.txt".to_string(), 0, &mut manager);
    
    // Get metadata (possible even when not open)
    let metadata = file.metadata().unwrap();
    assert_eq!(metadata.file_type, FileType::RegularFile);

    // Write
    file.open(0).unwrap();
    file.write(b"Hello, world!").unwrap();
    
    // Get size
    let size = file.size().unwrap();
    assert_eq!(size, 13); // Length of "Hello, world!"
}

#[test_case]
fn test_file_read_all() {
    // Setup
    let mut manager = VfsManager::new();
    let device = Box::new(MockBlockDevice::new(1, "test_disk", 512, 100));
    let fs = Box::new(TestFileSystem::new(0, "testfs", device, 512));
    
    manager.register_fs(fs);
    let _ = manager.mount("testfs", "/mnt");
    
    // let mut file = File::new("/mnt/test.txt".to_string(), 0);
    let mut file = File::with_manager("/mnt/test.txt".to_string(), 0, &mut manager);

    file.open(0).unwrap();

    // Write
    file.write(b"Hello, world!").unwrap();
    
    // Read the entire file
    let content = file.read_all().unwrap();
    assert_eq!(content, b"Hello, world!");
    
    // Modify part of the file and read again
    file.seek(SeekFrom::Start(0)).unwrap();
    file.write(b"Modified, ").unwrap();
    file.write(b"world!").unwrap();
    
    file.seek(SeekFrom::Start(0)).unwrap();
    let modified_content = file.read_all().unwrap();
    assert_eq!(modified_content, b"Modified, world!");
}

#[test_case]
fn test_file_auto_close() {
    // Setup
    let mut manager = VfsManager::new();
    let device = Box::new(MockBlockDevice::new(1, "test_disk", 512, 100));
    let fs = Box::new(TestFileSystem::new(0, "testfs", device, 512));
    
    manager.register_fs(fs);
    let _ = manager.mount("testfs", "/mnt");
    
    // Open a file within a scope
    {
        // let mut file = File::new("/mnt/test.txt".to_string(), 0);
        let mut file = File::with_manager("/mnt/test.txt".to_string(), 0, &mut manager);
        file.open(0).unwrap();
        assert!(file.is_open());
        
        // Exiting the scope will automatically close the file due to the Drop trait
    }
    
    // Verify that a new file object can be created and opened
    let mut file2 = File::with_manager("/mnt/test.txt".to_string(), 0, &mut manager);
    let result = file2.open(0);
    assert!(result.is_ok());
}

#[test_case]
fn test_directory_creation() {
    // Setup
    let mut manager = VfsManager::new();
    let device = Box::new(MockBlockDevice::new(1, "test_disk", 512, 100));
    let fs = Box::new(TestFileSystem::new(0, "testfs", device, 512));
    
    manager.register_fs(fs);
    let _ = manager.mount("testfs", "/mnt");
    
    // Create an instance of the directory structure
    let dir = Directory::with_manager("/mnt".to_string(), 0, &mut manager);
    assert_eq!(dir.path, "/mnt");
    assert_eq!(dir.fs_id, 0);
}

#[test_case]
fn test_directory_read_entries() {
    // Setup
    let mut manager = VfsManager::new();
    let device = Box::new(MockBlockDevice::new(1, "test_disk", 512, 100));
    let fs = Box::new(TestFileSystem::new(0, "testfs", device, 512));
    
    manager.register_fs(fs);
    let _ = manager.mount("testfs", "/mnt");
    
    // Read directory entries
    let dir = Directory::with_manager("/mnt".to_string(), 0, &mut manager);
    let entries = dir.read_entries().unwrap();
    
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].name, "test.txt");
    assert_eq!(entries[1].name, "testdir");
    assert_eq!(entries[0].file_type, FileType::RegularFile);
    assert_eq!(entries[1].file_type, FileType::Directory);
}

#[test_case]
fn test_directory_create_file() {
    // Setup
    let mut manager = VfsManager::new();
    let device = Box::new(MockBlockDevice::new(1, "test_disk", 512, 100));
    let fs = Box::new(TestFileSystem::new(0, "testfs", device, 512));
    
    manager.register_fs(fs);
    let _ = manager.mount("testfs", "/mnt");
    
    // Create a file in the directory
    let dir = Directory::with_manager("/mnt".to_string(), 0, &mut manager);
    let result = dir.create_file("newfile.txt");
    assert!(result.is_ok());
    
    // Verify the created file
    let entries = dir.read_entries().unwrap();
    assert!(entries.iter().any(|e| e.name == "newfile.txt" && e.file_type == FileType::RegularFile));
    
    // Try opening the file
    let mut file = File::with_manager("/mnt/newfile.txt".to_string(), 0, &mut manager);
    let file_result = file.open(0);
    assert!(file_result.is_ok());
}

#[test_case]
fn test_directory_create_subdirectory() {
    // Setup
    let mut manager = VfsManager::new();
    let device = Box::new(MockBlockDevice::new(1, "test_disk", 512, 100));
    let fs = Box::new(TestFileSystem::new(0, "testfs", device, 512));
    
    manager.register_fs(fs);
    let _ = manager.mount("testfs", "/mnt");
    
    // Create a subdirectory
    let dir = Directory::with_manager("/mnt".to_string(), 0, &mut manager);
    let result = dir.create_dir("subdir");
    assert!(result.is_ok());
    
    // Verify the created directory
    let entries = dir.read_entries().unwrap();
    assert!(entries.iter().any(|e| e.name == "subdir" && e.file_type == FileType::Directory));
    
    // Operate on the subdirectory
    let subdir = Directory::with_manager("/mnt/subdir".to_string(), 0, &mut manager);
    let entries = subdir.read_entries().unwrap();
    assert!(entries.is_empty()); // Newly created directory is empty
}

#[test_case]
fn test_directory_nested_operations() {
    // Setup
    let mut manager = VfsManager::new();
    let device = Box::new(MockBlockDevice::new(1, "test_disk", 512, 100));
    let fs = Box::new(TestFileSystem::new(0, "testfs", device, 512));
    
    manager.register_fs(fs);
    let _ = manager.mount("testfs", "/mnt");
    
    // Nested operations
    let root_dir = Directory::with_manager("/mnt".to_string(), 0, &mut manager);
    
    // Create a subdirectory
    root_dir.create_dir("level1").unwrap();
    
    // Operate on the subdirectory
    let level1_dir = Directory::with_manager("/mnt/level1".to_string(), 0, &mut manager);
    level1_dir.create_dir("level2").unwrap();
    level1_dir.create_file("file_in_level1.txt").unwrap();
    
    // Operate on a deeper level
    let level2_dir = Directory::with_manager("/mnt/level1/level2".to_string(), 0, &mut manager);
    level2_dir.create_file("deep_file.txt").unwrap();
    
    // Verify
    let entries = level2_dir.read_entries().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "deep_file.txt");
}

#[test_case]
fn test_directory_error_handling() {
    // Setup
    let mut manager = VfsManager::new();
    let device = Box::new(MockBlockDevice::new(1, "test_disk", 512, 100));
    let fs = Box::new(TestFileSystem::new(0, "testfs", device, 512));
    
    manager.register_fs(fs);
    let _ = manager.mount("testfs", "/mnt");
    
    // Non-existent directory
    let nonexistent_dir = Directory::with_manager("/mnt/nonexistent".to_string(), 0, &mut manager);
    let result = nonexistent_dir.read_entries();
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().kind, FileSystemErrorKind::NotFound);
    
    // Create a file in a non-existent directory
    let result = nonexistent_dir.create_file("test.txt");
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().kind, FileSystemErrorKind::NotFound);
    
    // Treat a file as a directory
    let file_as_dir = Directory::with_manager("/mnt/test.txt".to_string(), 0, &mut manager);
    let result = file_as_dir.read_entries();
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().kind, FileSystemErrorKind::NotADirectory);
}

#[test_case]
fn test_directory_with_global_manager() {
    // Setup the global VFS manager
    let device = Box::new(MockBlockDevice::new(1, "test_disk", 512, 100));
    let fs = Box::new(TestFileSystem::new(0, "testfs", device, 512));
    
    let global_manager = get_vfs_manager();
    global_manager.register_fs(fs);
    let _ = global_manager.mount("testfs", "/mnt");
    
    // Directory operations using the global manager
    let dir = Directory::new("/mnt".to_string(), 0);
    let entries = dir.read_entries().unwrap();
    assert_eq!(entries.len(), 2);
    
    // Cleanup
    let _ = global_manager.unmount("/mnt");
}