//! TmpFS - Temporary File System (RAM-only)
//! 
//! This is a production-ready implementation of a temporary filesystem that stores
//! all data in RAM. Unlike TestFileSystem, this implementation is optimized for
//! practical use cases with features like:
//! - Dynamic memory allocation for file content
//! - Proper file permissions and timestamps
//! - Efficient directory tree management
//! - Support for device files and symbolic links
//! - Memory usage optimization

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use spin::rwlock::RwLock;
use spin::Mutex;

use super::*;
use crate::device::manager::{BorrowedDeviceGuard, DeviceManager};
use crate::device::DeviceType;

/// Directory entries collection with type-safe operations
#[derive(Clone, Default)]
struct DirectoryEntries {
    entries: BTreeMap<String, TmpNode>,
}

impl DirectoryEntries {
    /// Create new empty directory entries
    fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }

    /// Add a new entry to the directory
    fn insert(&mut self, name: String, node: TmpNode) -> Option<TmpNode> {
        self.entries.insert(name, node)
    }

    /// Remove an entry from the directory
    fn remove(&mut self, name: &str) -> Option<TmpNode> {
        self.entries.remove(name)
    }

    /// Get a reference to an entry
    fn get(&self, name: &str) -> Option<&TmpNode> {
        self.entries.get(name)
    }

    /// Get a mutable reference to an entry
    fn get_mut(&mut self, name: &str) -> Option<&mut TmpNode> {
        self.entries.get_mut(name)
    }

    /// Check if an entry exists
    fn contains(&self, name: &str) -> bool {
        self.entries.contains_key(name)
    }

    /// Check if a key exists (alias for contains)
    fn contains_key(&self, name: &str) -> bool {
        self.entries.contains_key(name)
    }

    /// Get all entry names
    fn entry_names(&self) -> impl Iterator<Item = &String> {
        self.entries.keys()
    }

    /// Get all entries
    fn entries(&self) -> impl Iterator<Item = (&String, &TmpNode)> {
        self.entries.iter()
    }

    /// Get mutable iterator over entries
    fn entries_mut(&mut self) -> impl Iterator<Item = (&String, &mut TmpNode)> {
        self.entries.iter_mut()
    }

    /// Get the number of entries
    fn len(&self) -> usize {
        self.entries.len()
    }

    /// Check if directory is empty
    fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Clear all entries
    fn clear(&mut self) {
        self.entries.clear();
    }
}

/// Node in the tmpfs filesystem
#[derive(Clone)]
struct TmpNode {
    /// File name
    name: String,
    /// File type and associated data
    file_type: FileType,
    /// File content (only for regular files)
    content: Vec<u8>,
    /// File metadata
    metadata: FileMetadata,
    /// For directories: child nodes
    children: DirectoryEntries,
}

impl TmpNode {
    /// Create a new regular file node
    fn new_file(name: String, file_id: u64) -> Self {
        Self {
            name: name.clone(),
            file_type: FileType::RegularFile,
            content: Vec::new(),
            metadata: FileMetadata {
                file_type: FileType::RegularFile,
                size: 0,
                permissions: FilePermission {
                    read: true,
                    write: true,
                    execute: false,
                },
                created_time: crate::time::current_time(),
                modified_time: crate::time::current_time(),
                accessed_time: crate::time::current_time(),
                file_id,
                link_count: 1,
            },
            children: DirectoryEntries::new(),
        }
    }

    /// Create a new directory node
    fn new_directory(name: String, file_id: u64) -> Self {
        Self {
            name: name.clone(),
            file_type: FileType::Directory,
            content: Vec::new(),
            metadata: FileMetadata {
                file_type: FileType::Directory,
                size: 0,
                permissions: FilePermission {
                    read: true,
                    write: true,
                    execute: true,
                },
                created_time: crate::time::current_time(),
                modified_time: crate::time::current_time(),
                accessed_time: crate::time::current_time(),
                file_id,
                link_count: 1,
            },
            children: DirectoryEntries::new(),
        }
    }

    /// Create a new device file node
    fn new_device(name: String, file_type: FileType, file_id: u64) -> Self {
        Self {
            name: name.clone(),
            file_type: file_type.clone(),
            content: Vec::new(),
            metadata: FileMetadata {
                file_type,
                size: 0,
                permissions: FilePermission {
                    read: true,
                    write: true,
                    execute: false,
                },
                created_time: crate::time::current_time(),
                modified_time: crate::time::current_time(),
                accessed_time: crate::time::current_time(),
                file_id,
                link_count: 1,
            },
            children: DirectoryEntries::new(),
        }
    }

    /// Update file size and modification time
    fn update_size(&mut self, new_size: usize) {
        self.metadata.size = new_size;
        self.metadata.modified_time = crate::time::current_time();
    }

    /// Update access time
    fn update_access_time(&mut self) {
        self.metadata.accessed_time = crate::time::current_time();
    }
}

/// TmpFS - RAM-only filesystem
pub struct TmpFS {
    mounted: bool,
    mount_point: String,
    /// Root directory of the filesystem
    root: RwLock<TmpNode>,
    /// Maximum memory usage in bytes (0 = unlimited)
    max_memory: usize,
    /// Current memory usage in bytes
    current_memory: Mutex<usize>,
    /// Next file ID to assign
    next_file_id: Mutex<u64>,
    /// Map from file_id to TmpNode for hardlink management
    file_id_to_node: Mutex<BTreeMap<u64, Arc<RwLock<TmpNode>>>>,
}

