//! EXT2 VFS Node Implementation
//!
//! This module implements the VfsNode trait for EXT2 filesystem nodes.
//! It provides the interface between the VFS layer and EXT2-specific node data.

use alloc::{
    collections::BTreeMap, string::String, sync::{Arc, Weak}, vec::Vec
};
use spin::rwlock::RwLock;
use core::{any::Any, fmt::Debug};

use crate::fs::{
    FileMetadata, FileObject, FilePermission, FileSystemError, FileType
};
use crate::object::capability::{StreamOps, StreamError, ControlOps, MemoryMappingOps};
use crate::object::capability::file::SeekFrom;

use crate::fs::vfs_v2::core::{VfsNode, FileSystemOperations};
use super::structures::Ext2Inode;

/// EXT2 filesystem node
/// 
/// This structure represents a file or directory in the EXT2 filesystem.
/// It implements the VfsNode trait to integrate with the VFS v2 architecture.
/// Content is read/written directly from/to the block device, not stored in memory.
pub struct Ext2Node {
    /// Node name
    pub name: RwLock<String>,
    /// File type (file or directory)
    pub file_type: RwLock<FileType>,
    /// File metadata
    pub metadata: RwLock<FileMetadata>,
    /// Child nodes (for directories) - cached, but loaded from disk on demand
    pub children: RwLock<BTreeMap<String, Arc<dyn VfsNode>>>,
    /// Parent node (weak reference to avoid cycles)
    pub parent: RwLock<Option<Weak<Ext2Node>>>,
    /// Reference to filesystem
    pub filesystem: RwLock<Option<Weak<dyn FileSystemOperations>>>,
    /// Inode number in EXT2
    pub inode_number: u32,
    /// Cached inode data
    pub inode_data: RwLock<Option<Ext2Inode>>,
    /// Directory entries loaded flag (for directories)
    pub children_loaded: RwLock<bool>,
}

impl Debug for Ext2Node {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Ext2Node")
            .field("name", &self.name.read())
            .field("file_type", &self.file_type.read())
            .field("metadata", &self.metadata.read())
            .field("inode_number", &self.inode_number)
            .field("children_loaded", &self.children_loaded.read())
            .finish()
    }
}

impl Ext2Node {
    /// Create a new EXT2 file node
    pub fn new_file(name: String, file_id: u64, inode_number: u32, size: usize) -> Self {
        let metadata = FileMetadata {
            file_type: FileType::RegularFile,
            size,
            permissions: FilePermission {
                owner_read: true,
                owner_write: true,
                owner_execute: false,
                group_read: true,
                group_write: false,
                group_execute: false,
                other_read: true,
                other_write: false,
                other_execute: false,
            },
            created_time: 0,
            modified_time: 0,
            accessed_time: 0,
            file_id,
            link_count: 1,
        };

        Self {
            name: RwLock::new(name),
            file_type: RwLock::new(FileType::RegularFile),
            metadata: RwLock::new(metadata),
            children: RwLock::new(BTreeMap::new()),
            parent: RwLock::new(None),
            filesystem: RwLock::new(None),
            inode_number,
            inode_data: RwLock::new(None),
            children_loaded: RwLock::new(false),
        }
    }

    /// Create a new EXT2 directory node
    pub fn new_directory(name: String, file_id: u64, inode_number: u32) -> Self {
        let metadata = FileMetadata {
            file_type: FileType::Directory,
            size: 0, // Directories have size 0 in metadata, actual size is in blocks
            permissions: FilePermission {
                owner_read: true,
                owner_write: true,
                owner_execute: true,
                group_read: true,
                group_write: false,
                group_execute: true,
                other_read: true,
                other_write: false,
                other_execute: true,
            },
            created_time: 0,
            modified_time: 0,
            accessed_time: 0,
            file_id,
            link_count: 2, // Directories start with 2 links (. and parent/..)
        };

        Self {
            name: RwLock::new(name),
            file_type: RwLock::new(FileType::Directory),
            metadata: RwLock::new(metadata),
            children: RwLock::new(BTreeMap::new()),
            parent: RwLock::new(None),
            filesystem: RwLock::new(None),
            inode_number,
            inode_data: RwLock::new(None),
            children_loaded: RwLock::new(false),
        }
    }

    /// Set the filesystem reference
    pub fn set_filesystem(&self, filesystem: Weak<dyn FileSystemOperations>) {
        *self.filesystem.write() = Some(filesystem);
    }

    /// Set parent node
    pub fn set_parent(&self, parent: Weak<Ext2Node>) {
        *self.parent.write() = Some(parent);
    }

    /// Add a child node
    pub fn add_child(&self, name: String, child: Arc<dyn VfsNode>) {
        self.children.write().insert(name, child);
    }

    /// Remove a child node
    pub fn remove_child(&self, name: &str) {
        self.children.write().remove(name);
    }

