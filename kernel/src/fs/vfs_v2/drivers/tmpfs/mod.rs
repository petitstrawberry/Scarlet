//! TmpFS v2 - Memory-based filesystem implementation
//!
//! This is a complete rewrite of TmpFS using the new VFS v2 architecture.
//! It implements FileSystemOperations directly and uses VfsNode for internal
//! structure representation.

use alloc::{
    boxed::Box, collections::BTreeMap, format, string::{String, ToString}, sync::{Arc, Weak}, vec, vec::Vec
};
use spin::{rwlock::RwLock, Mutex};
use core::{any::Any, fmt::Debug};

use crate::{device::{Device, DeviceType}, driver_initcall, fs::{
    get_fs_driver_manager, DeviceFileInfo, FileMetadata, FileObject, FilePermission, FileSystemDriver, FileSystemError, FileSystemErrorKind, FileType
}};
use crate::object::capability::{StreamOps, StreamError};
use crate::device::manager::DeviceManager;

use super::super::core::{VfsNode, FileSystemOperations, DirectoryEntryInternal};

/// TmpFS v2 - New memory-based filesystem implementation
///
/// This struct implements an in-memory filesystem for VFS v2.
/// It supports regular files, directories, and device nodes, with optional memory usage limits.
/// The internal structure is based on `TmpNode` and uses locking for thread safety.
///
pub struct TmpFS {
    /// Root directory node
    root: RwLock<Arc<TmpNode>>,
    /// Memory limit (0 = unlimited)
    memory_limit: usize,
    /// Current memory usage
    current_memory: Mutex<usize>,
    /// Next file ID generator
    next_file_id: Mutex<u64>,
    /// Filesystem name
    name: String,
}

impl TmpFS {
    /// Create a new TmpFS instance (two-phase initialization)
    pub fn new(memory_limit: usize) -> Arc<Self> {
        let root = Arc::new(TmpNode::new_directory("/".to_string(), 1));
        let fs = Arc::new(Self {
            root: RwLock::new(Arc::clone(&root)),
            memory_limit,
            current_memory: Mutex::new(0),
            next_file_id: Mutex::new(2), // Start from 2, root is 1
            name: "tmpfs_v2".to_string(),
        });
        let fs_weak = Arc::downgrade(&(fs.clone() as Arc<dyn FileSystemOperations>));
        root.set_filesystem(fs_weak);
        debug_assert!(root.filesystem().is_some(), "TmpFS root node's filesystem() is None after set_filesystem");
        fs
    }

    /// VFS v2 driver registration API: create from option string
    /// Example option: "mem=1048576" etc.
    pub fn create_from_option_string(option: Option<&str>) -> Arc<dyn FileSystemOperations> {
        let mut memory_limit = 0;
        if let Some(opt) = option {
            for part in opt.split(',') {
                let part = part.trim();
                if let Some(mem_str) = part.strip_prefix("mem=") {
                    if let Ok(val) = mem_str.parse::<usize>() {
                        memory_limit = val;
                    }
                }
            }
        }
        TmpFS::new(memory_limit) as Arc<dyn FileSystemOperations>
    }
    
    /// Generate next unique file ID
    fn generate_file_id(&self) -> u64 {
        let mut next_id = self.next_file_id.lock();
        let id = *next_id;
        *next_id += 1;
        id
    }
    
    /// Check memory limit
    fn check_memory_limit(&self, additional_bytes: usize) -> Result<(), FileSystemError> {
        if self.memory_limit == 0 {
            return Ok(()); // Unlimited
        }
        
        let current = *self.current_memory.lock();
        if current + additional_bytes > self.memory_limit {
            return Err(FileSystemError::new(
                FileSystemErrorKind::NoSpace,
                "TmpFS memory limit exceeded"
            ));
        }
        
        Ok(())
    }
    
    /// Add to memory usage
    fn add_memory_usage(&self, bytes: usize) {
        if self.memory_limit > 0 {
            *self.current_memory.lock() += bytes;
        }
    }
    