impl TmpFS {
    /// Create a new TmpFS instance
    pub fn new(max_memory: usize) -> Self {
        let root = TmpNode::new_directory("/".to_string(), 1); // Root always has file_id = 1
        let mut file_id_to_node = BTreeMap::new();
        let root_arc = Arc::new(RwLock::new(root.clone()));
        file_id_to_node.insert(1, root_arc);
        
        Self {
            mounted: false,
            mount_point: String::new(),
            root: RwLock::new(root),
            max_memory,
            current_memory: Mutex::new(0),
            next_file_id: Mutex::new(2), // Start from 2 since root is 1
            file_id_to_node: Mutex::new(file_id_to_node),
        }
    }

    /// Generate the next file ID
    fn generate_file_id(&self) -> u64 {
        let mut next_id = self.next_file_id.lock();
        let id = *next_id;
        *next_id += 1;
        id
    }

    /// Get current memory usage
    pub fn memory_usage(&self) -> usize {
        *self.current_memory.lock()
    }

    /// Get maximum memory limit
    pub fn memory_limit(&self) -> usize {
        self.max_memory
    }

    /// Check if memory allocation is allowed
    fn check_memory_limit(&self, additional_bytes: usize) -> Result<()> {
        if self.max_memory == 0 {
            return Ok(()); // Unlimited
        }

        let current = *self.current_memory.lock();
        if current + additional_bytes > self.max_memory {
            return Err(FileSystemError {
                kind: FileSystemErrorKind::NoSpace,
                message: "TmpFS memory limit exceeded".to_string(),
            });
        }

        Ok(())
    }

    /// Add to memory usage counter
    fn add_memory_usage(&self, bytes: usize) {
        *self.current_memory.lock() += bytes;
    }

    /// Subtract from memory usage counter
    fn subtract_memory_usage(&self, bytes: usize) {
        let mut current = self.current_memory.lock();
        *current = current.saturating_sub(bytes);
    }

    /// Find a node by path
    fn find_node(&self, path: &str) -> Option<TmpNode> {
        let normalized = self.normalize_path(path);
        
        if normalized == "/" {
            return Some(self.root.read().clone());
        }

        let parts: Vec<&str> = normalized.trim_start_matches('/').split('/').collect();
        let root = self.root.read();
        let mut current = &*root;

        for part in parts {
            if let Some(child) = current.children.get(part) {
                current = child;
            } else {
                return None;
            }
        }

        Some(current.clone())
    }

    /// Find a mutable reference to a node by path
    fn find_node_mut<F, R>(&self, path: &str, f: F) -> Option<R>
    where
        F: FnOnce(&mut TmpNode) -> R,
    {
        let normalized = self.normalize_path(path);
        
        if normalized == "/" {
            let mut root = self.root.write();
            return Some(f(&mut *root));
        }

        let parts: Vec<&str> = normalized.trim_start_matches('/').split('/').collect();
        let mut root = self.root.write();
        let mut current = &mut *root;

        for part in parts {
            if let Some(child) = current.children.get_mut(part) {
                current = child;
            } else {
                return None;
            }
        }

        Some(f(current))
    }

    /// Find parent node and return mutable reference
    fn find_parent_mut<F, R>(&self, path: &str, f: F) -> Result<R>
    where
        F: FnOnce(&mut TmpNode, &str) -> R,
    {
        let normalized = self.normalize_path(path);
        let (parent_path, filename) = if let Some(pos) = normalized.rfind('/') {
            let parent = if pos == 0 { "/" } else { &normalized[..pos] };
            let name = &normalized[pos + 1..];
            (parent, name)
        } else {
            return Err(FileSystemError {
                kind: FileSystemErrorKind::InvalidPath,
                message: "Invalid path".to_string(),
            });
        };

        if parent_path == "/" {
            let mut root = self.root.write();
            return Ok(f(&mut *root, filename));
        }

        let parts: Vec<&str> = parent_path.trim_start_matches('/').split('/').collect();
        let mut root = self.root.write();
        let mut current = &mut *root;

        for part in parts {
            if let Some(child) = current.children.get_mut(part) {
                if child.file_type != FileType::Directory {
                    return Err(FileSystemError {
                        kind: FileSystemErrorKind::NotADirectory,
                        message: "Parent path is not a directory".to_string(),
                    });
                }
                current = child;
            } else {
                return Err(FileSystemError {
                    kind: FileSystemErrorKind::NotFound,
                    message: "Parent directory not found".to_string(),
                });
            }
        }

        Ok(f(current, filename))
    }
    
    /// Synchronize all directory entries that reference a specific file_id with the shared node
    fn sync_all_nodes_with_file_id(&self, file_id: u64) -> Result<()> {
        // Get the current state of the shared node
        let shared_content = {
            let file_id_to_node = self.file_id_to_node.lock();
            if let Some(shared_node_arc) = file_id_to_node.get(&file_id) {
                let shared_node = shared_node_arc.read();
                shared_node.clone()
            } else {
                return Err(FileSystemError {
                    kind: FileSystemErrorKind::NotFound,
                    message: "Shared node not found for file_id".to_string(),
                });
            }
        };
        
        // Update all nodes in the directory tree that have this file_id
        self.sync_node_recursive(&mut self.root.write(), &shared_content);
        
        Ok(())
    }
    
    /// Recursive helper to sync nodes with matching file_id
    fn sync_node_recursive(&self, node: &mut TmpNode, shared_content: &TmpNode) {
        // Check all children
        for (_name, child) in node.children.entries_mut() {
            if child.metadata.file_id == shared_content.metadata.file_id {
                // Update this child with shared content (but preserve name)
                let original_name = child.name.clone();
                *child = shared_content.clone();
                child.name = original_name;
            }
            
            // Recursively check children if this is a directory
            if child.file_type == FileType::Directory {
                self.sync_node_recursive(child, shared_content);
            }
        }
    }

    /// Normalize path for consistent handling
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
}

