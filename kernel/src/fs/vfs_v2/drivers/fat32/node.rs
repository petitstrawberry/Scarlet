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
/// Content is read/written directly from/to the block device, not stored in memory.
pub struct Fat32Node {
    /// Node name
    pub name: RwLock<String>,
    /// File type (file or directory)
    pub file_type: RwLock<FileType>,
    /// File metadata
    pub metadata: RwLock<FileMetadata>,
    /// Child nodes (for directories) - cached, but loaded from disk on demand
    pub children: RwLock<BTreeMap<String, Arc<dyn VfsNode>>>,
    /// Parent node (weak reference to avoid cycles)
    pub parent: RwLock<Option<Weak<Fat32Node>>>,
    /// Reference to filesystem
    pub filesystem: RwLock<Option<Weak<dyn FileSystemOperations>>>,
    /// Starting cluster number in FAT32
    pub cluster: RwLock<u32>,
    /// Directory entries loaded flag (for directories)
    pub children_loaded: RwLock<bool>,
}

impl Debug for Fat32Node {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Fat32Node")
            .field("name", &self.name.read())
            .field("file_type", &self.file_type.read())
            .field("metadata", &self.metadata.read())
            .field("cluster", &self.cluster.read())
            .field("children_loaded", &self.children_loaded.read())
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
            children: RwLock::new(BTreeMap::new()),
            parent: RwLock::new(None),
            filesystem: RwLock::new(None),
            cluster: RwLock::new(cluster),
            children_loaded: RwLock::new(false),
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
            children: RwLock::new(BTreeMap::new()),
            parent: RwLock::new(None),
            filesystem: RwLock::new(None),
            cluster: RwLock::new(cluster),
            children_loaded: RwLock::new(false),
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
            children: RwLock::new(self.children.read().clone()),
            parent: RwLock::new(self.parent.read().clone()),
            filesystem: RwLock::new(self.filesystem.read().clone()),
            cluster: RwLock::new(*self.cluster.read()),
            children_loaded: RwLock::new(*self.children_loaded.read()),
        }
    }
}

/// FAT32 file object for regular files
pub struct Fat32FileObject {
    /// Reference to the FAT32 node
    node: Arc<Fat32Node>,
    /// Current file position
    position: RwLock<usize>,
    /// Cached file content in memory (lazily loaded)
    cached_content: RwLock<Option<Vec<u8>>>,
    /// Whether the cached content has been modified and needs to be written back
    is_dirty: RwLock<bool>,
}

impl Fat32FileObject {
    pub fn new(node: Arc<Fat32Node>) -> Self {
        Self {
            node,
            position: RwLock::new(0),
            cached_content: RwLock::new(None),
            is_dirty: RwLock::new(false),
        }
    }
    
    /// Load file content from disk into cache if not already loaded
    fn ensure_content_loaded(&self) -> Result<(), StreamError> {
        let mut cached = self.cached_content.write();
        
        // If already loaded, nothing to do
        if cached.is_some() {
            return Ok(());
        }
        
        // Get filesystem reference
        let fs = self.node.filesystem.read()
            .as_ref()
            .and_then(|weak| weak.upgrade())
            .ok_or(StreamError::Closed)?;
        
        // Downcast to Fat32FileSystem
        let fat32_fs = fs.as_any()
            .downcast_ref::<crate::fs::vfs_v2::drivers::fat32::Fat32FileSystem>()
            .ok_or(StreamError::NotSupported)?;
        
        let file_size = self.node.metadata.read().size;
        let cluster = self.node.cluster();
        
        // Read entire file content from disk
        let content = if file_size > 0 && cluster != 0 {
            fat32_fs.read_file_content(cluster, file_size)
                .map_err(|_| StreamError::IoError)?
        } else {
            Vec::new()
        };
        
        *cached = Some(content);
        Ok(())
    }
    
    /// Write cached content back to disk if dirty
    fn sync_to_disk(&self) -> Result<(), StreamError> {
        let is_dirty = *self.is_dirty.read();
        if !is_dirty {
            return Ok(()); // Nothing to sync
        }
        
        let cached = self.cached_content.read();
        let content = cached.as_ref().ok_or(StreamError::IoError)?;
        
        // Get filesystem reference
        let fs = self.node.filesystem.read()
            .as_ref()
            .and_then(|weak| weak.upgrade())
            .ok_or(StreamError::Closed)?;
        
        // Downcast to Fat32FileSystem
        let fat32_fs = fs.as_any()
            .downcast_ref::<crate::fs::vfs_v2::drivers::fat32::Fat32FileSystem>()
            .ok_or(StreamError::NotSupported)?;
        
        let cluster = self.node.cluster();
        
        // Write content to disk
        if content.len() > 0 {
            let new_cluster = fat32_fs.write_file_content(cluster, content)
                .map_err(|_| StreamError::IoError)?;
            
            // Update cluster if it changed
            if new_cluster != cluster {
                *self.node.cluster.write() = new_cluster;
            }
        }
        
        // Update file size in metadata
        {
            let mut metadata = self.node.metadata.write();
            metadata.size = content.len();
        }
        
        // Clear dirty flag
        *self.is_dirty.write() = false;
        
        Ok(())
    }
}