    /// Subtract from memory usage
    fn subtract_memory_usage(&self, bytes: usize) {
        if self.memory_limit > 0 {
            let mut current = self.current_memory.lock();
            *current = current.saturating_sub(bytes);
        }
    }
}

impl FileSystemOperations for TmpFS {
    fn lookup(
        &self,
        parent_node: &Arc<dyn VfsNode>,
        name: &String,
    ) -> Result<Arc<dyn VfsNode>, FileSystemError> {
        // Downcast to TmpNode
        let tmp_node = parent_node.as_any()
            .downcast_ref::<TmpNode>()
            .ok_or_else(|| FileSystemError::new(
                FileSystemErrorKind::NotSupported,
                "Invalid node type for TmpFS"
            ))?;
            
        // Check if parent is a directory
        if tmp_node.file_type() != FileType::Directory {
            return Err(FileSystemError::new(
                FileSystemErrorKind::NotADirectory,
                "Parent is not a directory"
            ));
        }

        // Handle special directory entries
        match name.as_str() {
            "." => {
                // Current directory - return self
                return Ok(Arc::clone(&parent_node));
            }
            ".." => {
                // Parent directory - try to handle within filesystem
                if let Some(parent_weak) = &tmp_node.parent() {
                    if let Some(parent) = parent_weak.upgrade() {
                        // crate::println!("TmpFS lookup: found parent node {:?}", parent);
                        // Return parent node within this filesystem
                        return Ok(parent as Arc<dyn VfsNode>);
                    }
                }
            }
            _ => {
                // Regular lookup
            }
        }
        
        // Look up child in directory
        let children = tmp_node.children.read();
        if let Some(child_node) = children.get(name) {
            Ok(Arc::clone(child_node) as Arc<dyn VfsNode>)
        } else {
            Err(FileSystemError::new(
                FileSystemErrorKind::NotFound,
                "File not found"
            ))
        }
    }
    
    fn open(
        &self,
        node: &Arc<dyn VfsNode>,
        _flags: u32,
    ) -> Result<Arc<dyn FileObject>, FileSystemError> {
        let tmp_node = Arc::downcast::<TmpNode>(node.clone())
            .map_err(|_| FileSystemError::new(
                FileSystemErrorKind::NotSupported,
                "Invalid node type for TmpFS"
            ))?;

        let file_object = match tmp_node.file_type() {
            FileType::RegularFile => TmpFileObject::new_regular(tmp_node),
            FileType::Directory => TmpFileObject::new_directory(tmp_node),
            FileType::CharDevice(info) | FileType::BlockDevice(info) => {
                TmpFileObject::new_device(tmp_node, info)
            }
            _ => {
                return Err(FileSystemError::new(
                    FileSystemErrorKind::NotSupported,
                    "Unsupported file type for open"
                ));
            }
        };

        Ok(Arc::new(file_object))
    }
    