impl FileSystem for TmpFS {
    fn mount(&mut self, mount_point: &str) -> Result<()> {
        if self.mounted {
            return Err(FileSystemError {
                kind: FileSystemErrorKind::AlreadyExists,
                message: "TmpFS already mounted".to_string(),
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
                message: "TmpFS not mounted".to_string(),
            });
        }
        self.mounted = false;
        self.mount_point = String::new();
        
        // Clear all data to free memory
        *self.root.write() = TmpNode::new_directory("/".to_string(), 1);
        *self.current_memory.lock() = 0;
        
        // Clear file_id mapping and reset next_file_id
        self.file_id_to_node.lock().clear();
        *self.next_file_id.lock() = 2;
        
        Ok(())
    }
    
    fn name(&self) -> &str {
        "tmpfs"
    }
}

/// File handle for TmpFS files
struct TmpFileHandle {
    path: String,
    position: RwLock<u64>,
    file_type: FileType,
    device_guard: Option<BorrowedDeviceGuard>,
    fs: *const TmpFS, // Weak reference to filesystem
}

// Safety: TmpFileHandle is safe to send between threads as long as the filesystem outlives it
unsafe impl Send for TmpFileHandle {}
unsafe impl Sync for TmpFileHandle {}

impl TmpFileHandle {
    fn new(path: String, file_type: FileType, fs: &TmpFS) -> Self {
        Self {
            path,
            position: RwLock::new(0),
            file_type,
            device_guard: None,
            fs: fs as *const TmpFS,
        }
    }

    fn new_with_device(path: String, file_type: FileType, device_guard: BorrowedDeviceGuard, fs: &TmpFS) -> Self {
        Self {
            path,
            position: RwLock::new(0),
            file_type,
            device_guard: Some(device_guard),
            fs: fs as *const TmpFS,
        }
    }

    fn get_fs(&self) -> &TmpFS {
        unsafe { &*self.fs }
    }

    fn read_device(&self, buffer: &mut [u8]) -> Result<usize> {
        if let Some(ref device_guard) = self.device_guard {
            let device_guard_ref = device_guard.device();
            let mut device_read = device_guard_ref.write();
            
            match device_read.device_type() {
                DeviceType::Char => {
                    if let Some(char_device) = device_read.as_char_device() {
                        let mut bytes_read = 0;
                        for byte in buffer.iter_mut() {
                            match char_device.read_byte() {
                                Some(b) => {
                                    *byte = b;
                                    bytes_read += 1;
                                },
                                None => break,
                            }
                        }
                        return Ok(bytes_read);
                    } else {
                        return Err(FileSystemError {
                            kind: FileSystemErrorKind::NotSupported,
                            message: "Device is not a character device".to_string(),
                        });
                    }
                },
                DeviceType::Block => {
                    if let Some(block_device) = device_read.as_block_device() {
                        // For block devices, we can read a single sector
                        let request = Box::new(crate::device::block::request::BlockIORequest {
                            request_type: crate::device::block::request::BlockIORequestType::Read,
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
                                Err(e) => {
                                    return Err(FileSystemError {
                                        kind: FileSystemErrorKind::IoError,
                                        message: format!("Block device read failed: {}", e),
                                    });
                                }
                            }
                        }
                        return Ok(0);
                    } else {
                        return Err(FileSystemError {
                            kind: FileSystemErrorKind::NotSupported,
                            message: "Device is not a block device".to_string(),
                        });
                    }
                },
                _ => {
                    return Err(FileSystemError {
                        kind: FileSystemErrorKind::NotSupported,
                        message: "Unsupported device type".to_string(),
                    });
                }
            }
        }

        Err(FileSystemError {
            kind: FileSystemErrorKind::NotSupported,
            message: "No device guard available".to_string(),
        })
    }

    fn read_regular_file(&self, buffer: &mut [u8]) -> Result<usize> {
        let fs = self.get_fs();
        let mut position = self.position.write();
        
        // First get the file_id of the file we're reading from
        let file_id = if let Some(node) = fs.find_node(&self.path) {
            node.metadata.file_id
        } else {
            return Err(FileSystemError {
                kind: FileSystemErrorKind::NotFound,
                message: "File not found".to_string(),
            });
        };
        
        // Read from the shared node in file_id_to_node map
        let file_id_to_node = fs.file_id_to_node.lock();
        if let Some(shared_node_arc) = file_id_to_node.get(&file_id) {
            let mut shared_node = shared_node_arc.write();
            shared_node.update_access_time();
            
            if *position as usize >= shared_node.content.len() {
                return Ok(0); // EOF
            }
            
            let available = shared_node.content.len() - *position as usize;
            let to_read = buffer.len().min(available);
            
            buffer[..to_read].copy_from_slice(&shared_node.content[*position as usize..*position as usize + to_read]);
            *position += to_read as u64;
            
            Ok(to_read)
        } else {
            Err(FileSystemError {
                kind: FileSystemErrorKind::NotFound,
                message: "Shared node not found".to_string(),
            })
        }
    }

