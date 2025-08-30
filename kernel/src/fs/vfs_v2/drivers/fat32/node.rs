//! FAT32 VFS Node Implementation
//!
//! This module implements the VfsNode trait for FAT32 filesystem nodes.
//! It provides the interface between the VFS layer and FAT32-specific node data.

use alloc::{
    collections::BTreeMap, string::String, sync::{Arc, Weak}, vec::Vec
};
use spin::rwlock::RwLock;
use core::{any::Any, fmt::Debug};

use crate::fs::{
    FileMetadata, FileObject, FilePermission, FileSystemError, FileType, SeekFrom
};
use crate::object::capability::{StreamOps, StreamError, ControlOps, MemoryMappingOps};

use crate::fs::vfs_v2::core::{VfsNode, FileSystemOperations};

/// FAT32 filesystem node
/// 
/// This structure represents a file or directory in the FAT32 filesystem.
/// It implements the VfsNode trait to integrate with the VFS v2 architecture.
pub struct Fat32Node {
    /// Node name
    pub name: RwLock<String>,
    /// File type (file or directory)
    pub file_type: RwLock<FileType>,
    /// File metadata
    pub metadata: RwLock<FileMetadata>,
    /// File content (for regular files)
    pub content: RwLock<Vec<u8>>,
    /// Child nodes (for directories)
    pub children: RwLock<BTreeMap<String, Arc<dyn VfsNode>>>,
    /// Parent node (weak reference to avoid cycles)
    pub parent: RwLock<Option<Weak<Fat32Node>>>,
    /// Reference to filesystem
    pub filesystem: RwLock<Option<Weak<dyn FileSystemOperations>>>,
    /// Starting cluster number in FAT32
    pub cluster: RwLock<u32>,
}

impl Debug for Fat32Node {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Fat32Node")
            .field("name", &self.name.read())
            .field("file_type", &self.file_type.read())
            .field("metadata", &self.metadata.read())
            .field("cluster", &self.cluster.read())
            .field("parent", &self.parent.read().as_ref().map(|p| p.strong_count()))
            .finish()
    }
}

impl Fat32Node {
    /// Create a new regular file node
    pub fn new_file(name: String, file_id: u64, cluster: u32) -> Self {
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
                created_time: 0, // TODO: Convert FAT32 timestamps
                modified_time: 0,
                accessed_time: 0,
                file_id,
                link_count: 1,
            }),
            content: RwLock::new(Vec::new()),
            children: RwLock::new(BTreeMap::new()),
            parent: RwLock::new(None),
            filesystem: RwLock::new(None),
            cluster: RwLock::new(cluster),
        }
    }
    
    /// Create a new directory node
    pub fn new_directory(name: String, file_id: u64, cluster: u32) -> Self {
        Self {
            name: RwLock::new(name),
            file_type: RwLock::new(FileType::Directory),
            metadata: RwLock::new(FileMetadata {
                file_type: FileType::Directory,
                size: 0,
                permissions: FilePermission {
                    read: true,
                    write: true,
                    execute: true, // Directories need execute permission for traversal
                },
                created_time: 0, // TODO: Convert FAT32 timestamps
                modified_time: 0,
                accessed_time: 0,
                file_id,
                link_count: 1,
            }),
            content: RwLock::new(Vec::new()),
            children: RwLock::new(BTreeMap::new()),
            parent: RwLock::new(None),
            filesystem: RwLock::new(None),
            cluster: RwLock::new(cluster),
        }
    }
    
    /// Set the parent node (weak reference)
    pub fn set_parent(&self, parent: Option<Weak<Fat32Node>>) {
        *self.parent.write() = parent;
    }
    
    /// Set the filesystem reference
    pub fn set_filesystem(&self, filesystem: Weak<dyn FileSystemOperations>) {
        *self.filesystem.write() = Some(filesystem);
    }
    
    /// Get the starting cluster number
    pub fn cluster(&self) -> u32 {
        *self.cluster.read()
    }
    
    /// Set the starting cluster number
    pub fn set_cluster(&self, cluster: u32) {
        *self.cluster.write() = cluster;
    }
}

impl VfsNode for Fat32Node {
    fn id(&self) -> u64 {
        self.metadata.read().file_id
    }
    
    fn filesystem(&self) -> Option<Weak<dyn FileSystemOperations>> {
        self.filesystem.read().clone()
    }
    
    fn file_type(&self) -> Result<FileType, FileSystemError> {
        Ok(self.file_type.read().clone())
    }
    
    fn metadata(&self) -> Result<FileMetadata, FileSystemError> {
        Ok(self.metadata.read().clone())
    }
    
    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl Clone for Fat32Node {
    fn clone(&self) -> Self {
        Self {
            name: RwLock::new(self.name.read().clone()),
            file_type: RwLock::new(self.file_type.read().clone()),
            metadata: RwLock::new(self.metadata.read().clone()),
            content: RwLock::new(self.content.read().clone()),
            children: RwLock::new(self.children.read().clone()),
            parent: RwLock::new(self.parent.read().clone()),
            filesystem: RwLock::new(self.filesystem.read().clone()),
            cluster: RwLock::new(*self.cluster.read()),
        }
    }
}

/// FAT32 file object for regular files
pub struct Fat32FileObject {
    /// Reference to the FAT32 node
    node: Arc<Fat32Node>,
    /// Current file position
    position: RwLock<usize>,
}

impl Fat32FileObject {
    pub fn new(node: Arc<Fat32Node>) -> Self {
        Self {
            node,
            position: RwLock::new(0),
        }
    }
}

impl Debug for Fat32FileObject {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Fat32FileObject")
            .field("node", &self.node.name.read())
            .field("position", &self.position.read())
            .finish()
    }
}