    fn create(&self,
        parent_node: &Arc<dyn VfsNode>,
        name: &String,
        file_type: FileType,
        _mode: u32,
    ) -> Result<Arc<dyn VfsNode>, FileSystemError> {
        let tmp_parent = Arc::downcast::<TmpNode>(parent_node.clone())
            .map_err(|_| FileSystemError::new(
                FileSystemErrorKind::NotSupported,
                "Invalid node type for TmpFS"
            ))?;
            
        // Check if parent is a directory
        if tmp_parent.file_type() != FileType::Directory {
            return Err(FileSystemError::new(
                FileSystemErrorKind::NotADirectory,
                "Parent is not a directory"
            ));
        }
        // Check if file already exists
        {
            let children = tmp_parent.children.read();
            if children.contains_key(name) {
                return Err(FileSystemError::new(
                    FileSystemErrorKind::AlreadyExists,
                    "File already exists"
                ));
            }
        }
        // Generate file ID
        let file_id = self.generate_file_id();
        let new_node = match file_type {
            FileType::RegularFile => {
                Arc::new(TmpNode::new_file(name.clone().to_string(), file_id))
            }
            FileType::Directory => {
                Arc::new(TmpNode::new_directory(name.clone().to_string(), file_id))
            }
            FileType::SymbolicLink(target_path) => {
                // Account for memory usage (target path length)
                self.add_memory_usage(target_path.len());
                Arc::new(TmpNode::new_symlink(name.clone().to_string(), target_path, file_id))
            }
            FileType::CharDevice(_) | FileType::BlockDevice(_) => {
                Arc::new(TmpNode::new_device(name.clone().to_string(), file_type, file_id))
            }
            _ => {
                return Err(FileSystemError::new(
                    FileSystemErrorKind::NotSupported,
                    "Unsupported file type for creation"
                ));
            }
        };
        // After creation, set the filesystem reference (always check if upgrade is possible)
        let fs_ref = parent_node.filesystem()
            .ok_or_else(|| FileSystemError::new(
                FileSystemErrorKind::NotSupported,
                "Parent node does not have a filesystem reference"
            ))?;
        if fs_ref.upgrade().is_none() {
            return Err(FileSystemError::new(
                FileSystemErrorKind::NotSupported,
                "Parent node's filesystem reference is dead (cannot upgrade)"
            ));
        }
        if let Some(tmp_node) = new_node.as_any().downcast_ref::<TmpNode>() {
            tmp_node.set_filesystem(fs_ref);
        }
        // Add to parent directory
        {
            let mut children = tmp_parent.children.write();
            new_node.set_parent(Arc::downgrade(&tmp_parent));
            children.insert(name.clone(), Arc::clone(&new_node) as Arc<dyn VfsNode>);
        }

        Ok(new_node)
    }

    /// Create a hard link to an existing file
    ///
    /// This function creates a new directory entry (hard link) for an existing file.
    /// Both the link and the target must be in the same filesystem.
    /// The link count of the target file is incremented.
    /// Returns the target node (the same inode as the original file).
    fn create_hardlink(
        &self,
        link_parent: &Arc<dyn VfsNode>,
        link_name: &String,
        target_node: &Arc<dyn VfsNode>,
    ) -> Result<Arc<dyn VfsNode>, FileSystemError> {
        // Check that both parent and target are TmpNodes
        let tmp_parent = link_parent.as_any()
            .downcast_ref::<TmpNode>()
            .ok_or_else(|| FileSystemError::new(
                FileSystemErrorKind::NotSupported,
                "Invalid parent node type for TmpFS"
            ))?;
            
        let tmp_target = target_node.as_any()
            .downcast_ref::<TmpNode>()
            .ok_or_else(|| FileSystemError::new(
                FileSystemErrorKind::NotSupported,
                "Invalid target node type for TmpFS"
            ))?;
        
        // Check that parent is a directory
        if tmp_parent.file_type() != FileType::Directory {
            return Err(FileSystemError::new(
                FileSystemErrorKind::NotADirectory,
                "Parent is not a directory"
            ));
        }
        
        // Check that target is a regular file (no directory hard links)
        if tmp_target.file_type() != FileType::RegularFile {
            return Err(FileSystemError::new(
                FileSystemErrorKind::InvalidOperation,
                "Cannot create hard link to non-regular file"
            ));
        }
        
        // Check if link name already exists
        {
            let children = tmp_parent.children.read();
            if children.contains_key(link_name) {
                return Err(FileSystemError::new(
                    FileSystemErrorKind::FileExists,
                    "Link name already exists"
                ));
            }
        }
        
        // For TmpFS, hard links are just additional references to the same TmpNode
        // Update the link count in metadata
        {
            let mut metadata = tmp_target.metadata.write();
            metadata.link_count += 1;
        }
        
        // Add the target node to the parent directory under the new name
        {
            let mut children = tmp_parent.children.write();
            children.insert(link_name.clone(), Arc::clone(target_node));
        }
        
        // Return the same target node (hard link shares the same inode)
        Ok(Arc::clone(target_node))
    }

    
    fn remove(
        &self,
        parent_node: &Arc<dyn VfsNode>,
        name: &String,
    ) -> Result<(), FileSystemError> {
        let tmp_parent = parent_node.as_any()
            .downcast_ref::<TmpNode>()
            .ok_or_else(|| FileSystemError::new(
                FileSystemErrorKind::NotSupported,
                "Invalid node type for TmpFS"
            ))?;
            
        // Check if parent is a directory
        if tmp_parent.file_type() != FileType::Directory {
            return Err(FileSystemError::new(
                FileSystemErrorKind::NotADirectory,
                "Parent is not a directory"
            ));
        }
        
        // Remove from parent directory
        let mut children = tmp_parent.children.write();
        if let Some(removed_node) = children.get(name) {
            // If it's a directory, check if it's empty first
            if let Some(tmp_node) = removed_node.as_any().downcast_ref::<TmpNode>() {
                if tmp_node.file_type() == FileType::Directory {
                    let child_children = tmp_node.children.read();
                    if !child_children.is_empty() {
                        return Err(FileSystemError::new(
                            FileSystemErrorKind::DirectoryNotEmpty,
                            "Directory not empty"
                        ));
                    }
                }
                
                // Update memory usage for regular files and symbolic links
                match tmp_node.file_type() {
                    FileType::RegularFile | FileType::SymbolicLink(_) => {
                        let content = tmp_node.content.read();
                        self.subtract_memory_usage(content.len());
                    },
                    _ => {}
                }
            }
        }
        
        // Now remove the node
        children.remove(name)
            .ok_or_else(|| FileSystemError::new(
                FileSystemErrorKind::NotFound,
                "File not found"
            ))?;
        
        Ok(())
    }
    