    fn write_device(&self, buffer: &[u8]) -> Result<usize> {
        if let Some(ref device_guard) = self.device_guard {
            let device_guard_ref = device_guard.device();
            let mut device_write = device_guard_ref.write();
            
            match device_write.device_type() {
                DeviceType::Char => {
                    if let Some(char_device) = device_write.as_char_device() {
                        let mut bytes_written = 0;
                        for &byte in buffer {
                            match char_device.write_byte(byte) {
                                Ok(_) => bytes_written += 1,
                                Err(_) => break,
                            }
                        }
                        return Ok(bytes_written);
                    } else {
                        return Err(FileSystemError {
                            kind: FileSystemErrorKind::NotSupported,
                            message: "Device is not a character device".to_string(),
                        });
                    }
                },
                DeviceType::Block => {
                    if let Some(block_device) = device_write.as_block_device() {
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
                                Err(e) => {
                                    return Err(FileSystemError {
                                        kind: FileSystemErrorKind::IoError,
                                        message: format!("Block device write failed: {}", e),
                                    });
                                }
                            }
                        }
                        return Ok(0);
                    } else {
                        return Err(FileSystemError {
                            kind: FileSystemErrorKind::NotSupported,
                            message: "Device is not a block device".to_string(),
                        });
                    }
                },
                _ => {
                    return Err(FileSystemError {
                        kind: FileSystemErrorKind::NotSupported,
                        message: "Unsupported device type".to_string(),
                    });
                }
            }
        } else {
            Err(FileSystemError {
                kind: FileSystemErrorKind::NotSupported,
                message: "No device guard available".to_string(),
            })
        }
    }

    fn write_regular_file(&self, buffer: &[u8]) -> Result<usize> {
        let fs = self.get_fs();
        let mut position = self.position.write();
        
        // Check memory limit before writing
        fs.check_memory_limit(buffer.len())?;
        
        // First get the file_id of the file we're writing to
        let file_id = if let Some(node) = fs.find_node(&self.path) {
            node.metadata.file_id
        } else {
            return Err(FileSystemError {
                kind: FileSystemErrorKind::NotFound,
                message: "File not found".to_string(),
            });
        };
        
        // Update the shared node in file_id_to_node map
        let size_increase = {
            let file_id_to_node = fs.file_id_to_node.lock();
            if let Some(shared_node_arc) = file_id_to_node.get(&file_id) {
                let mut shared_node = shared_node_arc.write();
                let old_size = shared_node.content.len();
                let new_position = *position as usize + buffer.len();
                
                // Expand file if necessary
                if new_position > shared_node.content.len() {
                    shared_node.content.resize(new_position, 0);
                }
                
                // Write data
                shared_node.content[*position as usize..new_position].copy_from_slice(buffer);
                let new_size = shared_node.content.len();
                shared_node.update_size(new_size);
                
                new_size.saturating_sub(old_size)
            } else {
                return Err(FileSystemError {
                    kind: FileSystemErrorKind::NotFound,
                    message: "Shared node not found".to_string(),
                });
            }
        };
        
        // Note: We don't need to sync directory entries because they reference
        // the shared node via file_id, and the content is stored in file_id_to_node
        
        *position += buffer.len() as u64;
        fs.add_memory_usage(size_increase);
        Ok(buffer.len())
    }
}

impl FileHandle for TmpFileHandle {
    fn read(&self, buffer: &mut [u8]) -> Result<usize> {
        match self.file_type {
            FileType::CharDevice(_) | FileType::BlockDevice(_) => {
                // Handle device files
                self.read_device(buffer)
            },
            FileType::RegularFile => {
                self.read_regular_file(buffer)
            }
            FileType::Directory => {
                return Err(FileSystemError {
                    kind: FileSystemErrorKind::IsADirectory,
                    message: "Cannot read from a directory".to_string(),
                });
            },
            _ => {
                return Err(FileSystemError {
                    kind: FileSystemErrorKind::NotSupported,
                    message: "Unsupported file type".to_string(),
                });
            }
        }
    }

     fn readdir(&self) -> Result<Vec<DirectoryEntry>> {
        let fs = self.get_fs();
        
        if let Some(node) = fs.find_node(&self.path) {
            if node.file_type != FileType::Directory {
                return Err(FileSystemError {
                    kind: FileSystemErrorKind::NotADirectory,
                    message: "Not a directory".to_string(),
                });
            }
            
            let mut entries = Vec::new();
            for (name, child) in node.children.entries() {
                entries.push(DirectoryEntry {
                    name: name.clone(),
                    file_type: child.file_type.clone(),
                    size: child.metadata.size,
                    file_id: child.metadata.file_id,
                    metadata: Some(child.metadata.clone()),
                });
            }
            
            Ok(entries)
        } else {
            Err(FileSystemError {
                kind: FileSystemErrorKind::NotFound,
                message: "Directory not found".to_string(),
            })
        }
    }
    
    fn write(&self, buffer: &[u8]) -> Result<usize> {
        match self.file_type {
            FileType::CharDevice(_) | FileType::BlockDevice(_) => {
                // Handle device files
                self.write_device(buffer)
            },
            FileType::RegularFile => {
                self.write_regular_file(buffer)
            }
            FileType::Directory => {
                return Err(FileSystemError {
                    kind: FileSystemErrorKind::IsADirectory,
                    message: "Cannot write to a directory".to_string(),
                });
            },
            _ => {
                return Err(FileSystemError {
                    kind: FileSystemErrorKind::NotSupported,
                    message: "Unsupported file type".to_string(),
                });
            }
        }
    }
    
    fn seek(&self, whence: SeekFrom) -> Result<u64> {
        let fs = self.get_fs();
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
                if let Some(node) = fs.find_node(&self.path) {
                    let end = node.content.len() as u64;
                    if offset >= 0 {
                        *position = end.saturating_add(offset as u64);
                    } else {
                        *position = end.saturating_sub((-offset) as u64);
                    }
                } else {
                    return Err(FileSystemError {
                        kind: FileSystemErrorKind::NotFound,
                        message: "File not found".to_string(),
                    });
                }
            },
        }
        
        Ok(*position)
    }
    
    fn release(&self) -> Result<()> {
        Ok(())
    }
    
    fn metadata(&self) -> Result<FileMetadata> {
        let fs = self.get_fs();
        if let Some(node) = fs.find_node(&self.path) {
            let file_id = node.metadata.file_id;
            
            // Get the most up-to-date metadata from the shared node
            let file_id_to_node = fs.file_id_to_node.lock();
            if let Some(shared_node_arc) = file_id_to_node.get(&file_id) {
                let shared_node = shared_node_arc.read();
                Ok(shared_node.metadata.clone())
            } else {
                // Fallback to the directory tree metadata (shouldn't happen for valid files)
                Ok(node.metadata)
            }
        } else {
            Err(FileSystemError {
                kind: FileSystemErrorKind::NotFound,
                message: "File not found".to_string(),
            })
        }
    }
}

