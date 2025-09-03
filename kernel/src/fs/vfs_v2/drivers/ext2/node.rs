//! ext2 VFS Node Implementation
//!
//! This module implements the VFS node interface for ext2 filesystem nodes,
//! providing file and directory objects that integrate with the VFS v2 architecture.

use alloc::{sync::{Arc, Weak}, string::String, vec::Vec, boxed::Box};
use spin::{RwLock, Mutex};
use core::{any::Any, fmt::Debug};

use crate::{
    fs::{
        FileObject, FileSystemError, FileSystemErrorKind, FileType, SeekFrom,
        FileMetadata, FilePermission
    },
    object::capability::{StreamOps, ControlOps, MemoryMappingOps, StreamError}
};

use crate::fs::vfs_v2::core::{VfsNode, FileSystemOperations};
use super::{Ext2FileSystem, structures::{Ext2Inode, EXT2_S_IFMT, EXT2_S_IFREG, EXT2_S_IFDIR}};

/// ext2 VFS Node
///
/// Represents a file or directory in the ext2 filesystem. This node
/// implements the VfsNode trait and provides access to ext2-specific
/// file operations.
#[derive(Debug)]
pub struct Ext2Node {
    /// Inode number in the ext2 filesystem
    inode_number: u32,
    /// File type (directory, regular file, etc.)
    file_type: FileType,
    /// Unique file ID for VFS
    file_id: u64,
    /// Weak reference to the filesystem
    filesystem: RwLock<Option<Weak<dyn FileSystemOperations>>>,
}

impl Ext2Node {
    /// Create a new ext2 node
    pub fn new(inode_number: u32, file_type: FileType, file_id: u64) -> Self {
        Self {
            inode_number,
            file_type,
            file_id,
            filesystem: RwLock::new(None),
        }
    }

    /// Get the inode number
    pub fn inode_number(&self) -> u32 {
        self.inode_number
    }

    /// Set the filesystem reference
    pub fn set_filesystem(&self, fs: Weak<dyn FileSystemOperations>) {
        *self.filesystem.write() = Some(fs);
    }

    /// Get the filesystem reference
    pub fn filesystem(&self) -> Option<Weak<dyn FileSystemOperations>> {
        self.filesystem.read().clone()
    }
}

impl VfsNode for Ext2Node {
    fn id(&self) -> u64 {
        self.file_id
    }

    fn filesystem(&self) -> Option<Weak<dyn FileSystemOperations>> {
        self.filesystem.read().clone()
    }

    fn file_type(&self) -> Result<FileType, FileSystemError> {
        Ok(self.file_type.clone())
    }