    fn readdir(
        &self,
        node: &Arc<dyn VfsNode>,
    ) -> Result<Vec<DirectoryEntryInternal>, FileSystemError> {
        let tmp_node = node.as_any()
            .downcast_ref::<TmpNode>()
            .ok_or_else(|| FileSystemError::new(
                FileSystemErrorKind::NotSupported,
                "Invalid node type for TmpFS"
            ))?;
            
        // Check if it's a directory
        if tmp_node.file_type() != FileType::Directory {
            return Err(FileSystemError::new(
                FileSystemErrorKind::NotADirectory,
                "Not a directory"
            ));
        }
        
        let mut entries = Vec::new();
        let children = tmp_node.children.read();
        
        for (name, child_node) in children.iter() {
            if let Some(child_tmp_node) = child_node.as_any().downcast_ref::<TmpNode>() {
                let metadata = child_tmp_node.metadata.read();
                entries.push(DirectoryEntryInternal {
                    name: name.clone(),
                    file_type: child_tmp_node.file_type.read().clone(),
                    file_id: metadata.file_id,
                });
            }
        }
        
        Ok(entries)
    }
    
    fn root_node(&self) -> Arc<dyn VfsNode> {
        Arc::clone(&*self.root.read()) as Arc<dyn VfsNode>
    }
    
    fn name(&self) -> &str {
        &self.name
    }
}

/// TmpNode represents a file, directory, or device node in TmpFS.
///
/// Each node contains metadata, content (for files), children (for directories),
/// and references to its parent and filesystem. All fields are protected by locks for thread safety.
pub struct TmpNode {
    /// File name
    name: RwLock<String>,
    /// File type
    file_type: RwLock<FileType>,
    /// File metadata
    metadata: RwLock<FileMetadata>,
    /// File content (for regular files)
    content: RwLock<Vec<u8>>,
    /// Child nodes (for directories)
    children: RwLock<BTreeMap<String, Arc<dyn VfsNode>>>,
    /// Parent node (weak reference to avoid cycles)
    parent: RwLock<Option<Weak<TmpNode>>>,
    /// Reference to filesystem (Weak<dyn FileSystemOperations>)
    filesystem: RwLock<Option<Weak<dyn FileSystemOperations>>>,
}

impl Debug for TmpNode {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("TmpNode")
            .field("name", &self.name.read())
            .field("file_type", &self.file_type.read())
            .field("metadata", &self.metadata.read())
            .field("parent", &self.parent.read().as_ref().map(|p| p.strong_count()))
            .finish()
    }
}

