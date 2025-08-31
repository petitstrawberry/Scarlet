//! Ext2 VFS Node Implementation
//!
//! This module implements the VFS node interface for ext2 filesystem nodes.
//! It provides file and directory objects that integrate with the VFS v2 architecture.

use alloc::{collections::BTreeMap, string::{String, ToString}, sync::{Arc, Weak}, vec::Vec};
use spin::{rwlock::RwLock, Mutex};
use core::{any::Any, fmt::Debug};

use crate::{
    fs::{FileMetadata, FileObject, FileSystemError, FileSystemErrorKind, FileType, SeekFrom, FilePermission, DeviceFileInfo},
    object::capability::{StreamOps, ControlOps, MemoryMappingOps, StreamError},
    device::DeviceType
};

use crate::fs::vfs_v2::core::{VfsNode, FileSystemOperations};
use super::structures::*;

/// Ext2 VFS Node implementation
/// 
/// This represents a file or directory in the ext2 filesystem.
/// It implements the VfsNode trait to integrate with the VFS v2 architecture.
#[derive(Debug)]
pub struct Ext2Node {
    /// Node name
    name: String,
    /// File type
    file_type: RwLock<FileType>,
    /// File metadata
    pub metadata: RwLock<FileMetadata>,
    /// Inode number
    pub inode_num: u32,
    /// Inode data
    pub inode: RwLock<Ext2Inode>,
    /// Reference to the filesystem
    pub filesystem: RwLock<Option<Weak<dyn FileSystemOperations>>>,
    /// Child nodes (for directories)
    pub children: RwLock<BTreeMap<String, Arc<dyn VfsNode>>>,
    /// Cached file content
    pub cached_content: RwLock<Option<Vec<u8>>>,
    /// Whether content is dirty and needs to be written back
    pub is_dirty: RwLock<bool>,
}

impl Ext2Node {
    /// Create a new ext2 node from an inode
    pub fn new_from_inode(name: String, inode_num: u32, inode: &Ext2Inode) -> Self {
        let file_type = Self::ext2_mode_to_file_type(inode.mode);
        
        let metadata = FileMetadata {
            file_id: inode_num as u64,
            size: inode.size as usize,
            accessed_time: inode.atime as u64,
            modified_time: inode.mtime as u64,
            created_time: inode.ctime as u64,
            file_type: file_type.clone(),
            permissions: Self::ext2_mode_to_permissions(inode.mode),
            link_count: inode.links_count as u32,
        };
        
        Self {
            name,
            file_type: RwLock::new(file_type),
            metadata: RwLock::new(metadata),
            inode_num,
            inode: RwLock::new(*inode),
            filesystem: RwLock::new(None),
            children: RwLock::new(BTreeMap::new()),
            cached_content: RwLock::new(None),
            is_dirty: RwLock::new(false),
        }
    }
    
    /// Convert ext2 file mode to VFS file type
    fn ext2_mode_to_file_type(mode: u16) -> FileType {
        match mode & EXT2_S_IFMT {
            EXT2_S_IFREG => FileType::RegularFile,
            EXT2_S_IFDIR => FileType::Directory,
            EXT2_S_IFLNK => {
                // For symbolic links, we'd need to read the target
                // For now, just use an empty target
                FileType::SymbolicLink(String::new())
            },
            EXT2_S_IFCHR => {
                // Create dummy device info
                FileType::CharDevice(DeviceFileInfo {
                    device_id: 0,
                    device_type: DeviceType::Char,
                })
            },
            EXT2_S_IFBLK => {
                // Create dummy device info
                FileType::BlockDevice(DeviceFileInfo {
                    device_id: 0,
                    device_type: DeviceType::Block,
                })
            },
            EXT2_S_IFIFO => FileType::Pipe,
            EXT2_S_IFSOCK => FileType::Socket,
            _ => FileType::Unknown, // Default to unknown
        }
    }
    
    /// Convert ext2 file mode to VFS file permissions
    fn ext2_mode_to_permissions(mode: u16) -> FilePermission {
        FilePermission {
            read: (mode & EXT2_S_IRUSR) != 0,
            write: (mode & EXT2_S_IWUSR) != 0,
            execute: (mode & EXT2_S_IXUSR) != 0,
        }
    }
    
