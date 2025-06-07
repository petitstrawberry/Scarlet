use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::sync::Arc;
use spin::rwlock::RwLock;
use spin::Mutex;
use core::any::Any;
use core::mem;
extern crate alloc;

use crate::fs::params::FileSystemParams;
use crate::fs::*;
use crate::device::block::BlockDevice;
use crate::device::manager::{BorrowedDeviceGuard, DeviceManager};
use crate::device::DeviceType;

/// xv6-style directory entry
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct Dirent {
    pub inum: u16,      // inode number
    pub name: [u8; 14], // file name (null-terminated)
}

impl Dirent {
    pub const DIRENT_SIZE: usize = mem::size_of::<Dirent>();
    
    pub fn new(inum: u16, name: &str) -> Self {
        let mut dirent = Dirent {
            inum,
            name: [0; 14],
        };
        
        // Copy name, ensuring null termination
        let name_bytes = name.as_bytes();
        let copy_len = name_bytes.len().min(13); // Leave space for null terminator
        dirent.name[..copy_len].copy_from_slice(&name_bytes[..copy_len]);
        dirent.name[copy_len] = 0; // Null terminate
        
        dirent
    }
    
    pub fn name_str(&self) -> &str {
        // Find null terminator
        let mut end = 0;
        while end < self.name.len() && self.name[end] != 0 {
            end += 1;
        }
        
        // Convert to string
        core::str::from_utf8(&self.name[..end]).unwrap_or("")
    }
    
    /// Convert Dirent to byte array for reading
    pub fn as_bytes(&self) -> &[u8] {
        unsafe {
            core::slice::from_raw_parts(
                self as *const Dirent as *const u8,
                mem::size_of::<Dirent>()
            )
        }
    }
}

/// xv6-style file stat structure
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct Stat {
    pub dev: i32,     // File system's disk device
    pub ino: u32,     // Inode number
    pub file_type: u16, // Type of file (T_DIR, T_FILE, T_DEVICE)
    pub nlink: u16,     // Number of links to file
    pub size: u64,    // Size of file in bytes
}

// xv6 file type constants
pub const T_DIR: u16 = 1;    // Directory
pub const T_FILE: u16 = 2;   // File
pub const T_DEVICE: u16 = 3; // Device

/// Directory entries collection for xv6fs
#[derive(Clone, Default)]
struct Xv6DirectoryEntries {
    entries: BTreeMap<String, Xv6Node>,
    inode_counter: u16,
}

impl Xv6DirectoryEntries {
    fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
            inode_counter: 2, // Start from 2 (0 is invalid, 1 is root)
        }
    }

    fn insert(&mut self, name: String, mut node: Xv6Node) -> Option<Xv6Node> {
        if !self.entries.contains_key(&name) {
            node.inode_number = self.inode_counter;
            self.inode_counter += 1;
        }
        self.entries.insert(name, node)
    }

    fn remove(&mut self, name: &str) -> Option<Xv6Node> {
        self.entries.remove(name)
    }

    fn get(&self, name: &str) -> Option<&Xv6Node> {
        self.entries.get(name)
    }

    fn get_mut(&mut self, name: &str) -> Option<&mut Xv6Node> {
        self.entries.get_mut(name)
    }

    fn contains_key(&self, name: &str) -> bool {
        self.entries.contains_key(name)
    }

    fn entries(&self) -> impl Iterator<Item = (&String, &Xv6Node)> {
        self.entries.iter()
    }

    fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    
    fn len(&self) -> usize {
        self.entries.len()
    }
}

/// Node in the xv6fs filesystem
#[derive(Clone)]
struct Xv6Node {
    name: String,
    file_type: FileType,
    content: Vec<u8>,
    metadata: FileMetadata,
    children: Xv6DirectoryEntries,
    inode_number: u16,
}