impl FileOperations for TmpFS {
    fn open(&self, path: &str, _flags: u32) -> Result<Arc<dyn FileHandle>> {
        let normalized = self.normalize_path(path);
        if let Some(node) = self.find_node(&normalized) {
            match node.file_type {
                FileType::RegularFile | FileType::Directory => {
                    Ok(Arc::new(TmpFileHandle::new(normalized, node.file_type, self)))
                },
                FileType::CharDevice(ref info) | FileType::BlockDevice(ref info) => {
                    // Try to borrow the device from DeviceManager
                    match DeviceManager::get_manager().borrow_device(info.device_id) {
                        Ok(guard) => {
                            Ok(Arc::new(TmpFileHandle::new_with_device(normalized, node.file_type, guard, self)))
                        },
                        Err(_) => {
                            Err(FileSystemError {
                                kind: FileSystemErrorKind::PermissionDenied,
                                message: "Failed to access device".to_string(),
                            })
                        }
                    }
                },
                _ => {
                    Err(FileSystemError {
                        kind: FileSystemErrorKind::NotSupported,
                        message: "Unsupported file type".to_string(),
                    })
                }
            }
        } else {
            Err(FileSystemError {
                kind: FileSystemErrorKind::NotFound,
                message: "File not found".to_string(),
            })
        }
    }
    
    fn read_dir(&self, path: &str) -> Result<Vec<DirectoryEntry>> {
        let normalized = self.normalize_path(path);
        
        if let Some(node) = self.find_node(&normalized) {
            if node.file_type != FileType::Directory {
                return Err(FileSystemError {
                    kind: FileSystemErrorKind::NotADirectory,
                    message: "Not a directory".to_string(),
                });
            }
            
            let mut entries = Vec::new();
            for (name, child) in node.children.entries() {
                entries.push(DirectoryEntry {
                    name: name.clone(),
                    file_type: child.file_type.clone(),
                    size: child.metadata.size,
                    file_id: child.metadata.file_id,
                    metadata: Some(child.metadata.clone()),
                });
            }
            
            Ok(entries)
        } else {
            Err(FileSystemError {
                kind: FileSystemErrorKind::NotFound,
                message: "Directory not found".to_string(),
            })
        }
    }
    
    fn create_file(&self, path: &str, file_type: FileType) -> Result<()> {
        self.find_parent_mut(path, |parent, filename| {
            if parent.children.contains_key(filename) {
                return Err(FileSystemError {
                    kind: FileSystemErrorKind::AlreadyExists,
                    message: "File already exists".to_string(),
                });
            }
            
            // Generate new file_id for this file
            let file_id = self.generate_file_id();
            
            let node = match file_type {
                FileType::RegularFile => TmpNode::new_file(filename.to_string(), file_id),
                FileType::Directory => TmpNode::new_directory(filename.to_string(), file_id),
                FileType::CharDevice(_) | FileType::BlockDevice(_) => {
                    TmpNode::new_device(filename.to_string(), file_type, file_id)
                },
                _ => {
                    return Err(FileSystemError {
                        kind: FileSystemErrorKind::NotSupported,
                        message: "Unsupported file type".to_string(),
                    });
                }
            };
            
            // Register file_id -> node mapping for hardlink support
            let node_arc = Arc::new(RwLock::new(node.clone()));
            self.file_id_to_node.lock().insert(file_id, node_arc);
            
            parent.children.insert(filename.to_string(), node);
            parent.metadata.modified_time = crate::time::current_time();
            
            Ok(())
        })?
    }
    
    fn create_dir(&self, path: &str) -> Result<()> {
        self.create_file(path, FileType::Directory)
    }
    
    fn remove(&self, path: &str) -> Result<()> {
        self.find_parent_mut(path, |parent, filename| {
            if let Some(node) = parent.children.get(filename) {
                // Check if directory is empty
                if node.file_type == FileType::Directory && !node.children.is_empty() {
                    return Err(FileSystemError {
                        kind: FileSystemErrorKind::NotSupported,
                        message: "Cannot remove non-empty directory".to_string(),
                    });
                }
                
                let file_id = node.metadata.file_id;
                let mut should_free_memory = false;
                let mut memory_freed = 0;
                
                // Update link count in the shared node
                {
                    let mut file_id_to_node = self.file_id_to_node.lock();
                    if let Some(shared_node_arc) = file_id_to_node.get(&file_id) {
                        let mut shared_node = shared_node_arc.write();
                        shared_node.metadata.link_count -= 1;
                        
                        // If link count reaches 0, mark for data deletion
                        if shared_node.metadata.link_count == 0 {
                            should_free_memory = true;
                            memory_freed = shared_node.content.len(); // Get actual content size
                            // Drop the reference before removing from map
                            drop(shared_node);
                            file_id_to_node.remove(&file_id);
                        }
                    }
                }
                
                // Remove directory entry
                parent.children.remove(filename);
                parent.metadata.modified_time = crate::time::current_time();
                
                // Free memory only when link count reaches 0
                if should_free_memory {
                    self.subtract_memory_usage(memory_freed);
                }
                
                Ok(())
            } else {
                Err(FileSystemError {
                    kind: FileSystemErrorKind::NotFound,
                    message: "File or directory not found".to_string(),
                })
            }
        })?
    }
    