    /// Set the filesystem reference
    pub fn set_filesystem(&self, fs: Weak<dyn FileSystemOperations>) {
        *self.filesystem.write() = Some(fs);
    }
    
    /// Add a child node (for directories)
    pub fn add_child(&self, name: String, node: Arc<dyn VfsNode>) -> Result<(), FileSystemError> {
        match *self.file_type.read() {
            FileType::Directory => {
                let mut children = self.children.write();
                children.insert(name, node);
                Ok(())
            },
            _ => Err(FileSystemError::new(
                FileSystemErrorKind::NotADirectory,
                "Cannot add child to non-directory"
            ))
        }
    }
    
    /// Get a child node by name (for directories)
    pub fn get_child(&self, name: &str) -> Option<Arc<dyn VfsNode>> {
        let children = self.children.read();
        children.get(name).cloned()
    }
    
    /// Get the name of this node
    pub fn name(&self) -> String {
        self.name.clone()
    }
}

impl Clone for Ext2Node {
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            file_type: RwLock::new(self.file_type.read().clone()),
            metadata: RwLock::new(self.metadata.read().clone()),
            inode_num: self.inode_num,
            inode: RwLock::new(*self.inode.read()),
            filesystem: RwLock::new(self.filesystem.read().clone()),
            children: RwLock::new(self.children.read().clone()),
            cached_content: RwLock::new(self.cached_content.read().clone()),
            is_dirty: RwLock::new(*self.is_dirty.read()),
        }
    }
}

impl VfsNode for Ext2Node {
    fn id(&self) -> u64 {
        self.inode_num as u64
    }
    
    fn file_type(&self) -> Result<FileType, FileSystemError> {
        Ok(self.file_type.read().clone())
    }
    
    fn metadata(&self) -> Result<FileMetadata, FileSystemError> {
        Ok(self.metadata.read().clone())
    }
    
    fn filesystem(&self) -> Option<Weak<dyn FileSystemOperations>> {
        self.filesystem.read().clone()
    }
    
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Ext2 File Object for regular files
/// 
/// This implements the FileObject trait for reading and writing ext2 files.
pub struct Ext2FileObject {
    /// Reference to the ext2 node
    pub node: Arc<Ext2Node>,
    /// Current file position
    pub position: RwLock<usize>,
    /// Cached file content
    pub cached_content: RwLock<Option<Vec<u8>>>,
    /// Whether content is dirty
    pub is_dirty: RwLock<bool>,
}

impl Ext2FileObject {
    /// Create a new ext2 file object
    pub fn new(node: Arc<Ext2Node>) -> Self {
        Self {
            node,
            position: RwLock::new(0),
            cached_content: RwLock::new(None),
            is_dirty: RwLock::new(false),
        }
    }
    
    /// Ensure file content is loaded into cache
    fn ensure_content_loaded(&self) -> Result<(), StreamError> {
        let mut cached = self.cached_content.write();
        if cached.is_none() {
            // Load content from filesystem
            // For now, return empty content
            // TODO: Implement actual file content reading from blocks
            *cached = Some(Vec::new());
        }
        Ok(())
    }
    
    /// Write cached content back to disk if dirty
    fn sync_to_disk(&self) -> Result<(), StreamError> {
        let is_dirty = *self.is_dirty.read();
        if !is_dirty {
            return Ok(()); // Nothing to sync
        }
        
        // TODO: Implement actual writing to disk
        // For now, just mark as clean
        *self.is_dirty.write() = false;
        Ok(())
    }
}

impl Debug for Ext2FileObject {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Ext2FileObject")
            .field("node", &self.node)
            .field("position", &self.position.read())
            .finish()
    }
}

impl StreamOps for Ext2FileObject {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        // Ensure content is loaded
        self.ensure_content_loaded()?;
        
        let pos = *self.position.read();
        let cached = self.cached_content.read();
        let content = cached.as_ref().ok_or(StreamError::IoError)?;
        
        let available = content.len().saturating_sub(pos);
        let to_read = core::cmp::min(buffer.len(), available);
        
        if to_read > 0 {
            buffer[..to_read].copy_from_slice(&content[pos..pos + to_read]);
            
            // Update position
            {
                let mut position = self.position.write();
                *position += to_read;
            }
        }
        