impl StreamOps for Fat32FileObject {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        let content = self.node.content.read();
        let mut pos = self.position.write();
        
        if *pos >= content.len() {
            return Ok(0); // EOF
        }
        
        let remaining = content.len() - *pos;
        let to_read = core::cmp::min(buffer.len(), remaining);
        
        buffer[..to_read].copy_from_slice(&content[*pos..*pos + to_read]);
        *pos += to_read;
        
        Ok(to_read)
    }
    
    fn write(&self, buffer: &[u8]) -> Result<usize, StreamError> {
        let mut content = self.node.content.write();
        let mut pos = self.position.write();
        
        // Extend content if necessary
        if *pos + buffer.len() > content.len() {
            content.resize(*pos + buffer.len(), 0);
        }
        
        // Write data
        content[*pos..*pos + buffer.len()].copy_from_slice(buffer);
        *pos += buffer.len();
        
        // Update file size in metadata
        {
            let mut metadata = self.node.metadata.write();
            metadata.size = content.len();
        }
        
        Ok(buffer.len())
    }
}

impl ControlOps for Fat32FileObject {
    fn control(&self, _command: u32, _arg: usize) -> Result<i32, &'static str> {
        Err("Control operations not supported for FAT32 files")
    }
}

impl MemoryMappingOps for Fat32FileObject {
    fn get_mapping_info(&self, _offset: usize, _length: usize) -> Result<(usize, usize, bool), &'static str> {
        Err("Memory mapping not supported for FAT32 files")
    }
    
    fn on_mapped(&self, _vaddr: usize, _paddr: usize, _length: usize, _offset: usize) {
        // Not supported
    }
    
    fn on_unmapped(&self, _vaddr: usize, _length: usize) {
        // Not supported
    }
    
    fn supports_mmap(&self) -> bool {
        false
    }
}

impl FileObject for Fat32FileObject {
    fn seek(&self, from: SeekFrom) -> Result<u64, StreamError> {
        let content = self.node.content.read();
        let mut pos = self.position.write();
        
        let new_pos = match from {
            SeekFrom::Start(offset) => offset as usize,
            SeekFrom::End(offset) => {
                if offset < 0 {
                    content.len().saturating_sub((-offset) as usize)
                } else {
                    content.len() + offset as usize
                }
            },
            SeekFrom::Current(offset) => {
                if offset < 0 {
                    pos.saturating_sub((-offset) as usize)
                } else {
                    *pos + offset as usize
                }
            },
        };
        
        *pos = new_pos;
        Ok(new_pos as u64)
    }
    
    fn metadata(&self) -> Result<crate::fs::FileMetadata, StreamError> {
        Ok(self.node.metadata.read().clone())
    }
    
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// FAT32 directory object
pub struct Fat32DirectoryObject {
    /// Reference to the FAT32 node
    node: Arc<Fat32Node>,
    /// Current position in directory listing
    position: RwLock<usize>,
}

impl Fat32DirectoryObject {
    pub fn new(node: Arc<Fat32Node>) -> Self {
        Self {
            node,
            position: RwLock::new(0),
        }
    }
}

impl Debug for Fat32DirectoryObject {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Fat32DirectoryObject")
            .field("node", &self.node.name.read())
            .field("position", &self.position.read())
            .finish()
    }
}

impl StreamOps for Fat32DirectoryObject {
    fn read(&self, _buffer: &mut [u8]) -> Result<usize, StreamError> {
        Err(StreamError::NotSupported)
    }
    
    fn write(&self, _buffer: &[u8]) -> Result<usize, StreamError> {
        Err(StreamError::NotSupported)
    }
}

impl ControlOps for Fat32DirectoryObject {
    fn control(&self, _command: u32, _arg: usize) -> Result<i32, &'static str> {
        Err("Control operations not supported for FAT32 directories")
    }
}

impl MemoryMappingOps for Fat32DirectoryObject {
    fn get_mapping_info(&self, _offset: usize, _length: usize) -> Result<(usize, usize, bool), &'static str> {
        Err("Memory mapping not supported for FAT32 directories")
    }
    
    fn on_mapped(&self, _vaddr: usize, _paddr: usize, _length: usize, _offset: usize) {
        // Not supported
    }
    
    fn on_unmapped(&self, _vaddr: usize, _length: usize) {
        // Not supported
    }
    
    fn supports_mmap(&self) -> bool {
        false
    }
}

impl FileObject for Fat32DirectoryObject {
    fn seek(&self, from: SeekFrom) -> Result<u64, StreamError> {
        let children = self.node.children.read();
        let mut pos = self.position.write();
        
        let new_pos = match from {
            SeekFrom::Start(offset) => offset as usize,
            SeekFrom::End(offset) => {
                if offset < 0 {
                    children.len().saturating_sub((-offset) as usize)
                } else {
                    children.len() + offset as usize
                }
            },
            SeekFrom::Current(offset) => {
                if offset < 0 {
                    pos.saturating_sub((-offset) as usize)
                } else {
                    *pos + offset as usize
                }
            },
        };
        
        *pos = new_pos;
        Ok(new_pos as u64)
    }
    
    fn metadata(&self) -> Result<crate::fs::FileMetadata, StreamError> {
        Ok(self.node.metadata.read().clone())
    }
    
    fn as_any(&self) -> &dyn Any {
        self
    }
}