impl Xv6Node {
    fn new_file(name: String) -> Self {
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
            },
            children: Xv6DirectoryEntries::new(),
            inode_number: 0, // Will be set when added to parent
        }
    }

    fn new_directory(name: String) -> Self {
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
            },
            children: Xv6DirectoryEntries::new(),
            inode_number: 0, // Will be set when added to parent
        }
    }

    fn new_device(name: String, file_type: FileType) -> Self {
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
            },
            children: Xv6DirectoryEntries::new(),
            inode_number: 0, // Will be set when added to parent
        }
    }

    /// Generate directory content as serialized Dirent entries
    fn generate_directory_content(&self, parent_inum: u16) -> Vec<u8> {
        let mut content = Vec::new();
        
        // Add "." entry (current directory)
        let current_dirent = Dirent::new(self.inode_number, ".");
        content.extend_from_slice(current_dirent.as_bytes());
        
        // Add ".." entry (parent directory)
        let parent_dirent = Dirent::new(parent_inum, "..");
        content.extend_from_slice(parent_dirent.as_bytes());
        
        // Add all child entries
        for (name, child) in self.children.entries() {
            let dirent = Dirent::new(child.inode_number, name);
            content.extend_from_slice(dirent.as_bytes());
        }
        
        content
    }

    /// Update file size and modification time
    fn update_size(&mut self, new_size: usize) {
        self.metadata.size = new_size;
        self.metadata.modified_time = crate::time::current_time();
    }
}

/// Xv6FS - xv6-compatible filesystem based on tmpfs
pub struct Xv6FS {
    mounted: bool,
    mount_point: String,
    root: RwLock<Xv6Node>,
    max_memory: usize,
    current_memory: Mutex<usize>,
}

impl Xv6FS {
    pub fn new(max_memory: usize) -> Self {
        let mut root = Xv6Node::new_directory("/".to_string());
        root.inode_number = 1; // Root directory always has inode 1
        
        Self {
            mounted: false,
            mount_point: String::new(),
            root: RwLock::new(root),
            max_memory,
            current_memory: Mutex::new(0),
        }
    }

    fn normalize_path(&self, path: &str) -> String {
        if !path.starts_with('/') {
            return format!("/{}", path);
        }
        
        let mut normalized_components = Vec::new();
        let components: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        
        for component in components {
            match component {
                "." => continue,
                ".." => {
                    if !normalized_components.is_empty() {
                        normalized_components.pop();
                    }
                }
                comp => normalized_components.push(comp),
            }
        }
        
        if normalized_components.is_empty() {
            "/".to_string()
        } else {
            format!("/{}", normalized_components.join("/"))
        }
    }

    fn find_node(&self, path: &str) -> Option<Xv6Node> {
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
        F: FnOnce(&mut Xv6Node) -> R,
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

    fn find_parent_and_name(&self, path: &str) -> Option<(Xv6Node, String)> {
        let normalized = self.normalize_path(path);
        
        if normalized == "/" {
            return None; // Root has no parent
        }

        let parts: Vec<&str> = normalized.trim_start_matches('/').split('/').collect();
        if parts.is_empty() {
            return None;
        }

        let parent_path = if parts.len() == 1 {
            "/"
        } else {
            &normalized[..normalized.rfind('/').unwrap()]
        };

        let parent = self.find_node(parent_path)?;
        Some((parent, parts.last().unwrap().to_string()))
    }

    fn check_memory_limit(&self, additional_bytes: usize) -> Result<()> {
        if self.max_memory > 0 {
            let current = *self.current_memory.lock();
            if current + additional_bytes > self.max_memory {
                return Err(FileSystemError {
                    kind: FileSystemErrorKind::NoSpace,
                    message: "Insufficient memory".to_string(),
                });
            }
        }
        Ok(())
    }

    fn add_memory_usage(&self, bytes: usize) {
        let mut current = self.current_memory.lock();
        *current += bytes;
    }

    fn subtract_memory_usage(&self, bytes: usize) {
        let mut current = self.current_memory.lock();
        *current = current.saturating_sub(bytes);
    }
}

impl FileSystem for Xv6FS {
    fn mount(&mut self, mount_point: &str) -> Result<()> {
        if self.mounted {
            return Err(FileSystemError {
                kind: FileSystemErrorKind::AlreadyExists,
                message: "Already mounted".to_string(),
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
                message: "Not mounted".to_string(),
            });
        }
        self.mounted = false;
        self.mount_point = String::new();
        Ok(())
    }