    fn metadata(&self) -> Result<FileMetadata, FileSystemError> {
        // For now, return basic metadata
        // In a full implementation, we'd read the inode to get real metadata
        Ok(FileMetadata {
            file_type: self.file_type.clone(),
            size: 0, // Would read from inode
            permissions: FilePermission {
                read: true,
                write: true,
                execute: false,
            },
            created_time: 0,
            modified_time: 0,
            accessed_time: 0,
            file_id: self.file_id,
            link_count: 1,
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// ext2 File Object
///
/// Handles file operations for regular files in the ext2 filesystem.
#[derive(Debug)]
pub struct Ext2FileObject {
    /// Inode number of the file
    inode_number: u32,
    /// File ID
    file_id: u64,
    /// Current position in the file
    position: Mutex<u64>,
    /// Weak reference to the filesystem
    filesystem: RwLock<Option<Weak<dyn FileSystemOperations>>>,
    /// Cached file content in memory (lazily loaded)
    cached_content: RwLock<Option<Vec<u8>>>,
    /// Whether the cached content has been modified
    is_dirty: RwLock<bool>,
}

impl Ext2FileObject {
    /// Create a new ext2 file object
    pub fn new(inode_number: u32, file_id: u64) -> Self {
        Self {
            inode_number,
            file_id,
            position: Mutex::new(0),
            filesystem: RwLock::new(None),
            cached_content: RwLock::new(None),
            is_dirty: RwLock::new(false),
        }
    }

    /// Set the filesystem reference
    pub fn set_filesystem(&self, fs: Weak<dyn FileSystemOperations>) {
        *self.filesystem.write() = Some(fs);
    }

    /// Get the file ID
    pub fn file_id(&self) -> u64 {
        self.file_id
    }

    /// Load file content from disk into cache if not already loaded
    fn ensure_content_loaded(&self) -> Result<(), StreamError> {
        let mut cached = self.cached_content.write();
        
        // If already loaded, nothing to do
        if cached.is_some() {
            return Ok(());
        }
        
        // Get filesystem reference
        let fs = self.filesystem.read()
            .as_ref()
            .and_then(|weak| weak.upgrade())
            .ok_or(StreamError::Closed)?;
        
        // Downcast to Ext2FileSystem
        let ext2_fs = fs.as_any()
            .downcast_ref::<Ext2FileSystem>()
            .ok_or(StreamError::NotSupported)?;
        
        // Read inode to get file size
        let inode = ext2_fs.read_inode(self.inode_number)
            .map_err(|_| StreamError::IoError)?;
        
        // Read entire file content from disk
        let content = if inode.size > 0 {
            ext2_fs.read_file_content(self.inode_number, inode.size as usize)
                .map_err(|_| StreamError::IoError)?
        } else {
            Vec::new()
        };
        
        *cached = Some(content);
        Ok(())
    }

    /// Sync cached content to disk
    fn sync_to_disk(&self) -> Result<(), StreamError> {
        let is_dirty = *self.is_dirty.read();
        if !is_dirty {
            return Ok(());
        }

        // For now, just mark as clean since we don't implement writing yet
        *self.is_dirty.write() = false;
        Ok(())
    }
}

impl StreamOps for Ext2FileObject {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        // TODO: Implement file reading from ext2 blocks
        // For now, return empty read
        Ok(0)
    }

    fn write(&self, _buffer: &[u8]) -> Result<usize, StreamError> {
        // TODO: Implement file writing to ext2 blocks
        Err(StreamError::IoError)
    }
}

impl ControlOps for Ext2FileObject {
}

impl MemoryMappingOps for Ext2FileObject {
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

impl FileObject for Ext2FileObject {
    fn metadata(&self) -> Result<FileMetadata, StreamError> {
        // Get filesystem reference
        let fs = self.filesystem.read()
            .as_ref()
            .and_then(|weak| weak.upgrade())
            .ok_or(StreamError::Closed)?;
        
        // Downcast to Ext2FileSystem
        let ext2_fs = fs.as_any()
            .downcast_ref::<Ext2FileSystem>()
            .ok_or(StreamError::NotSupported)?;
        
        // Read inode metadata
        let inode = ext2_fs.read_inode(self.inode_number)
            .map_err(|_| StreamError::IoError)?;
        
        // Convert inode permissions to FilePermission
        let permissions = FilePermission {
            read: (inode.mode & 0o444) != 0,
            write: (inode.mode & 0o222) != 0,
            execute: (inode.mode & 0o111) != 0,
        };
        
        // Determine file type from inode mode
        let file_type = if (inode.mode & EXT2_S_IFMT) == EXT2_S_IFREG {
            FileType::RegularFile
        } else if (inode.mode & EXT2_S_IFMT) == EXT2_S_IFDIR {
            FileType::Directory
        } else {
            FileType::RegularFile // Default fallback
        };
        
        Ok(FileMetadata {
            file_type,
            size: inode.size as usize,
            permissions,
            created_time: inode.ctime as u64,
            modified_time: inode.mtime as u64,
            accessed_time: inode.atime as u64,
            file_id: self.file_id,
            link_count: inode.links_count as u32,
        })
    }

    fn seek(&self, whence: SeekFrom) -> Result<u64, StreamError> {
        let mut pos = self.position.lock();
        
        match whence {
            SeekFrom::Start(offset) => {
                *pos = offset;
                Ok(*pos)
            },
            SeekFrom::Current(offset) => {
                if offset >= 0 {
                    *pos += offset as u64;
                } else {
                    let abs_offset = (-offset) as u64;
                    if abs_offset > *pos {
                        *pos = 0;
                    } else {
                        *pos -= abs_offset;
                    }
                }
                Ok(*pos)
            },
            SeekFrom::End(_offset) => {
                // TODO: Get actual file size from inode
                Err(StreamError::IoError)
            }
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// ext2 Directory Object
///
/// Handles directory operations for directories in the ext2 filesystem.
#[derive(Debug)]
pub struct Ext2DirectoryObject {
    /// Inode number of the directory
    inode_number: u32,
    /// File ID
    file_id: u64,
    /// Current position in directory listing
    position: Mutex<u64>,
    /// Weak reference to the filesystem
    filesystem: RwLock<Option<Weak<dyn FileSystemOperations>>>,
}

impl Ext2DirectoryObject {
    /// Create a new ext2 directory object
    pub fn new(inode_number: u32, file_id: u64) -> Self {
        Self {
            inode_number,
            file_id,
            position: Mutex::new(0),
            filesystem: RwLock::new(None),
        }
    }

    /// Set the filesystem reference
    pub fn set_filesystem(&self, fs: Weak<dyn FileSystemOperations>) {
        *self.filesystem.write() = Some(fs);
    }
}

impl StreamOps for Ext2DirectoryObject {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        // TODO: Implement directory reading
        // For now, return empty read to indicate end of directory
        Ok(0)
    }

    fn write(&self, _buffer: &[u8]) -> Result<usize, StreamError> {
        Err(StreamError::IoError)
    }
}

impl ControlOps for Ext2DirectoryObject {
}

impl MemoryMappingOps for Ext2DirectoryObject {
    fn get_mapping_info(&self, _offset: usize, _length: usize) -> Result<(usize, usize, bool), &'static str> {
        Err("Memory mapping not supported for directories")
    }
}

impl FileObject for Ext2DirectoryObject {
    fn metadata(&self) -> Result<FileMetadata, StreamError> {
        // Get filesystem reference
        let fs = self.filesystem.read()
            .as_ref()
            .and_then(|weak| weak.upgrade())
            .ok_or(StreamError::Closed)?;
        
        // Downcast to Ext2FileSystem
        let ext2_fs = fs.as_any()
            .downcast_ref::<Ext2FileSystem>()
            .ok_or(StreamError::NotSupported)?;
        
        // Read inode metadata
        let inode = ext2_fs.read_inode(self.inode_number)
            .map_err(|_| StreamError::IoError)?;
        
        // Convert inode permissions to FilePermission
        let permissions = FilePermission {
            read: (inode.mode & 0o444) != 0,
            write: (inode.mode & 0o222) != 0,
            execute: (inode.mode & 0o111) != 0,
        };
        
        Ok(FileMetadata {
            file_type: FileType::Directory,
            size: inode.size as usize,
            permissions,
            created_time: inode.ctime as u64,
            modified_time: inode.mtime as u64,
            accessed_time: inode.atime as u64,
            file_id: self.file_id,
            link_count: inode.links_count as u32,
        })
    }

    fn seek(&self, whence: SeekFrom) -> Result<u64, StreamError> {
        let mut pos = self.position.lock();
        
        match whence {
            SeekFrom::Start(offset) => {
                *pos = offset;
                Ok(*pos)
            },
            _ => Err(StreamError::IoError)
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}