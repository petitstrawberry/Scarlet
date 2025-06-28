//! TmpFS v2 - Memory-based filesystem implementation
//!
//! This is a complete rewrite of TmpFS using the new VFS v2 architecture.
//! It implements FileSystemOperations directly and uses VfsNode for internal
//! structure representation.

use alloc::{
    collections::BTreeMap, format, string::{String, ToString}, sync::{Arc, Weak}, vec::Vec
};
use spin::{rwlock::RwLock, Mutex};
use core::{any::Any, fmt::Debug};

use crate::fs::{
    FileSystemError, FileSystemErrorKind, FileMetadata, FilePermission, 
    FileType, FileObject
};
use crate::object::capability::{StreamOps, StreamError};
use crate::device::manager::BorrowedDeviceGuard;

use super::core::{VfsNode, FileSystemOperations, DirectoryEntryInternal};

/// TmpFS v2 - New memory-based filesystem implementation
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
    /// Create a new TmpFS instance (2段階初期化)
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
        parent_node: Arc<dyn VfsNode>,
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
        for (_child_name, _child_node) in children.iter() {
            // crate::println!("TmpFS lookup: checking child '{}'", child_name);
        }
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
        node: Arc<dyn VfsNode>,
        _flags: u32,
    ) -> Result<Arc<dyn FileObject>, FileSystemError> {
        let tmp_node = Arc::downcast::<TmpNode>(node)
            .map_err(|_| FileSystemError::new(
                FileSystemErrorKind::NotSupported,
                "Invalid node type for TmpFS"
            ))?;

        let file_object = TmpFileObject::new_regular(tmp_node);
        Ok(Arc::new(file_object))
    }
    
    fn create(&self,
        parent_node: Arc<dyn VfsNode>,
        name: &String,
        file_type: FileType,
        mode: u32,
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
        // 生成後にfs参照をセット（upgrade可能か必ず確認）
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
    
    fn remove(
        &self,
        parent_node: Arc<dyn VfsNode>,
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
                
                // Update memory usage for regular files
                if tmp_node.file_type() == FileType::RegularFile {
                    let content = tmp_node.content.read();
                    self.subtract_memory_usage(content.len());
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
        node: Arc<dyn VfsNode>,
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

/// TmpNode represents a file or directory in TmpFS
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
            file_type: RwLock::new(file_type),
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
        Err(FileSystemError::new(
            FileSystemErrorKind::NotSupported,
            "Not a symbolic link"
        ))
    }
}

/// File object for TmpFS operations
pub struct TmpFileObject {
    /// Reference to the TmpNode
    node: Arc<TmpNode>,
    
    /// Current file position
    position: RwLock<u64>,
    
    /// Optional device guard for device files
    device_guard: Option<BorrowedDeviceGuard>,
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
    pub fn new_device(node: Arc<TmpNode>, device_guard: BorrowedDeviceGuard) -> Self {
        Self {
            node,
            position: RwLock::new(0),
            device_guard: Some(device_guard),
        }
    }
}

impl StreamOps for TmpFileObject {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        match self.node.file_type() {
            FileType::RegularFile => {
                let mut position = self.position.write();
                let content = self.node.content.read();
                
                if *position as usize >= content.len() {
                    return Ok(0); // EOF
                }
                
                let available = content.len() - *position as usize;
                let to_read = buffer.len().min(available);
                
                buffer[..to_read].copy_from_slice(
                    &content[*position as usize..*position as usize + to_read]
                );
                *position += to_read as u64;
                
                Ok(to_read)
            }
            FileType::Directory => {
                // TODO: Implement directory reading
                // For now, return empty
                Ok(0)
            }
            FileType::CharDevice(_) | FileType::BlockDevice(_) => {
                // Delegate to device
                if let Some(ref device_guard) = self.device_guard {
                    // TODO: Implement device reading
                    Ok(0)
                } else {
                    Err(StreamError::NotSupported)
                }
            }
            _ => Err(StreamError::NotSupported)
        }
    }
    
    fn write(&self, buffer: &[u8]) -> Result<usize, StreamError> {
        match self.node.file_type() {
            FileType::RegularFile => {
                let mut position = self.position.write();
                let mut content = self.node.content.write();
                
                // Expand file if necessary
                if *position as usize + buffer.len() > content.len() {
                    content.resize(*position as usize + buffer.len(), 0);
                }
                
                // Write data
                content[*position as usize..*position as usize + buffer.len()]
                    .copy_from_slice(buffer);
                *position += buffer.len() as u64;
                
                // Update metadata
                self.node.update_size(content.len() as u64);
                
                Ok(buffer.len())
            }
            FileType::Directory => {
                Err(StreamError::from(FileSystemError::new(
                    FileSystemErrorKind::IsADirectory,
                    "Cannot write to directory"
                )))
            }
            FileType::CharDevice(_) | FileType::BlockDevice(_) => {
                // Delegate to device
                if let Some(ref device_guard) = self.device_guard {
                    // TODO: Implement device writing
                    Ok(buffer.len())
                } else {
                    Err(StreamError::NotSupported)
                }
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