    fn name(&self) -> &str {
        "xv6fs"
    }
}

/// File handle for Xv6FS files
struct Xv6FileHandle {
    path: String,
    position: RwLock<u64>,
    file_type: FileType,
    device_guard: Option<BorrowedDeviceGuard>,
    fs: *const Xv6FS,
    /// Cached directory content for directory files
    directory_content: Option<RwLock<Vec<u8>>>,
    inode_number: u16,
}

unsafe impl Send for Xv6FileHandle {}
unsafe impl Sync for Xv6FileHandle {}

impl Xv6FileHandle {
    fn new(path: String, file_type: FileType, inode_number: u16, fs: &Xv6FS) -> Self {
        let directory_content = if matches!(file_type, FileType::Directory) {
            // Generate directory content when handle is created
            let node = fs.find_node(&path).unwrap();
            let parent_inum = if path == "/" { 1 } else {
                fs.find_parent_and_name(&path)
                    .map(|(parent, _)| parent.inode_number)
                    .unwrap_or(1)
            };
            let content = node.generate_directory_content(parent_inum);
            Some(RwLock::new(content))
        } else {
            None
        };

        Self {
            path,
            position: RwLock::new(0),
            file_type,
            device_guard: None,
            fs: fs as *const Xv6FS,
            directory_content,
            inode_number,
        }
    }

    fn new_with_device(
        path: String,
        file_type: FileType,
        inode_number: u16,
        device_guard: BorrowedDeviceGuard,
        fs: &Xv6FS,
    ) -> Self {
        Self {
            path,
            position: RwLock::new(0),
            file_type,
            device_guard: Some(device_guard),
            fs: fs as *const Xv6FS,
            directory_content: None,
            inode_number,
        }
    }