impl TmpNode {
    /// Create a new regular file node
    pub fn new_file(name: String, file_id: u64) -> Self {
        Self {
            name: RwLock::new(name),
            file_type: RwLock::new(FileType::RegularFile),
            metadata: RwLock::new(FileMetadata {
                file_type: FileType::RegularFile,
                size: 0,
                permissions: FilePermission {
                    read: true,
                    write: true,
                    execute: false,
                },
                created_time: 0, // TODO: actual timestamp
                modified_time: 0,
                accessed_time: 0,
                file_id,
                link_count: 1,
            }),
            content: RwLock::new(Vec::new()),
            children: RwLock::new(BTreeMap::new()),
            parent: RwLock::new(None), // No parent initially
            filesystem: RwLock::new(None),
        }
    }
    
    /// Create a new directory node
    pub fn new_directory(name: String, file_id: u64) -> Self {
        Self {
            name: RwLock::new(name),
            file_type: RwLock::new(FileType::Directory),
            metadata: RwLock::new(FileMetadata {
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
                file_id,
                link_count: 1,
            }),
            content: RwLock::new(Vec::new()),
            children: RwLock::new(BTreeMap::new()),
            parent: RwLock::new(None), // No parent initially
            filesystem: RwLock::new(None),
        }
    }
    
    /// Create a new device file node
    pub fn new_device(name: String, file_type: FileType, file_id: u64) -> Self {
        Self {
            name: RwLock::new(name),
            file_type: RwLock::new(file_type.clone()),
            metadata: RwLock::new(FileMetadata {
                file_type,
                size: 0,
                permissions: FilePermission {
                    read: true,
                    write: true,
                    execute: false,
                },
                created_time: 0,
                modified_time: 0,
                accessed_time: 0,
                file_id,
                link_count: 1,
            }),
            content: RwLock::new(Vec::new()),
            children: RwLock::new(BTreeMap::new()),
            parent: RwLock::new(None), // No parent initially
            filesystem: RwLock::new(None),
        }
    }
    
    /// Create a new symbolic link node
    pub fn new_symlink(name: String, target: String, file_id: u64) -> Self {
        Self {
            name: RwLock::new(name),
            file_type: RwLock::new(FileType::SymbolicLink(target.clone())),
            metadata: RwLock::new(FileMetadata {
                file_type: FileType::SymbolicLink(target.clone()),
                size: target.len(),
                permissions: FilePermission {
                    read: true,
                    write: true,
                    execute: false,
                },
                created_time: 0, // TODO: actual timestamp
                modified_time: 0,
                accessed_time: 0,
                file_id,
                link_count: 1,
            }),
            // Store symlink target in content as UTF-8 bytes
            content: RwLock::new(target.into_bytes()),
            children: RwLock::new(BTreeMap::new()),
            parent: RwLock::new(None), // No parent initially
            filesystem: RwLock::new(None),
        }
    }
    
    /// Set the filesystem reference for this node
    pub fn set_filesystem(&self, fs: Weak<dyn FileSystemOperations>) {
        *self.filesystem.write() = Some(fs);
    }
    
    /// Update file size in metadata
    pub fn update_size(&self, new_size: u64) {
        let mut metadata = self.metadata.write();
        metadata.size = new_size as usize;
        metadata.modified_time = 0; // TODO: actual timestamp
    }
    
    /// Set parent reference for this node
    pub fn set_parent(&self, parent: Weak<TmpNode>) {
        self.parent.write().replace(parent);
    }
    
    /// Check if this node is the root of the filesystem
    pub fn is_filesystem_root(&self) -> bool {
        self.parent.read().is_none()
    }

    /// Get the file name
    pub fn name(&self) -> String {
        self.name.read().clone()
    }

    /// Get the file type
    pub fn file_type(&self) -> FileType {
        self.file_type.read().clone()
    }

    /// Get the filesystem reference
    pub fn filesystem(&self) -> Option<Weak<dyn FileSystemOperations>> {
        self.filesystem.read().clone()
    }

    /// Get the parent node
    pub fn parent(&self) -> Option<Weak<TmpNode>> {
        self.parent.read().clone()
    }
}

impl VfsNode for TmpNode {
    fn id(&self) -> u64 {
        self.metadata.read().file_id
    }
    
