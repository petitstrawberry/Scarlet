use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use spin::rwlock::RwLock;
use spin::Mutex;

use super::*;

// Simple file system implementation for testing
pub struct TestFileSystem {
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

// File handle for testing
struct TestFileHandle {
    path: String,
    position: RwLock<u64>,
    content: RwLock<Vec<u8>>,
}

impl FileHandle for TestFileHandle {
    fn read(&self, buffer: &mut [u8]) -> Result<usize> {
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
    
    fn write(&self, buffer: &[u8]) -> Result<usize> {
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
    
    fn seek(&self, whence: SeekFrom) -> Result<u64> {
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
    
    fn release(&self) -> Result<()> {
        Ok(())
    }
    
    fn metadata(&self) -> Result<FileMetadata> {
        Ok(FileMetadata {
            file_type: FileType::RegularFile,
            size: self.content.read().len(),
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
    fn open(&self, path: &str, _flags: u32) -> Result<Arc<dyn FileHandle>> {
        let normalized = self.normalize_path(path);
        
        // Simple implementation for testing (only check the beginning of the path)
        if normalized == "/test.txt" {
            return Ok(Arc::new(TestFileHandle {
                path: normalized,
                position: RwLock::new(0),
                content: RwLock::new(b"Hello, world!".to_vec()),
            }));
        }

        // Search for dynamically created files
        let parent_path = normalized.rfind('/').map_or("/", |idx| &normalized[..idx]);
        let parent_path = if parent_path.is_empty() { "/" } else { parent_path };
        let name = normalized.rfind('/').map_or(normalized.as_str(), |idx| &normalized[idx + 1..]);
        
        for (dir_path, entries) in self.directories.lock().iter() {
            if dir_path == parent_path {
                if let Some(_) = entries.iter().find(|e| e.name == name && e.file_type == FileType::RegularFile) {
                    return Ok(Arc::new(TestFileHandle {
                        path: normalized,
                        position: RwLock::new(0),
                        content: RwLock::new(Vec::new()), // Newly created files are empty
                    }));
                }
            }
        }
        
        Err(FileSystemError {
            kind: FileSystemErrorKind::NotFound,
            message: "File not found".to_string(),
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
                        message: "File already exists".to_string(),
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
            message: "Parent directory not found".to_string(),
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
                        message: "Directory already exists".to_string(),
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
            message: "Parent directory not found".to_string(),
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
            message: "File or directory not found".to_string(),
        })
    }
    
    fn root_dir(&self) -> Result<Directory> {
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
        FileSystemType::Block  // このドライバーはブロックデバイスベースのファイルシステム
    }
    
    fn create_from_block(&self, block_device: Box<dyn BlockDevice>, block_size: usize) -> Result<Box<dyn VirtualFileSystem>> {
        Ok(Box::new(TestFileSystem::new(0, "testfs", block_device, block_size)))
    }
}