    fn fs(&self) -> &Xv6FS {
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
        let fs = self.fs();
        let mut position = self.position.write();
        
        if let Some(node) = fs.find_node(&self.path) {            
            if *position as usize >= node.content.len() {
                return Ok(0); // EOF
            }
            
            let available = node.content.len() - *position as usize;
            let to_read = buffer.len().min(available);
            
            buffer[..to_read].copy_from_slice(&node.content[*position as usize..*position as usize + to_read]);
            *position += to_read as u64;
            Ok(to_read)
        } else {
            Err(FileSystemError {
                kind: FileSystemErrorKind::NotFound,
                message: "File not found".to_string(),
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
        let fs = self.fs();
        let mut position = self.position.write();
        
        // Check memory limit before writing
        fs.check_memory_limit(buffer.len())?;
        
        match fs.find_node_mut(&self.path, |n| {
            let old_size = n.content.len();
            let new_position = *position as usize + buffer.len();
            
            // Expand file if necessary
            if new_position > n.content.len() {
                n.content.resize(new_position, 0);
            }
            
            // Write data
            n.content[*position as usize..new_position].copy_from_slice(buffer);
            n.update_size(n.content.len());
            
            let size_increase = n.content.len().saturating_sub(old_size);
            size_increase
        }) {
            Some(_) => {
                *position += buffer.len() as u64;
                fs.add_memory_usage(buffer.len());
                Ok(buffer.len())
            },
            None => {
                Err(FileSystemError {
                    kind: FileSystemErrorKind::NotFound,
                    message: "File not found".to_string(),
                })
            }
        }
    }
}

impl FileHandle for Xv6FileHandle {
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
                // For directories, read from cached directory content as Dirent entries
                if let Some(ref dir_content) = self.directory_content {
                    let content = dir_content.read();
                    let mut position = self.position.write();
                    
                    if *position as usize >= content.len() {
                        return Ok(0); // EOF
                    }
                    
                    let available = content.len() - *position as usize;
                    let to_read = buffer.len().min(available);
                    
                    buffer[..to_read].copy_from_slice(&content[*position as usize..*position as usize + to_read]);
                    *position += to_read as u64;
                    Ok(to_read)
                } else {
                    return Err(FileSystemError {
                        kind: FileSystemErrorKind::IsADirectory,
                        message: "Directory content not available".to_string(),
                    });
                }
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
        if !matches!(self.file_type, FileType::Directory) {
            return Err(FileSystemError {
                kind: FileSystemErrorKind::NotADirectory,
                message: "Not a directory".to_string(),
            });
        }

        if let Some(node) = self.fs().find_node(&self.path) {
            let mut entries = Vec::new();
            
            // Add "." entry
            entries.push(DirectoryEntry {
                name: ".".to_string(),
                file_type: FileType::Directory,
                size: 0,
                metadata: Some(node.metadata.clone()),
            });
            
            // Add ".." entry
            let parent_metadata = if self.path == "/" {
                node.metadata.clone()
            } else {
                self.fs().find_parent_and_name(&self.path)
                    .map(|(parent, _)| parent.metadata)
                    .unwrap_or(node.metadata.clone())
            };
            
            entries.push(DirectoryEntry {
                name: "..".to_string(),
                file_type: FileType::Directory,
                size: 0,
                metadata: Some(parent_metadata),
            });
            
            // Add child entries
            for (name, child) in node.children.entries() {
                entries.push(DirectoryEntry {
                    name: name.clone(),
                    file_type: child.file_type.clone(),
                    size: child.metadata.size,
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
        let mut position = self.position.write();
        
        match whence {
            SeekFrom::Start(offset) => {
                *position = offset;
                Ok(*position)
            }
            SeekFrom::Current(offset) => {
                let new_pos = (*position as i64) + offset;
                if new_pos < 0 {
                    return Err(FileSystemError {
                        kind: FileSystemErrorKind::InvalidData,
                        message: "Seek position cannot be negative".to_string(),
                    });
                }
                *position = new_pos as u64;
                Ok(*position)
            }
            SeekFrom::End(offset) => {
                let size = match &self.file_type {
                    FileType::RegularFile => {
                        if let Some(node) = self.fs().find_node(&self.path) {
                            node.content.len() as i64
                        } else {
                            0
                        }
                    }
                    FileType::Directory => {
                        if let Some(dir_content) = &self.directory_content {
                            dir_content.read().len() as i64
                        } else {
                            0
                        }
                    }
                    _ => 0,
                };
                
                let new_pos = size + offset;
                if new_pos < 0 {
                    return Err(FileSystemError {
                        kind: FileSystemErrorKind::InvalidData,
                        message: "Seek position cannot be negative".to_string(),
                    });
                }
                *position = new_pos as u64;
                Ok(*position)
            }
        }
    }

    fn release(&self) -> Result<()> {
        // For xv6fs, no special cleanup needed
        Ok(())
    }

    fn metadata(&self) -> Result<FileMetadata> {
        if let Some(node) = self.fs().find_node(&self.path) {
            Ok(node.metadata)
        } else {
            Err(FileSystemError {
                kind: FileSystemErrorKind::NotFound,
                message: "File not found".to_string(),
            })
        }
    }
}

impl FileOperations for Xv6FS {
    fn open(&self, path: &str, _flags: u32) -> Result<Arc<dyn FileHandle>> {
        let normalized = self.normalize_path(path);
        
        if let Some(node) = self.find_node(&normalized) {
            match node.file_type {
                FileType::RegularFile | FileType::Directory => {
                    Ok(Arc::new(Xv6FileHandle::new(
                        normalized,
                        node.file_type,
                        node.inode_number,
                        self,
                    )))
                }
                FileType::CharDevice(ref info) | FileType::BlockDevice(ref info) => {
                    match DeviceManager::get_manager().borrow_device(info.device_id) {
                        Ok(guard) => {
                            Ok(Arc::new(Xv6FileHandle::new_with_device(
                                normalized,
                                node.file_type,
                                node.inode_number,
                                guard,
                                self,
                            )))
                        }
                        Err(_) => Err(FileSystemError {
                            kind: FileSystemErrorKind::PermissionDenied,
                            message: "Failed to access device".to_string(),
                        })
                    }
                }
                _ => Err(FileSystemError {
                    kind: FileSystemErrorKind::NotSupported,
                    message: "Unsupported file type".to_string(),
                })
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
            if !matches!(node.file_type, FileType::Directory) {
                return Err(FileSystemError {
                    kind: FileSystemErrorKind::NotADirectory,
                    message: "Not a directory".to_string(),
                });
            }
            
            let mut entries = Vec::new();
            
            // Add "." entry
            entries.push(DirectoryEntry {
                name: ".".to_string(),
                file_type: FileType::Directory,
                size: 0,
                metadata: Some(node.metadata.clone()),
            });
            
            // Add ".." entry
            let parent_metadata = if normalized == "/" {
                node.metadata.clone()
            } else {
                self.find_parent_and_name(&normalized)
                    .map(|(parent, _)| parent.metadata)
                    .unwrap_or(node.metadata.clone())
            };
            
            entries.push(DirectoryEntry {
                name: "..".to_string(),
                file_type: FileType::Directory,
                size: 0,
                metadata: Some(parent_metadata),
            });
            
            // Add child entries
            for (name, child) in node.children.entries() {
                entries.push(DirectoryEntry {
                    name: name.clone(),
                    file_type: child.file_type.clone(),
                    size: child.metadata.size,
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
        let normalized = self.normalize_path(path);
        
        if let Some((parent, name)) = self.find_parent_and_name(&normalized) {
            if parent.children.contains_key(&name) {
                return Err(FileSystemError {
                    kind: FileSystemErrorKind::AlreadyExists,
                    message: "File already exists".to_string(),
                });
            }
            
            let new_node = match file_type {
                FileType::RegularFile => Xv6Node::new_file(name.clone()),
                FileType::Directory => Xv6Node::new_directory(name.clone()),
                _ => Xv6Node::new_device(name.clone(), file_type),
            };
            
            // Update the filesystem
            let mut root = self.root.write();
            let parent_path = if normalized == format!("/{}", name) {
                "/"
            } else {
                &normalized[..normalized.rfind('/').unwrap()]
            };
            
            if parent_path == "/" {
                root.children.insert(name, new_node);
            } else {
                // Navigate to parent and insert
                let parts: Vec<&str> = parent_path.trim_start_matches('/').split('/').collect();
                let mut current = &mut *root;
                
                for part in parts {
                    if let Some(child) = current.children.get_mut(part) {
                        current = child;
                    } else {
                        return Err(FileSystemError {
                            kind: FileSystemErrorKind::NotFound,
                            message: "Parent directory not found".to_string(),
                        });
                    }
                }
                
                current.children.insert(name, new_node);
            }
            
            Ok(())
        } else {
            Err(FileSystemError {
                kind: FileSystemErrorKind::NotFound,
                message: "Parent directory not found".to_string(),
            })
        }
    }

    fn create_dir(&self, path: &str) -> Result<()> {
        self.create_file(path, FileType::Directory)
    }

    fn remove(&self, path: &str) -> Result<()> {
        let normalized = self.normalize_path(path);
        
        if normalized == "/" {
            return Err(FileSystemError {
                kind: FileSystemErrorKind::PermissionDenied,
                message: "Cannot remove root directory".to_string(),
            });
        }
        
        if let Some((_parent, name)) = self.find_parent_and_name(&normalized) {
            let mut root = self.root.write();
            let parent_path = &normalized[..normalized.rfind('/').unwrap()];
            let parent_path = if parent_path.is_empty() { "/" } else { parent_path };
            
            if parent_path == "/" {
                if let Some(removed_node) = root.children.remove(&name) {
                    if matches!(removed_node.file_type, FileType::Directory) && !removed_node.children.is_empty() {
                        // Put it back and return error
                        root.children.insert(name, removed_node);
                        return Err(FileSystemError {
                            kind: FileSystemErrorKind::DirectoryNotEmpty,
                            message: "Directory not empty".to_string(),
                        });
                    }
                    self.subtract_memory_usage(removed_node.content.len());
                }
            } else {
                // Navigate to parent and remove
                let parts: Vec<&str> = parent_path.trim_start_matches('/').split('/').collect();
                let mut current = &mut *root;
                
                for part in parts {
                    if let Some(child) = current.children.get_mut(part) {
                        current = child;
                    } else {
                        return Err(FileSystemError {
                            kind: FileSystemErrorKind::NotFound,
                            message: "Parent directory not found".to_string(),
                        });
                    }
                }
                
                if let Some(removed_node) = current.children.remove(&name) {
                    if matches!(removed_node.file_type, FileType::Directory) && !removed_node.children.is_empty() {
                        // Put it back and return error
                        current.children.insert(name, removed_node);
                        return Err(FileSystemError {
                            kind: FileSystemErrorKind::DirectoryNotEmpty,
                            message: "Directory not empty".to_string(),
                        });
                    }
                    self.subtract_memory_usage(removed_node.content.len());
                }
            }
            
            Ok(())
        } else {
            Err(FileSystemError {
                kind: FileSystemErrorKind::NotFound,
                message: "File not found".to_string(),
            })
        }
    }

    fn metadata(&self, path: &str) -> Result<FileMetadata> {
        let normalized = self.normalize_path(path);
        
        if let Some(node) = self.find_node(&normalized) {
            Ok(node.metadata)
        } else {
            Err(FileSystemError {
                kind: FileSystemErrorKind::NotFound,
                message: "File not found".to_string(),
            })
        }
    }

    fn root_dir(&self) -> Result<Directory> {
        Ok(Directory::open("/".to_string()))
    }
}

/// Xv6FS driver for creating Xv6FS instances
pub struct Xv6FSDriver;

impl FileSystemDriver for Xv6FSDriver {
    fn name(&self) -> &'static str {
        "xv6fs"
    }

    fn filesystem_type(&self) -> FileSystemType {
        FileSystemType::Memory
    }

    fn create_from_block(
        &self,
        _block_device: Box<dyn BlockDevice>,
        _block_size: usize,
    ) -> Result<Box<dyn VirtualFileSystem>> {
        // Xv6FS is memory-based, ignoring block device
        Ok(Box::new(Xv6FS::new(0))) // 0 = unlimited memory
    }

    fn create_with_params(
        &self,
        params: &dyn crate::fs::params::FileSystemParams,
    ) -> Result<Box<dyn VirtualFileSystem>> {
        if let Some(xv6_params) = params.as_any().downcast_ref::<Xv6FSParams>() {
            Ok(Box::new(Xv6FS::new(xv6_params.memory_limit)))
        } else {
            Err(FileSystemError {
                kind: FileSystemErrorKind::InvalidData,
                message: "Invalid parameters for Xv6FS".to_string(),
            })
        }
    }
}

impl Xv6FSDriver {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Xv6FSParams {
    /// Maximum memory usage in bytes (0 = unlimited)
    /// 
    /// This limit applies to the total size of all files and directories
    /// stored in the Xv6FS instance. When the limit is reached, write
    /// operations will fail with ENOSPC (No space left on device).
    pub memory_limit: usize,
}

impl Default for Xv6FSParams {
    /// Create Xv6FS parameters with unlimited memory
    /// 
    /// The default configuration allows unlimited memory usage, which
    /// provides maximum flexibility but requires careful monitoring in
    /// production environments.
    fn default() -> Self {
        Self {
            memory_limit: 0, // Unlimited by default
        }
    }
}

impl Xv6FSParams {
    /// Create Xv6FS parameters with specified memory limit
    /// 
    /// # Arguments
    /// 
    /// * `memory_limit` - Maximum memory usage in bytes (0 for unlimited)
    /// 
    /// # Returns
    /// 
    /// Xv6FSParams instance with the specified memory limit
    /// 
    /// # Example
    /// 
    /// ```rust
    /// // Create Xv6FS with 10MB limit
    /// let params = Xv6FSParams::with_memory_limit(10 * 1024 * 1024);
    /// ```
    pub fn with_memory_limit(memory_limit: usize) -> Self {
        Self {
            memory_limit,
        }
    }
}

impl FileSystemParams for Xv6FSParams {
    fn to_string_map(&self) -> BTreeMap<String, String> {
        let mut map = BTreeMap::new();
        map.insert("memory_limit".to_string(), self.memory_limit.to_string());
        map
    }
    
    fn from_string_map(map: &BTreeMap<String, String>) -> core::result::Result<Xv6FSParams, String> {
        let memory_limit = if let Some(limit_str) = map.get("memory_limit") {
            limit_str.parse::<usize>()
                .map_err(|_| format!("Invalid memory_limit value: {}", limit_str))?
        } else {
            0 // Default to unlimited memory
        };

        Ok(Self { memory_limit })
    }
    
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Register Xv6FS driver with the filesystem driver manager
pub fn register_xv6fs_driver() {
    crate::fs::get_fs_driver_manager().register_driver(Box::new(Xv6FSDriver::new()));
}

// Auto-register the Xv6FS driver when this module is loaded
crate::driver_initcall!(register_xv6fs_driver);

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_dirent_creation() {
        let dirent = Dirent::new(1, "test.txt");
        assert_eq!(dirent.inum, 1);
        assert_eq!(dirent.name_str(), "test.txt");
        
        // Test long name truncation
        let long_dirent = Dirent::new(2, "very_long_filename.txt");
        assert_eq!(long_dirent.inum, 2);
        assert_eq!(long_dirent.name_str(), "very_long_fil"); // Truncated to 13 chars
    }

    #[test_case]
    fn test_xv6fs_creation() {
        let fs = Xv6FS::new(1024);
        assert_eq!(fs.name(), "xv6fs");
        assert!(!fs.mounted);
    }

    #[test_case]
    fn test_xv6fs_directory_operations() {
        let fs = Xv6FS::new(0);
        
        // Create directory
        assert!(fs.create_dir("/test_dir").is_ok());
        
        // Create file in directory
        assert!(fs.create_file("/test_dir/file.txt", FileType::RegularFile).is_ok());
        
        // Read directory
        let entries = fs.read_dir("/test_dir").unwrap();
        assert_eq!(entries.len(), 3); // ".", "..", "file.txt"
        
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"."));
        assert!(names.contains(&".."));
        assert!(names.contains(&"file.txt"));
    }

    #[test_case]
    fn test_xv6fs_directory_file_read() {
        let fs = Xv6FS::new(0);
        
        // Create some files
        assert!(fs.create_dir("/test_dir").is_ok());
        assert!(fs.create_file("/test_dir/file1.txt", FileType::RegularFile).is_ok());
        assert!(fs.create_file("/test_dir/file2.txt", FileType::RegularFile).is_ok());
        
        // Open directory as file
        let dir_handle = fs.open("/test_dir", 0).unwrap();
        
        // Read directory content as bytes (Dirent entries)
        let mut buffer = [0u8; 1024];
        let bytes_read = dir_handle.read(&mut buffer).unwrap();
        
        assert!(bytes_read > 0);
        assert_eq!(bytes_read % Dirent::DIRENT_SIZE, 0); // Should be multiple of Dirent size
        
        let num_entries = bytes_read / Dirent::DIRENT_SIZE;
        assert_eq!(num_entries, 4); // ".", "..", "file1.txt", "file2.txt"
        
        // Parse first Dirent entry (should be ".")
        let first_dirent = unsafe {
            &*(buffer.as_ptr() as *const Dirent)
        };
        assert_eq!(first_dirent.name_str(), ".");
        assert_eq!(first_dirent.inum, 2); // Directory inode number
    }

    #[test_case]
    fn test_xv6fs_file_operations() {
        let fs = Xv6FS::new(0);
        
        // Create and write to file
        assert!(fs.create_file("/test.txt", FileType::RegularFile).is_ok());
        
        let file_handle = fs.open("/test.txt", 0).unwrap();
        let test_data = b"Hello, xv6fs!";
        assert_eq!(file_handle.write(test_data).unwrap(), test_data.len());
        
        // Read back
        assert_eq!(file_handle.seek(SeekFrom::Start(0)).unwrap(), 0);
        let mut read_buffer = [0u8; 32];
        let bytes_read = file_handle.read(&mut read_buffer).unwrap();
        assert_eq!(bytes_read, test_data.len());
        assert_eq!(&read_buffer[..bytes_read], test_data);
    }

    #[test_case]
    fn test_xv6fs_seek_operations() {
        let fs = Xv6FS::new(0);
        
        // Create directory with some entries
        assert!(fs.create_dir("/seek_test").is_ok());
        assert!(fs.create_file("/seek_test/file1", FileType::RegularFile).is_ok());
        assert!(fs.create_file("/seek_test/file2", FileType::RegularFile).is_ok());
        
        let dir_handle = fs.open("/seek_test", 0).unwrap();
        
        // Read first entry
        let mut buffer = [0u8; Dirent::DIRENT_SIZE];
        assert_eq!(dir_handle.read(&mut buffer).unwrap(), Dirent::DIRENT_SIZE);
        
        // Seek to second entry
        assert_eq!(dir_handle.seek(SeekFrom::Start(Dirent::DIRENT_SIZE as u64)).unwrap(), Dirent::DIRENT_SIZE as u64);
        
        // Read second entry
        assert_eq!(dir_handle.read(&mut buffer).unwrap(), Dirent::DIRENT_SIZE);
        let second_dirent = unsafe { &*(buffer.as_ptr() as *const Dirent) };
        assert_eq!(second_dirent.name_str(), "..");
        
        // Seek to end
        assert!(dir_handle.seek(SeekFrom::End(0)).is_ok());
        
        // Try to read (should return 0)
        assert_eq!(dir_handle.read(&mut buffer).unwrap(), 0);
    }

    #[test_case]
    fn test_xv6fs_readdir_vs_read() {
        let fs = Xv6FS::new(0);
        
        assert!(fs.create_dir("/compare_test").is_ok());
        assert!(fs.create_file("/compare_test/testfile", FileType::RegularFile).is_ok());
        
        let dir_handle = fs.open("/compare_test", 0).unwrap();
        
        // Test readdir() method
        let dir_entries = dir_handle.readdir().unwrap();
        assert_eq!(dir_entries.len(), 3); // ".", "..", "testfile"
        
        // Test read() method (should return serialized Dirent entries)
        let mut buffer = [0u8; 1024];
        let bytes_read = dir_handle.read(&mut buffer).unwrap();
        let num_dirents = bytes_read / Dirent::DIRENT_SIZE;
        assert_eq!(num_dirents, 3); // Same number of entries
        
        // Verify that the number of entries matches
        assert_eq!(dir_entries.len(), num_dirents);
    }
}

