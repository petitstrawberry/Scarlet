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
}

impl Ext2FileObject {
    /// Create a new ext2 file object
    pub fn new(inode_number: u32, file_id: u64) -> Self {
        Self {
            inode_number,
            file_id,
            position: Mutex::new(0),
        }
    }

    /// Get the file ID
    pub fn file_id(&self) -> u64 {
        self.file_id
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
    fn get_mapping_info(&self, _offset: usize, _length: usize) -> Result<(usize, usize, bool), &'static str> {
        Err("Memory mapping not supported for ext2 files")
    }
}

impl FileObject for Ext2FileObject {
    fn metadata(&self) -> Result<FileMetadata, StreamError> {
        // TODO: Read inode metadata
        Ok(FileMetadata {
            file_type: FileType::RegularFile,
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
}

impl Ext2DirectoryObject {
    /// Create a new ext2 directory object
    pub fn new(inode_number: u32, file_id: u64) -> Self {
        Self {
            inode_number,
            file_id,
            position: Mutex::new(0),
        }
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
        // TODO: Read inode metadata
        Ok(FileMetadata {
            file_type: FileType::Directory,
            size: 0, // Would read from inode
            permissions: FilePermission {
                read: true,
                write: true,
                execute: true,
            },
            created_time: 0,
            modified_time: 0,
            accessed_time: 0,
            file_id: self.file_id,
            link_count: 1,
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