    fn filesystem(&self) -> Option<Weak<dyn FileSystemOperations>> {
        self.filesystem.read().clone()
    }
    
    fn metadata(&self) -> Result<FileMetadata, FileSystemError> {
        Ok(self.metadata.read().clone())
    }
    
    fn as_any(&self) -> &dyn Any {
        self
    }
    
    fn read_link(&self) -> Result<String, FileSystemError> {
        // Check if this is actually a symbolic link and return target
        match &self.file_type() {
            FileType::SymbolicLink(target) => Ok(target.clone()),
            _ => Err(FileSystemError::new(
                FileSystemErrorKind::NotSupported,
                "Not a symbolic link"
            ))
        }
    }
}

/// File object for TmpFS operations
///
/// TmpFileObject represents an open file or directory in TmpFS.
///
/// It maintains the current file position and, for device files, an optional device guard.
pub struct TmpFileObject {
    /// Reference to the TmpNode
    node: Arc<TmpNode>,
    
    /// Current file position
    position: RwLock<u64>,
    
    /// Optional device guard for device files
    device_guard: Option<Arc<dyn Device>>,
}

impl TmpFileObject {
    /// Create a new file object for regular files
    pub fn new_regular(node: Arc<TmpNode>) -> Self {
        Self {
            node,
            position: RwLock::new(0),
            device_guard: None,
        }
    }
    
    /// Create a new file object for directories
    pub fn new_directory(node: Arc<TmpNode>) -> Self {
        Self {
            node,
            position: RwLock::new(0),
            device_guard: None,
        }
    }
    
    /// Create a new file object for device files
    pub fn new_device(node: Arc<TmpNode>, info: DeviceFileInfo) -> Self {
        // Try to borrow the device from DeviceManager
        match DeviceManager::get_manager().get_device(info.device_id) {
            Some(device_guard) => {
                Self {
                    node,
                    position: RwLock::new(0),
                    device_guard: Some(device_guard),
                }
            },
            None => {
                // If borrowing fails, return an error
                panic!("Failed to borrow device {}", info.device_id);
            }
        }
    }
                        