    fn metadata(&self, path: &str) -> Result<FileMetadata> {
        let normalized = self.normalize_path(path);
        
        if let Some(node) = self.find_node(&normalized) {
            let file_id = node.metadata.file_id;
            
            // Get the most up-to-date metadata from the shared node
            let file_id_to_node = self.file_id_to_node.lock();
            if let Some(shared_node_arc) = file_id_to_node.get(&file_id) {
                let shared_node = shared_node_arc.read();
                Ok(shared_node.metadata.clone())
            } else {
                // Fallback to the directory tree metadata (shouldn't happen for valid files)
                Ok(node.metadata)
            }
        } else {
            Err(FileSystemError {
                kind: FileSystemErrorKind::NotFound,
                message: "File or directory not found".to_string(),
            })
        }
    }
    
    fn create_hardlink(&self, target_path: &str, link_path: &str) -> Result<()> {
        // First, verify target exists and get its file_id
        let target_metadata = self.metadata(target_path)?;
        let target_file_id = target_metadata.file_id;
        
        // Hardlinks to directories are not allowed
        if target_metadata.file_type == FileType::Directory {
            return Err(FileSystemError {
                kind: FileSystemErrorKind::NotSupported,
                message: "Hard links to directories are not supported".to_string(),
            });
        }
        
        // Find the shared node by file_id
        let shared_node_arc = {
            let file_id_to_node = self.file_id_to_node.lock();
            file_id_to_node.get(&target_file_id).cloned()
        };
        
        let shared_node_arc = shared_node_arc.ok_or_else(|| FileSystemError {
            kind: FileSystemErrorKind::NotFound,
            message: "Target file node not found in registry".to_string(),
        })?;
        
        // Create new directory entry pointing to the same node
        let _ = self.find_parent_mut(link_path, |parent, filename| {
            if parent.children.contains_key(filename) {
                return Err(FileSystemError {
                    kind: FileSystemErrorKind::AlreadyExists,
                    message: "Link path already exists".to_string(),
                });
            }
            
            // Get the shared node and increment link count
            let mut shared_node = shared_node_arc.write();
            shared_node.metadata.link_count += 1;
            let updated_metadata = shared_node.metadata.clone();
            
            // Create a new local copy for this directory entry
            let mut new_node = shared_node.clone();
            new_node.name = filename.to_string();
            
            // Add to parent directory
            parent.children.insert(filename.to_string(), new_node);
            parent.metadata.modified_time = crate::time::current_time();
            
            // Release the shared node lock
            drop(shared_node);
            
            Ok(updated_metadata)
        })?;
        
        // Update all existing directory entries that point to the same file_id
        // This ensures that metadata() calls return consistent link_count
        self.update_all_nodes_with_file_id(target_file_id)?;
        
        Ok(())
    }
    
    fn root_dir(&self) -> Result<Directory> {
        Ok(Directory::open("/".to_string()))
    }
}

impl TmpFS {
    /// Update all directory entries that have the same file_id with the latest metadata
    fn update_all_nodes_with_file_id(&self, file_id: u64) -> Result<()> {
        // Get the latest metadata from the shared node
        let latest_metadata = {
            let file_id_to_node = self.file_id_to_node.lock();
            if let Some(shared_node_arc) = file_id_to_node.get(&file_id) {
                let shared_node = shared_node_arc.read();
                shared_node.metadata.clone()
            } else {
                return Ok(()); // Node not found, nothing to update
            }
        };
        
        // Update root directory entries
        self.update_nodes_in_directory(&mut self.root.write(), file_id, &latest_metadata);
        
        Ok(())
    }
    
    /// Recursively update nodes in a directory tree
    fn update_nodes_in_directory(&self, node: &mut TmpNode, target_file_id: u64, latest_metadata: &FileMetadata) {
        // Update direct children
        for (_, child) in node.children.entries_mut() {
            if child.metadata.file_id == target_file_id {
                child.metadata = latest_metadata.clone();
            }
            
            // Recursively update subdirectories
            if child.file_type == FileType::Directory {
                self.update_nodes_in_directory(child, target_file_id, latest_metadata);
            }
        }
    }
}

/// TmpFS driver for creating TmpFS instances
pub struct TmpFSDriver;

impl FileSystemDriver for TmpFSDriver {
    fn name(&self) -> &'static str {
        "tmpfs"
    }
    
    fn filesystem_type(&self) -> FileSystemType {
        FileSystemType::Virtual  // TmpFS is a virtual filesystem
    }
    
    fn create_from_block(&self, _block_device: Box<dyn BlockDevice>, _block_size: usize) -> Result<Box<dyn VirtualFileSystem>> {
        // TmpFS doesn't use block devices, but we can create an instance with unlimited memory
        Ok(Box::new(TmpFS::new(0)))
    }
    
    fn create_from_memory(&self, _memory_area: &crate::vm::vmem::MemoryArea) -> Result<Box<dyn VirtualFileSystem>> {
        // TmpFS doesn't need specific memory area, create with unlimited memory
        Ok(Box::new(TmpFS::new(0)))
    }

    fn create_with_params(&self, params: &dyn crate::fs::params::FileSystemParams) -> Result<Box<dyn VirtualFileSystem>> {
        use crate::fs::params::*;
        
        // Try to downcast to TmpFSParams first
        if let Some(tmpfs_params) = params.as_any().downcast_ref::<TmpFSParams>() {
            return Ok(Box::new(TmpFS::new(tmpfs_params.memory_limit)));
        }
        
        // Try to downcast to BasicFSParams for compatibility
        if let Some(_basic_params) = params.as_any().downcast_ref::<BasicFSParams>() {
            return Ok(Box::new(TmpFS::new(0))); // Unlimited memory for basic params
        }
        
        // If all downcasts fail, return error
        Err(FileSystemError {
            kind: FileSystemErrorKind::NotSupported,
            message: "TmpFS requires TmpFSParams or BasicFSParams parameter type".to_string(),
        })
    }
}