    /// Check if children have been loaded from disk
    pub fn are_children_loaded(&self) -> bool {
        *self.children_loaded.read()
    }

    /// Mark children as loaded
    pub fn mark_children_loaded(&self) {
        *self.children_loaded.write() = true;
    }

    /// Set the cached inode data
    pub fn set_inode_data(&self, inode: Ext2Inode) {
        *self.inode_data.write() = Some(inode);
        
        // Update metadata from inode
        let mut metadata = self.metadata.write();
        metadata.size = inode.size as usize;
        metadata.accessed_time = inode.atime as u64;
        metadata.modified_time = inode.mtime as u64;
        metadata.created_time = inode.ctime as u64;
        
        // Update file type based on inode mode
        let file_type = if inode.is_directory() {
            FileType::Directory
        } else if inode.is_regular_file() {
            FileType::RegularFile
        } else if inode.is_symbolic_link() {
            FileType::SymbolicLink
        } else {
            FileType::RegularFile // Default fallback
        };
        
        *self.file_type.write() = file_type;
        metadata.file_type = file_type;
    }

    /// Get the cached inode data
    pub fn get_inode_data(&self) -> Option<Ext2Inode> {
        *self.inode_data.read()
    }

    /// Get the node name
    pub fn name(&self) -> String {
        self.name.read().clone()
    }
}

impl VfsNode for Ext2Node {
    fn id(&self) -> u64 {
        self.metadata().file_id
    }

    fn metadata(&self) -> Result<FileMetadata, FileSystemError> {
        Ok(self.metadata.read().clone())
    }

    fn file_type(&self) -> Result<FileType, FileSystemError> {
        Ok(*self.file_type.read())
    }

    fn filesystem(&self) -> Option<Weak<dyn FileSystemOperations>> {
        self.filesystem.read().clone()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// EXT2 File Object for handling file I/O operations
pub struct Ext2FileObject {
    /// Reference to the EXT2 node
    node: Arc<Ext2Node>,
    /// Current file position
    position: RwLock<u64>,
    /// Open flags
    flags: u32,
}

impl Ext2FileObject {
    /// Create a new EXT2 file object
    pub fn new(node: Arc<Ext2Node>, flags: u32) -> Self {
        Self {
            node,
            position: RwLock::new(0),
            flags,
        }
    }
}

impl Debug for Ext2FileObject {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Ext2FileObject")
            .field("node", &self.node)
            .field("position", &self.position.read())
            .field("flags", &self.flags)
            .finish()
    }
}

impl StreamOps for Ext2FileObject {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        // For now, return empty read - will be implemented when filesystem supports block I/O
        let _ = buffer;
        Ok(0)
    }

    fn write(&self, buffer: &[u8]) -> Result<usize, StreamError> {
        // For now, return error - will be implemented when filesystem supports block I/O
        let _ = buffer;
        Err(StreamError::IoError)
    }
}

impl FileObject for Ext2FileObject {
    fn seek(&self, pos: SeekFrom) -> Result<u64, StreamError> {
        let mut position = self.position.write();
        let file_size = self.node.metadata().map_err(|e| StreamError::FileSystemError(e))?.size;

        let new_pos = match pos {
            SeekFrom::Start(offset) => offset,
            SeekFrom::End(offset) => {
                if offset >= 0 {
                    file_size + offset as u64
                } else {
                    if file_size < (-offset) as u64 {
                        0
                    } else {
                        file_size - (-offset) as u64
                    }
                }
            }
            SeekFrom::Current(offset) => {
                if offset >= 0 {
                    *position + offset as u64
                } else {
                    if *position < (-offset) as u64 {
                        0
                    } else {
                        *position - (-offset) as u64
                    }
                }
            }
        };

        *position = new_pos;
        Ok(new_pos)
    }

    fn metadata(&self) -> Result<crate::fs::FileMetadata, StreamError> {
        self.node.metadata().map_err(|e| StreamError::FileSystemError(e))
    }
}

impl ControlOps for Ext2FileObject {
    fn control(&self, _command: u32, _arg: usize) -> Result<i32, &'static str> {
        Err("Control operations not supported for EXT2 files")
    }
}

impl MemoryMappingOps for Ext2FileObject {
    fn get_mapping_info(&self, _offset: usize, _length: usize) 
                       -> Result<(usize, usize, bool), &'static str> {
        Err("Memory mapping not supported for EXT2 files")
    }

    fn on_mapped(&self, _vaddr: usize, _paddr: usize, _length: usize, _offset: usize) {
        // EXT2 files don't support memory mapping
    }

    fn on_unmapped(&self, _vaddr: usize, _length: usize) {
        // EXT2 files don't support memory mapping
    }

    fn supports_mmap(&self) -> bool {
        false
    }
}