impl Debug for Fat32FileObject {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Fat32FileObject")
            .field("node", &self.node.name.read())
            .field("position", &self.position.read())
            .field("is_dirty", &self.is_dirty.read())
            .finish()
    }
}

impl StreamOps for Fat32FileObject {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        // Ensure content is loaded into cache
        self.ensure_content_loaded()?;
        
        let cached = self.cached_content.read();
        let content = cached.as_ref().ok_or(StreamError::IoError)?;
        
        let pos = *self.position.read();
        
        // Check if we're at or past EOF
        if pos >= content.len() {
            return Ok(0); // EOF
        }
        
        // Calculate how much we can read
        let remaining = content.len() - pos;
        let to_read = core::cmp::min(buffer.len(), remaining);
        
        if to_read == 0 {
            return Ok(0);
        }
        
        // Copy data from cached content
        buffer[..to_read].copy_from_slice(&content[pos..pos + to_read]);
        
        // Update position
        {
            let mut position = self.position.write();
            *position += to_read;
        }
        
        Ok(to_read)
    }
    
    fn write(&self, buffer: &[u8]) -> Result<usize, StreamError> {
        // Ensure content is loaded into cache
        self.ensure_content_loaded()?;
        
        let pos = *self.position.read();
        let mut cached = self.cached_content.write();
        let content = cached.as_mut().ok_or(StreamError::IoError)?;
        
        // Calculate new size
        let new_size = core::cmp::max(content.len(), pos + buffer.len());
        
        // Extend content if needed
        if new_size > content.len() {
            content.resize(new_size, 0);
        }
        
        // Write new data to cached content
        content[pos..pos + buffer.len()].copy_from_slice(buffer);
        
        // Mark as dirty
        *self.is_dirty.write() = true;
        
        // Update position
        {
            let mut position = self.position.write();
            *position += buffer.len();
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
    fn get_mapping_info(&self, offset: usize, length: usize) -> Result<(usize, usize, bool), &'static str> {
        // Ensure content is loaded into cache
        self.ensure_content_loaded().map_err(|_| "Failed to load file content")?;
        
        let cached = self.cached_content.read();
        let content = cached.as_ref().ok_or("No cached content available")?;
        
        // Check bounds
        if offset >= content.len() {
            return Err("Offset beyond file size");
        }
        
        let available_length = content.len() - offset;
        if length > available_length {
            return Err("Length extends beyond file size");
        }
        
        // Return the virtual address of the cached content as the physical address
        // This is a simplified implementation - in a real OS, this would involve
        // proper virtual-to-physical address translation
        let content_ptr = content.as_ptr() as usize;
        let paddr = content_ptr + offset;
        
        // Return read/write permissions (0x3 = read | write)
        // Not shared between processes (false)
        Ok((paddr, 0x3, false))
    }
    
    fn on_mapped(&self, _vaddr: usize, _paddr: usize, _length: usize, _offset: usize) {
        // For a simple implementation, we don't need to track mappings
        // In a more complex system, we might track active mappings here
    }
    
    fn on_unmapped(&self, _vaddr: usize, _length: usize) {
        // For a simple implementation, we don't need to track unmappings
        // In a more complex system, we might want to sync dirty pages here
        
        // Optionally sync to disk when unmapped
        let _ = self.sync_to_disk();
    }
    
    fn supports_mmap(&self) -> bool {
        true
    }
}

impl FileObject for Fat32FileObject {
    fn seek(&self, from: SeekFrom) -> Result<u64, StreamError> {
        let metadata = self.node.metadata.read();
        let file_size = metadata.size;
        let mut pos = self.position.write();
        
        let new_pos = match from {
            SeekFrom::Start(offset) => offset as usize,
            SeekFrom::End(offset) => {
                if offset < 0 {
                    file_size.saturating_sub((-offset) as usize)
                } else {
                    file_size + offset as usize
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
    
    fn sync(&self) -> Result<(), StreamError> {
        self.sync_to_disk()
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