        Ok(to_read)
    }
    
    fn write(&self, buffer: &[u8]) -> Result<usize, StreamError> {
        // Ensure content is loaded
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
        
        // Write new data
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

impl ControlOps for Ext2FileObject {
    fn control(&self, _command: u32, _arg: usize) -> Result<i32, &'static str> {
        Err("Control operations not supported for ext2 files")
    }
}

impl MemoryMappingOps for Ext2FileObject {
    fn get_mapping_info(&self, _offset: usize, _length: usize) -> Result<(usize, usize, bool), &'static str> {
        Err("Memory mapping not yet implemented for ext2 files")
    }
    
    fn on_mapped(&self, _vaddr: usize, _paddr: usize, _length: usize, _offset: usize) {
        // Not implemented yet
    }
    
    fn on_unmapped(&self, _vaddr: usize, _length: usize) {
        // Not implemented yet
    }
}

impl FileObject for Ext2FileObject {
    fn seek(&self, from: SeekFrom) -> Result<u64, StreamError> {
        let mut position = self.position.write();
        let size = self.node.metadata.read().size;
        
        let new_pos = match from {
            SeekFrom::Start(offset) => offset as usize,
            SeekFrom::End(offset) => {
                if offset < 0 {
                    size.saturating_sub((-offset) as usize)
                } else {
                    size + offset as usize
                }
            },
            SeekFrom::Current(offset) => {
                if offset < 0 {
                    position.saturating_sub((-offset) as usize)
                } else {
                    *position + offset as usize
                }
            }
        };
        
        *position = new_pos;
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

/// Ext2 Directory Object for directories
/// 
/// This implements the FileObject trait for reading ext2 directories.
pub struct Ext2DirectoryObject {
    /// Reference to the ext2 node
    pub node: Arc<Ext2Node>,
    /// Current position in directory entries
    pub position: RwLock<usize>,
}

impl Ext2DirectoryObject {
    /// Create a new ext2 directory object
    pub fn new(node: Arc<Ext2Node>) -> Self {
        Self {
            node,
            position: RwLock::new(0),
        }
    }
}

impl Debug for Ext2DirectoryObject {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Ext2DirectoryObject")
            .field("node", &self.node)
            .field("position", &self.position.read())
            .finish()
    }
}

impl StreamOps for Ext2DirectoryObject {
    fn read(&self, _buffer: &mut [u8]) -> Result<usize, StreamError> {
        // TODO: Implement directory entry reading
        Err(StreamError::NotSupported)
    }
    
    fn write(&self, _buf: &[u8]) -> Result<usize, StreamError> {
        Err(StreamError::FileSystemError(FileSystemError::new(
            FileSystemErrorKind::ReadOnly,
            "Cannot write to directory"
        )))
    }
}

impl ControlOps for Ext2DirectoryObject {
    fn control(&self, _command: u32, _arg: usize) -> Result<i32, &'static str> {
        Err("Control operations not supported for ext2 directories")
    }
}

impl MemoryMappingOps for Ext2DirectoryObject {
    fn get_mapping_info(&self, _offset: usize, _length: usize) -> Result<(usize, usize, bool), &'static str> {
        Err("Memory mapping not supported for ext2 directories")
    }
    
    fn on_mapped(&self, _vaddr: usize, _paddr: usize, _length: usize, _offset: usize) {
        // Not supported
    }
    
    fn on_unmapped(&self, _vaddr: usize, _length: usize) {
        // Not supported
    }
}

impl FileObject for Ext2DirectoryObject {
    fn seek(&self, from: SeekFrom) -> Result<u64, StreamError> {
        let mut position = self.position.write();
        
        let new_pos = match from {
            SeekFrom::Start(offset) => offset as usize,
            SeekFrom::End(_) => 0, // End of directory
            SeekFrom::Current(offset) => {
                if offset < 0 {
                    position.saturating_sub((-offset) as usize)
                } else {
                    *position + offset as usize
                }
            }
        };
        
        *position = new_pos;
        Ok(new_pos as u64)
    }
    
    fn metadata(&self) -> Result<crate::fs::FileMetadata, StreamError> {
        Ok(self.node.metadata.read().clone())
    }
    
    fn sync(&self) -> Result<(), StreamError> {
        Ok(()) // Nothing to sync for directories
    }
    
    fn as_any(&self) -> &dyn Any {
        self
    }
}