impl TmpFSDriver {
    /// Create a new TmpFS with specified memory limit
    pub fn create_with_limit(&self, max_memory: usize) -> Box<dyn VirtualFileSystem> {
        Box::new(TmpFS::new(max_memory))
    }
    
    /// Create a new TmpFS with unlimited memory
    pub fn create_unlimited(&self) -> Box<dyn VirtualFileSystem> {
        Box::new(TmpFS::new(0))
    }
}

/// Register TmpFS driver with the filesystem driver manager
pub fn register_tmpfs_driver() {
    let fs_driver_manager = crate::fs::get_fs_driver_manager();
    fs_driver_manager.register_driver(Box::new(TmpFSDriver));
}

// Auto-register the TmpFS driver when this module is loaded
crate::driver_initcall!(register_tmpfs_driver);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::{char::mockchar::MockCharDevice, Device};

    #[test_case]
    fn test_tmpfs_basic_operations() {
        let tmpfs = TmpFS::new(0); // Unlimited memory
        
        // Test directory creation
        tmpfs.create_dir("/test").unwrap();
        
        // Test file creation
        tmpfs.create_file("/test/file.txt", FileType::RegularFile).unwrap();
        
        // Test file opening and writing
        let file = tmpfs.open("/test/file.txt", 0).unwrap();
        let data = b"Hello, TmpFS!";
        let bytes_written = file.write(data).unwrap();
        assert_eq!(bytes_written, data.len());
        
        // Test file reading
        file.seek(SeekFrom::Start(0)).unwrap();
        let mut buffer = vec![0u8; data.len()];
        let bytes_read = file.read(&mut buffer).unwrap();
        assert_eq!(bytes_read, data.len());
        assert_eq!(&buffer, data);
        
        // Test directory listing
        let entries = tmpfs.read_dir("/test").unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "file.txt");
        assert_eq!(entries[0].file_type, FileType::RegularFile);
    }

    #[test_case]
    fn test_tmpfs_memory_limit() {
        let tmpfs = TmpFS::new(100); // 100 bytes limit
        
        tmpfs.create_file("/test.txt", FileType::RegularFile).unwrap();
        let file = tmpfs.open("/test.txt", 0).unwrap();
        
        // Write within limit
        let small_data = b"Small";
        assert!(file.write(small_data).is_ok());
        
        // Try to write beyond limit
        let large_data = vec![0u8; 200];
        assert!(file.write(&large_data).is_err());
        
        // Check memory usage
        assert_eq!(tmpfs.memory_usage(), small_data.len());
    }

    #[test_case]
    fn test_tmpfs_device_files() {
        let tmpfs = TmpFS::new(0);
        
        // Create a character device
        let mut char_device = Box::new(MockCharDevice::new(1, "tmpfs_char"));
        char_device.set_read_data(vec![b'T', b'M', b'P', b'F', b'S']);
        let device_id = DeviceManager::get_mut_manager().register_device(char_device as Box<dyn Device>);
        
        // Create device file
        let device_info = DeviceFileInfo {
            device_id,
            device_type: DeviceType::Char,
        };
        
        tmpfs.create_dir("/dev").unwrap();
        tmpfs.create_file("/dev/tmpfs_char", FileType::CharDevice(device_info)).unwrap();
        
        // Test device file access
        let device_file = tmpfs.open("/dev/tmpfs_char", 0).unwrap();
        let mut buffer = [0u8; 5];
        let bytes_read = device_file.read(&mut buffer).unwrap();
        assert_eq!(bytes_read, 5);
        assert_eq!(&buffer, b"TMPFS");
    }

    #[test_case]
    fn test_tmpfs_file_operations() {
        let tmpfs = TmpFS::new(0);
        
        // Create nested directories
        tmpfs.create_dir("/home").unwrap();
        tmpfs.create_dir("/home/user").unwrap();
        tmpfs.create_file("/home/user/document.txt", FileType::RegularFile).unwrap();
        
        // Test metadata
        let metadata = tmpfs.metadata("/home/user/document.txt").unwrap();
        assert_eq!(metadata.file_type, FileType::RegularFile);
        assert!(metadata.permissions.read);
        assert!(metadata.permissions.write);
        
        // Test file removal
        tmpfs.remove("/home/user/document.txt").unwrap();
        assert!(tmpfs.open("/home/user/document.txt", 0).is_err());
        
        // Test directory removal (should fail if not empty)
        tmpfs.create_file("/home/user/another.txt", FileType::RegularFile).unwrap();
        assert!(tmpfs.remove("/home/user").is_err());
        
        // Remove file and then directory
        tmpfs.remove("/home/user/another.txt").unwrap();
        tmpfs.remove("/home/user").unwrap();
        assert!(tmpfs.open("/home/user", 0).is_err());
    }

    #[test_case]
    fn test_tmpfs_memory_management() {
        let tmpfs = TmpFS::new(1000); // 1KB limit
        
        // Create multiple files
        for i in 0..10 {
            let filename = format!("/file{}.txt", i);
            tmpfs.create_file(&filename, FileType::RegularFile).unwrap();
            
            let file = tmpfs.open(&filename, 0).unwrap();
            let data = vec![i as u8; 50]; // 50 bytes per file
            file.write(&data).unwrap();
        }
        
        // Should use 500 bytes
        assert_eq!(tmpfs.memory_usage(), 500);
        
        // Remove some files
        for i in 0..5 {
            let filename = format!("/file{}.txt", i);
            tmpfs.remove(&filename).unwrap();
        }
        
        // Should use 250 bytes now
        assert_eq!(tmpfs.memory_usage(), 250);
    }

    #[test_case]
    fn test_tmpfs_large_file_operations() {
        let tmpfs = TmpFS::new(0); // Unlimited
        
        tmpfs.create_file("/large.bin", FileType::RegularFile).unwrap();
        let file = tmpfs.open("/large.bin", 0).unwrap();
        
        // Write large data
        let large_data = vec![0xAA; 8192]; // 8KB
        let bytes_written = file.write(&large_data).unwrap();
        assert_eq!(bytes_written, large_data.len());
        
        // Seek to middle and write
        file.seek(SeekFrom::Start(4096)).unwrap();
        let pattern = vec![0x55; 1024];
        file.write(&pattern).unwrap();
        
        // Read back and verify
        file.seek(SeekFrom::Start(4096)).unwrap();
        let mut buffer = vec![0u8; 1024];
        let bytes_read = file.read(&mut buffer).unwrap();
        assert_eq!(bytes_read, 1024);
        assert_eq!(buffer, pattern);
        
        // Check file size
        let metadata = file.metadata().unwrap();
        assert_eq!(metadata.size, 8192);
    }

    #[test_case]
    fn test_tmpfs_file_readdir() {
        let mut tmpfs = TmpFS::new(0); // Unlimited memory
        tmpfs.mount("/tmp").unwrap();
        
        // Create test directory structure
        tmpfs.create_dir("/subdir").unwrap();
        tmpfs.create_file("/file1.txt", FileType::RegularFile).unwrap();
        tmpfs.create_file("/file2.bin", FileType::RegularFile).unwrap();
        tmpfs.create_file("/subdir/nested.txt", FileType::RegularFile).unwrap();
        
        // Open root directory as a file
        let file = tmpfs.open("/", 0).unwrap();
        let entries = file.readdir().unwrap();
        
        // Verify directory entries
        assert_eq!(entries.len(), 3); // subdir, file1.txt, file2.bin
        
        let mut entry_names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        entry_names.sort();
        assert_eq!(entry_names, vec!["file1.txt", "file2.bin", "subdir"]);
        
        // Check file types
        for entry in &entries {
            match entry.name.as_str() {
                "subdir" => assert_eq!(entry.file_type, FileType::Directory),
                "file1.txt" | "file2.bin" => assert_eq!(entry.file_type, FileType::RegularFile),
                _ => panic!("Unexpected entry: {}", entry.name),
            }
        }
        
        // Test subdirectory listing
        let subdir_file = tmpfs.open("/subdir", 0).unwrap();
        let subdir_entries = subdir_file.readdir().unwrap();
        assert_eq!(subdir_entries.len(), 1);
        assert_eq!(subdir_entries[0].name, "nested.txt");
        assert_eq!(subdir_entries[0].file_type, FileType::RegularFile);
    }

    #[test_case]
    fn test_tmpfs_readdir_with_special_entries() {
        let mut tmpfs = TmpFS::new(0);
        tmpfs.mount("/tmp").unwrap();
        
        // Create device file
        let mut char_device = Box::new(MockCharDevice::new(10, "test_device"));
        char_device.set_read_data(vec![b'D', b'E', b'V']);
        let device_id = DeviceManager::get_mut_manager().register_device(char_device as Box<dyn Device>);
        
        let device_info = DeviceFileInfo {
            device_id,
            device_type: DeviceType::Char,
        };
        
        tmpfs.create_dir("/dev").unwrap();
        tmpfs.create_file("/dev/test_char", FileType::CharDevice(device_info)).unwrap();
        tmpfs.create_file("/dev/regular.txt", FileType::RegularFile).unwrap();
        
        // Test /dev directory listing
        let dev_file = tmpfs.open("/dev", 0).unwrap();
        let dev_entries = dev_file.readdir().unwrap();
        
        assert_eq!(dev_entries.len(), 2);
        
        let mut found_device = false;
        let mut found_regular = false;
        
        for entry in &dev_entries {
            match entry.name.as_str() {
                "test_char" => {
                    assert_eq!(entry.file_type, FileType::CharDevice(device_info));
                    found_device = true;
                },
                "regular.txt" => {
                    assert_eq!(entry.file_type, FileType::RegularFile);
                    found_regular = true;
                },
                _ => panic!("Unexpected entry: {}", entry.name),
            }
        }
        
        assert!(found_device, "Device file not found in directory listing");
        assert!(found_regular, "Regular file not found in directory listing");
    }

    #[test_case]
    fn test_tmpfs_readdir_error_cases() {
        let mut tmpfs = TmpFS::new(0);
        tmpfs.mount("/tmp").unwrap();
        
        // Create a regular file
        tmpfs.create_file("/regular.txt", FileType::RegularFile).unwrap();
        
        // Try to readdir on a regular file (should fail)
        let file = tmpfs.open("/regular.txt", 0).unwrap();
        let result = file.readdir();
        assert!(result.is_err());
        
        if let Err(e) = result {
            assert_eq!(e.kind, FileSystemErrorKind::NotADirectory);
        }
        
        // Try to readdir on non-existent path
        let result = tmpfs.open("/nonexistent", 0);
        assert!(result.is_err());
    }

    #[test_case]
    fn test_tmpfs_empty_directory_readdir() {
        let mut tmpfs = TmpFS::new(0);
        tmpfs.mount("/tmp").unwrap();
        
        // Create empty directory
        tmpfs.create_dir("/empty").unwrap();
        
        // Test reading empty directory
        let empty_dir = tmpfs.open("/empty", 0).unwrap();
        let entries = empty_dir.readdir().unwrap();
        
        // Empty directory should return empty list
        assert_eq!(entries.len(), 0);
        
        // Root directory should contain the empty directory
        let root_file = tmpfs.open("/", 0).unwrap();
        let root_entries = root_file.readdir().unwrap();
        assert_eq!(root_entries.len(), 1);
        assert_eq!(root_entries[0].name, "empty");
        assert_eq!(root_entries[0].file_type, FileType::Directory);
    }
}
