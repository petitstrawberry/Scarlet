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

use crate::abi::xv6::riscv64::file;
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
#[derive(Default)]
struct Xv6DirectoryEntries {
    entries: BTreeMap<String, Arc<Xv6Node>>,
}

impl Xv6DirectoryEntries {
    fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }

    fn insert(&mut self, name: String, node: Arc<Xv6Node>) -> Option<Arc<Xv6Node>> {
        self.entries.insert(name, node)
    }

    fn remove(&mut self, name: &str) -> Option<Arc<Xv6Node>> {
        self.entries.remove(name)
    }

    fn get(&self, name: &str) -> Option<&Arc<Xv6Node>> {
        self.entries.get(name)
    }

    fn contains_key(&self, name: &str) -> bool {
        self.entries.contains_key(name)
    }

    fn entries(&self) -> impl Iterator<Item = (&String, &Arc<Xv6Node>)> {
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
struct Xv6Node {
    name: RwLock<String>,
    file_type: RwLock<FileType>,
    content: RwLock<Vec<u8>>,
    metadata: RwLock<FileMetadata>,
    children: RwLock<Xv6DirectoryEntries>,
}

impl Xv6Node {
    fn new_file(name: String, file_id: u32) -> Arc<Self> {
        Arc::new(Self {
            name: RwLock::new(name.clone()),
            file_type: RwLock::new(FileType::RegularFile),
            content: RwLock::new(Vec::new()),
            metadata: RwLock::new(FileMetadata {
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
                file_id: file_id as u64,
                link_count: 1,
            }),
            children: RwLock::new(Xv6DirectoryEntries::new()),
        })
    }

    fn new_directory(name: String, file_id: u32) -> Arc<Self> {
        Arc::new(Self {
            name: RwLock::new(name.clone()),
            file_type: RwLock::new(FileType::Directory),
            content: RwLock::new(Vec::new()),
            metadata: RwLock::new(FileMetadata {
                file_type: FileType::Directory,
                size: 1024, // Initial size for directory entries
                permissions: FilePermission {
                    read: true,
                    write: true,
                    execute: true,
                },
                created_time: crate::time::current_time(),
                modified_time: crate::time::current_time(),
                accessed_time: crate::time::current_time(),
                file_id: file_id as u64,
                link_count: 1,
            }),
            children: RwLock::new(Xv6DirectoryEntries::new()),
        })
    }

    fn new_device(name: String, file_type: FileType, file_id: u32) -> Arc<Self> {
        Arc::new(Self {
            name: RwLock::new(name.clone()),
            file_type: RwLock::new(file_type.clone()),
            content: RwLock::new(Vec::new()),
            metadata: RwLock::new(FileMetadata {
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
                file_id: file_id as u64,
                link_count: 1,
            }),
            children: RwLock::new(Xv6DirectoryEntries::new()),
        })
    }

    /// Generate directory content as serialized Dirent entries
    fn generate_directory_content(&self, parent_inum: u32) -> Vec<u8> {
        let mut content = Vec::new();
        
        // Add "." entry (current directory)
        let current_dirent = Dirent::new(self.metadata.read().file_id as u16, ".");
        content.extend_from_slice(current_dirent.as_bytes());
        
        // Add ".." entry (parent directory)
        let parent_dirent = Dirent::new(parent_inum as u16, "..");
        content.extend_from_slice(parent_dirent.as_bytes());
        
        // Add all child entries
        let children = self.children.read();
        for (name, child) in children.entries() {
            let dirent = Dirent::new(child.metadata.read().file_id as u16, name);
            content.extend_from_slice(dirent.as_bytes());
        }

        // Sort content by dirent entries (assuming fixed size)
        const DIRENT_SIZE: usize = core::mem::size_of::<Dirent>();
        let mut entries: Vec<_> = content.chunks_exact(DIRENT_SIZE).collect();
        entries.sort_by_key(|chunk| {
            // Safety: We know each chunk is exactly DIRENT_SIZE bytes
            let dirent = unsafe { &*(chunk.as_ptr() as *const Dirent) };
            dirent.inum
        });
        
        // Rebuild content from sorted entries into a new vector
        let mut sorted_content = Vec::with_capacity(content.len());
        for entry in entries {
            sorted_content.extend_from_slice(entry);
        }
        
        sorted_content
        }

    /// Update file size and modification time
    fn update_size(&self, new_size: usize) {
        let mut metadata = self.metadata.write();
        metadata.size = new_size;
        metadata.modified_time = crate::time::current_time();
    }
}

/// Xv6FS - xv6-compatible filesystem based on tmpfs
pub struct Xv6FS {
    mounted: bool,
    mount_point: String,
    root: Arc<Xv6Node>,
    max_memory: usize,
    current_memory: Mutex<usize>,
    next_inode_number: Mutex<u32>, // For generating unique inode numbers
}

impl Xv6FS {
    pub fn new(max_memory: usize) -> Self {
        let root = Xv6Node::new_directory("/".to_string(), 1);
        *root.metadata.write() = FileMetadata {
            file_type: FileType::Directory,
            size: 1024,
            permissions: FilePermission {
                read: true,
                write: true,
                execute: true,
            },
            created_time: crate::time::current_time(),
            modified_time: crate::time::current_time(),
            accessed_time: crate::time::current_time(),
            file_id: 1, // Root always has file_id 1
            link_count: 1,
        };
        
        Self {
            mounted: false,
            mount_point: String::new(),
            root,
            max_memory,
            current_memory: Mutex::new(0),
            next_inode_number: Mutex::new(2), // Start from 2 (0 is invalid, 1 is root)
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

    fn generate_inode_number(&self) -> u32 {
        let mut next = self.next_inode_number.lock();
        let inode_number = *next;
        *next += 1;
        inode_number
    }

    fn find_node(&self, path: &str) -> Option<Arc<Xv6Node>> {
        let normalized = self.normalize_path(path);
        
        if normalized == "/" {
            return Some(Arc::clone(&self.root));
        }

        let parts: Vec<&str> = normalized.trim_start_matches('/').split('/').collect();
        let mut current = Arc::clone(&self.root);

        for part in parts {
            let child = {
                let children = current.children.read();
                children.get(part).cloned()
            };
            if let Some(child_node) = child {
                current = child_node;
            } else {
                return None;
            }
        }

        Some(current)
    }

    /// Find a mutable reference to a node by path
    fn find_node_mut<F, R>(&self, path: &str, f: F) -> Option<R>
    where
        F: FnOnce(&Arc<Xv6Node>) -> R,
    {
        let normalized = self.normalize_path(path);
        
        if normalized == "/" {
            return Some(f(&self.root));
        }

        let parts: Vec<&str> = normalized.trim_start_matches('/').split('/').collect();
        let mut current = Arc::clone(&self.root);

        for part in parts {
            let child = {
                let children = current.children.read();
                children.get(part).cloned()
            };
            if let Some(child_node) = child {
                current = child_node;
            } else {
                return None;
            }
        }

        Some(f(&current))
    }

    fn find_parent_and_name(&self, path: &str) -> Option<(Arc<Xv6Node>, String)> {
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
    node: Arc<Xv6Node>,
    position: RwLock<u64>,
    device_guard: Option<BorrowedDeviceGuard>,
    fs: *const Xv6FS,
    /// Cached directory content for directory files
    directory_content: Option<RwLock<Vec<u8>>>,
}

unsafe impl Send for Xv6FileHandle {}
unsafe impl Sync for Xv6FileHandle {}

impl Xv6FileHandle {
    fn new(node: Arc<Xv6Node>, fs: &Xv6FS) -> Self {
        let directory_content = {
            let file_type = node.file_type.read();
            if matches!(*file_type, FileType::Directory) {
                // Generate directory content when handle is created
                let parent_inum = if *node.name.read() == "/" { 
                    1 
                } else {
                    // For non-root directories, we need to find the parent inode
                    1 // Simplified for now, could be improved
                };
                let content = node.generate_directory_content(parent_inum);
                Some(RwLock::new(content))
            } else {
                None
            }
        };

        Self {
            node,
            position: RwLock::new(0),
            device_guard: None,
            fs: fs as *const Xv6FS,
            directory_content,
        }
    }

    fn new_with_device(
        node: Arc<Xv6Node>,
        device_guard: BorrowedDeviceGuard,
        fs: &Xv6FS,
    ) -> Self {
        Self {
            node,
            position: RwLock::new(0),
            device_guard: Some(device_guard),
            fs: fs as *const Xv6FS,
            directory_content: None,
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
        let mut position = self.position.write();
        
        let content = self.node.content.read();
        if *position as usize >= content.len() {
            return Ok(0); // EOF
        }
        
        let available = content.len() - *position as usize;
        let to_read = buffer.len().min(available);
        
        buffer[..to_read].copy_from_slice(&content[*position as usize..*position as usize + to_read]);
        *position += to_read as u64;
        Ok(to_read)
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
        
        let mut content = self.node.content.write();
        let old_size = content.len();
        let new_position = *position as usize + buffer.len();
        
        // Expand file if necessary
        if new_position > content.len() {
            content.resize(new_position, 0);
        }
        
        // Write data
        content[*position as usize..new_position].copy_from_slice(buffer);
        let new_size = content.len();
        drop(content); // Release the lock before updating size
        
        self.node.update_size(new_size);
        
        let size_increase = new_size.saturating_sub(old_size);
        *position += buffer.len() as u64;
        fs.add_memory_usage(size_increase);
        Ok(buffer.len())
    }
}

impl FileHandle for Xv6FileHandle {
    fn read(&self, buffer: &mut [u8]) -> Result<usize> {
        let file_type = self.node.file_type.read();
        match &*file_type {
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
        let file_type = self.node.file_type.read();
        if !matches!(*file_type, FileType::Directory) {
            return Err(FileSystemError {
                kind: FileSystemErrorKind::NotADirectory,
                message: "Not a directory".to_string(),
            });
        }

        let mut entries = Vec::new();
        
        // Add "." entry
        let metadata = self.node.metadata.read();
        entries.push(DirectoryEntry {
            name: ".".to_string(),
            file_type: FileType::Directory,
            size: 0,
            metadata: Some(metadata.clone()),
            file_id: metadata.file_id,
        });
        
        // Add ".." entry - simplified parent metadata
        entries.push(DirectoryEntry {
            name: "..".to_string(),
            file_type: FileType::Directory,
            size: 0,
            metadata: Some(metadata.clone()),
            file_id: metadata.file_id,
        });
        drop(metadata);
        
        // Add child entries
        let children = self.node.children.read();
        for (name, child) in children.entries() {
            let child_metadata = child.metadata.read();
            let child_file_type = child.file_type.read();
            entries.push(DirectoryEntry {
                name: name.clone(),
                file_type: child_file_type.clone(),
                size: child_metadata.size,
                metadata: Some(child_metadata.clone()),
                file_id: child_metadata.file_id,
            });
        }
        
        Ok(entries)
    }

    fn write(&self, buffer: &[u8]) -> Result<usize> {
        let file_type = self.node.file_type.read();
        match &*file_type {
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
                let file_type = self.node.file_type.read();
                let size = match &*file_type {
                    FileType::RegularFile => {
                        let content = self.node.content.read();
                        content.len() as i64
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
        let metadata = self.node.metadata.read();
        Ok(metadata.clone())
    }
}

impl FileOperations for Xv6FS {
    fn open(&self, path: &str, _flags: u32) -> Result<Arc<dyn FileHandle>> {
        let normalized = self.normalize_path(path);
        
        if let Some(node) = self.find_node(&normalized) {
            let file_type = node.file_type.read();
            match &*file_type {
                FileType::RegularFile | FileType::Directory => {
                    drop(file_type);
                    Ok(Arc::new(Xv6FileHandle::new(
                        Arc::clone(&node),
                        self,
                    )))
                }
                FileType::CharDevice(info) | FileType::BlockDevice(info) => {
                    let device_id = info.device_id;
                    drop(file_type);
                    match DeviceManager::get_manager().borrow_device(device_id) {
                        Ok(guard) => {
                            Ok(Arc::new(Xv6FileHandle::new_with_device(
                                Arc::clone(&node),
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
            let file_type = node.file_type.read();
            if !matches!(*file_type, FileType::Directory) {
                return Err(FileSystemError {
                    kind: FileSystemErrorKind::NotADirectory,
                    message: "Not a directory".to_string(),
                });
            }
            
            let mut entries = Vec::new();
            
            // Add "." entry
            let metadata = node.metadata.read();
            entries.push(DirectoryEntry {
                name: ".".to_string(),
                file_type: FileType::Directory,
                size: 0,
                metadata: Some(metadata.clone()),
                file_id: metadata.file_id,
            });
            
            // Add ".." entry - simplified parent metadata
            entries.push(DirectoryEntry {
                name: "..".to_string(),
                file_type: FileType::Directory,
                size: 0,
                metadata: Some(metadata.clone()),
                file_id: metadata.file_id,
            });
            drop(metadata);
            
            // Add child entries
            let children = node.children.read();
            for (name, child) in children.entries() {
                let child_metadata = child.metadata.read();
                let child_file_type = child.file_type.read();
                entries.push(DirectoryEntry {
                    name: name.clone(),
                    file_type: child_file_type.clone(),
                    size: child_metadata.size,
                    metadata: Some(child_metadata.clone()),
                    file_id: child_metadata.file_id,
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
            let children = parent.children.read();
            if children.contains_key(&name) {
                return Err(FileSystemError {
                    kind: FileSystemErrorKind::AlreadyExists,
                    message: "File already exists".to_string(),
                });
            }
            drop(children);

            let file_id = self.generate_inode_number();
            
            let new_node = match file_type {
                FileType::RegularFile => Xv6Node::new_file(name.clone(), file_id),
                FileType::Directory => Xv6Node::new_directory(name.clone(), file_id),
                _ => Xv6Node::new_device(name.clone(), file_type, file_id),
            };
            
            // Update the filesystem using the parent directly
            let mut parent_children = parent.children.write();
            parent_children.insert(name, new_node);
            
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

    fn create_hardlink(&self, target_path: &str, link_path: &str) -> Result<()> {
        let normalized_target = self.normalize_path(target_path);
        let normalized_link = self.normalize_path(link_path);
        
        if normalized_target == normalized_link {
            return Err(FileSystemError {
                kind: FileSystemErrorKind::InvalidData,
                message: "Cannot create hard link to itself".to_string(),
            });
        }
        
        if let Some(target_node) = self.find_node(&normalized_target) {
            if let Some((parent, name)) = self.find_parent_and_name(&normalized_link) {
                let children = parent.children.read();
                if children.contains_key(&name) {
                    return Err(FileSystemError {
                        kind: FileSystemErrorKind::AlreadyExists,
                        message: "Link already exists".to_string(),
                    });
                }
                drop(children);
                
                // Create the hard link
                let new_node = Arc::clone(&target_node);
                new_node.metadata.write().link_count += 1; // Increment link count
                
                let mut parent_children = parent.children.write();
                parent_children.insert(name, new_node);
                
                Ok(())
            } else {
                Err(FileSystemError {
                    kind: FileSystemErrorKind::NotFound,
                    message: "Parent directory not found".to_string(),
                })
            }
        } else {
            Err(FileSystemError {
                kind: FileSystemErrorKind::NotFound,
                message: "Target file not found".to_string(),
            })
        }
    }

    fn remove(&self, path: &str) -> Result<()> {
        let normalized = self.normalize_path(path);
        
        if normalized == "/" {
            return Err(FileSystemError {
                kind: FileSystemErrorKind::PermissionDenied,
                message: "Cannot remove root directory".to_string(),
            });
        }
        
        if let Some((parent, name)) = self.find_parent_and_name(&normalized) {
            let mut parent_children = parent.children.write();
            if let Some(removed_node) = parent_children.remove(&name) {
                let file_type = removed_node.file_type.read();
                let children = removed_node.children.read();
                let is_dir_not_empty = matches!(*file_type, FileType::Directory) && !children.is_empty();
                drop(file_type);
                drop(children);
                
                if is_dir_not_empty {
                    // Put it back and return error
                    parent_children.insert(name, removed_node);
                    return Err(FileSystemError {
                        kind: FileSystemErrorKind::DirectoryNotEmpty,
                        message: "Directory not empty".to_string(),
                    });
                }
                let content_len = removed_node.content.read().len();
                self.subtract_memory_usage(content_len);
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
            let metadata = node.metadata.read();
            Ok(metadata.clone())
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