    fn read_device(&self, buffer: &mut [u8]) -> Result<usize, FileSystemError> {
        if let Some(ref device_guard) = self.device_guard {
            let device_guard_ref = device_guard.as_ref();
            
            match device_guard_ref.device_type() {
                DeviceType::Char => {
                    if let Some(char_device) = device_guard_ref.as_char_device() {
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
                    if let Some(block_device) = device_guard_ref.as_block_device() {
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

    fn read_regular_file(&self, buffer: &mut [u8]) -> Result<usize, FileSystemError> {
        let mut position = self.position.write();
        
        // Use the direct node reference instead of finding it by path
        
        let content_guard = self.node.content.write();
        // self.node.update_access_time();

        if *position as usize >= content_guard.len() {
            return Ok(0); // EOF
        }
        
        let available = content_guard.len() - *position as usize;
        let to_read = buffer.len().min(available);
        
        buffer[..to_read].copy_from_slice(&content_guard[*position as usize..*position as usize + to_read]);
        *position += to_read as u64;
        
        Ok(to_read)
    }

    fn write_device(&self, buffer: &[u8]) -> Result<usize, FileSystemError> {
        if let Some(ref device_guard) = self.device_guard {
            let device_guard_ref = device_guard.as_ref();
            
            match device_guard_ref.device_type() {
                DeviceType::Char => {
                    if let Some(char_device) = device_guard_ref.as_char_device() {
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
                    if let Some(block_device) = device_guard_ref.as_block_device() {
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

    fn write_regular_file(&self, buffer: &[u8]) -> Result<usize, FileSystemError> {
        let mut position = self.position.write();
        
        // Use the direct node reference instead of finding it by path
        let mut content_guard = self.node.content.write();
        let _old_size = content_guard.len();
        let new_position = *position as usize + buffer.len();
        
        // Expand file if necessary
        if new_position > content_guard.len() {
            content_guard.resize(new_position, 0);
        }
        
        // Write data
        content_guard[*position as usize..new_position].copy_from_slice(buffer);
        let new_size = content_guard.len();
        
        // Update metadata
        self.node.update_size(new_size as u64);
        
        // let size_increase = new_size.saturating_sub(old_size);
        *position += buffer.len() as u64;
        Ok(buffer.len())
    }
}

impl StreamOps for TmpFileObject {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        match self.node.file_type() {
            FileType::RegularFile => {
                self.read_regular_file(buffer)
                    .map_err(StreamError::from)
            }
            FileType::Directory => {
                // For directories, return entries in struct format
                let node = self.node.clone();
                // We need to reconstruct the path from the node structure
                // Since we don't have path stored, use the readdir logic directly
                
                // Create a vector to store all entries including "." and ".."
                // Add "." entry (current directory)
                let current_metadata = node.metadata.read();
                let mut all_entries = vec![crate::fs::DirectoryEntryInternal {
                    name: ".".to_string(),
                    file_type: FileType::Directory,
                    size: current_metadata.size,
                    file_id: current_metadata.file_id,
                    metadata: Some(current_metadata.clone()),
                }];
                
                // Add ".." entry (parent directory) - simplified to point to self for now
                all_entries.push(crate::fs::DirectoryEntryInternal {
                    name: "..".to_string(),
                    file_type: FileType::Directory,
                    size: current_metadata.size,
                    file_id: current_metadata.file_id,
                    metadata: Some(current_metadata.clone()),
                });
                
                // Add regular directory entries and sort by file_id
                let children = node.children.read();
                let mut regular_entries = Vec::new();
                for (name, child) in children.iter() {
                    let metadata = child.metadata().unwrap();
                    regular_entries.push(crate::fs::DirectoryEntryInternal {
                        name: name.clone(),
                        file_type: child.file_type().unwrap().clone(),
                        size: metadata.size,
                        file_id: metadata.file_id,
                        metadata: Some(metadata.clone()),
                    });
                }
                
                // Sort regular entries by file_id (ascending order)
                regular_entries.sort_by_key(|entry| entry.file_id);
                
                // Append sorted regular entries to the result
                all_entries.extend(regular_entries);
                
                // position is the entry index
                let position = *self.position.read() as usize;
                
                if position >= all_entries.len() {
                    return Ok(0); // EOF
                }
                
                // Get current entry (already sorted)
                let internal_entry = &all_entries[position];
                
                // Convert to binary format
                let dir_entry = crate::fs::DirectoryEntry::from_internal(internal_entry);
                
                // Calculate actual entry size
                let entry_size = dir_entry.entry_size();
                
                // Check buffer size
                if buffer.len() < entry_size {
                    return Err(StreamError::InvalidArgument); // Buffer too small
                }
                
                // Treat struct as byte array
                let entry_bytes = unsafe {
                    core::slice::from_raw_parts(
                        &dir_entry as *const _ as *const u8,
                        entry_size
                    )
                };
                
                // Copy to buffer
                buffer[..entry_size].copy_from_slice(entry_bytes);
                
                // Move to next entry
                *self.position.write() += 1;
                
                Ok(entry_size)
            },
            FileType::CharDevice(_) | FileType::BlockDevice(_) => {
                self.read_device(buffer)
                    .map_err(StreamError::from)
            }
            _ => Err(StreamError::NotSupported)
        }
    }
    
    fn write(&self, buffer: &[u8]) -> Result<usize, StreamError> {
        match self.node.file_type() {
            FileType::RegularFile => {
                self.write_regular_file(buffer).map_err(StreamError::from)
            }
            FileType::Directory => {
                Err(StreamError::from(FileSystemError::new(
                    FileSystemErrorKind::IsADirectory,
                    "Cannot write to directory"
                )))
            }
            FileType::CharDevice(_) | FileType::BlockDevice(_) => {
                self.write_device(buffer).map_err(StreamError::from)
            }
            _ => Err(StreamError::NotSupported)
        }
    }
}

impl FileObject for TmpFileObject {
    fn seek(&self, pos: crate::fs::SeekFrom) -> Result<u64, StreamError> {
        use crate::fs::SeekFrom;
        
        let mut position = self.position.write();
        let content = self.node.content.read();
        let file_size = content.len() as u64;
        
        let new_pos = match pos {
            SeekFrom::Start(offset) => {
                if offset <= file_size {
                    offset
                } else {
                    return Err(StreamError::from(FileSystemError::new(
                        FileSystemErrorKind::NotSupported,
                        "Seek offset beyond EOF"
                    )));
                }
            }
            SeekFrom::End(offset) => {
                if offset >= 0 {
                    file_size + offset as u64
                } else {
                    file_size.saturating_sub((-offset) as u64)
                }
            }
            SeekFrom::Current(offset) => {
                if offset >= 0 {
                    *position + offset as u64
                } else {
                    position.saturating_sub((-offset) as u64)
                }
            }
        };
        
        *position = new_pos;
        Ok(new_pos)
    }
    
    fn metadata(&self) -> Result<FileMetadata, StreamError> {
        self.node.metadata().map_err(StreamError::from)
    }
    
    fn truncate(&self, size: u64) -> Result<(), StreamError> {
        if self.node.file_type() != FileType::RegularFile {
            return Err(StreamError::from(FileSystemError::new(
                FileSystemErrorKind::IsADirectory,
                "Cannot truncate non-regular file"
            )));
        }
        
        let mut content = self.node.content.write();
        let old_size = content.len();
        let new_size = size as usize;
        
        if new_size > old_size {
            // Expand with zeros
            content.resize(new_size, 0);
        } else if new_size < old_size {
            // Truncate
            content.truncate(new_size);
        }
        
        // Update metadata
        self.node.update_size(size);
        
        Ok(())
    }
}

pub struct TmpFSDriver;

impl FileSystemDriver for TmpFSDriver {
    
    fn filesystem_type(&self) -> crate::fs::FileSystemType {
        crate::fs::FileSystemType::Virtual
    }
    
    fn create_from_memory(&self, _memory_area: &crate::vm::vmem::MemoryArea) -> Result<Arc<dyn FileSystemOperations>, FileSystemError> {
        Ok(TmpFS::new(0) as Arc<dyn FileSystemOperations>)
    }
    
    fn create_from_params(&self, _params: &dyn crate::fs::params::FileSystemParams) -> Result<Arc<dyn FileSystemOperations>, FileSystemError> {
        Ok(TmpFS::create_from_option_string(None))
    }

    fn create_from_option_string(&self, options: &str) -> Result<Arc<dyn FileSystemOperations>, FileSystemError> {
        // Parse tmpfs options (e.g., "size=64M")
        let memory_limit = parse_tmpfs_size_option(options).unwrap_or(64 * 1024 * 1024); // Default 64MB
        Ok(TmpFS::new(memory_limit))
    }
    
    fn name(&self) -> &'static str {
        "tmpfs"
    }
}

/// Parse tmpfs size option from option string
/// 
/// Parses size option in the format "size=64M", "size=1G", etc.
/// Returns the size in bytes, or None if no valid size option is found.
fn parse_tmpfs_size_option(options: &str) -> Option<usize> {
    for option in options.split(',') {
        if let Some(size_str) = option.strip_prefix("size=") {
            // Parse size with suffix (K, M, G)
            let size_str = size_str.trim();
            if size_str.is_empty() {
                continue;
            }
            
            let (number_part, multiplier) = if size_str.ends_with('K') || size_str.ends_with('k') {
                (&size_str[..size_str.len()-1], 1024)
            } else if size_str.ends_with('M') || size_str.ends_with('m') {
                (&size_str[..size_str.len()-1], 1024 * 1024)
            } else if size_str.ends_with('G') || size_str.ends_with('g') {
                (&size_str[..size_str.len()-1], 1024 * 1024 * 1024)
            } else {
                (size_str, 1)
            };
            
            if let Ok(number) = number_part.parse::<usize>() {
                return Some(number * multiplier);
            }
        }
    }
    None
}

fn register_driver() {
    let fs_driver_manager = get_fs_driver_manager();
    fs_driver_manager.register_driver(Box::new(TmpFSDriver));
}

driver_initcall!(register_driver);

#[cfg(test)]
mod tests;