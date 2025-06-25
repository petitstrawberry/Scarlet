//! CpioFS v2 - CPIO filesystem implementation for initramfs
//!
//! This is a simplified read-only filesystem for handling CPIO archives
//! used as initramfs. It implements the VFS v2 architecture.

use alloc::{
    collections::BTreeMap,
    string::{String, ToString},
    sync::Arc,
    vec::Vec,
};
use spin::RwLock;
use core::any::Any;
use alloc::sync::Weak;

use crate::fs::vfs_v2::core::{
    VfsNode, FileSystemOperations, FileSystemRef
};
use crate::fs::{
    FileSystemError, FileSystemErrorKind, FileMetadata, FileObject, FileType, FilePermission
};
use crate::object::capability::{StreamOps, StreamError};

/// CPIO filesystem implementation
pub struct CpioFS {
    /// Root node of the filesystem
    root_node: Arc<CpioNode>,
    
    /// Filesystem name
    name: String,
}

/// A single node in the CPIO filesystem
pub struct CpioNode {
    /// File name
    name: String,
    
    /// File type
    file_type: FileType,
    
    /// File content (for regular files)
    content: Vec<u8>,
    
    /// Child nodes (for directories)
    children: RwLock<BTreeMap<String, Arc<CpioNode>>>,
    
    /// Reference to filesystem
    filesystem: RwLock<Option<Arc<CpioFS>>>,
    
    /// File ID
    file_id: usize,
}

impl CpioNode {
    /// Create a new CPIO node
    pub fn new(name: String, file_type: FileType, content: Vec<u8>, file_id: usize) -> Arc<Self> {
        Arc::new(Self {
            name,
            file_type,
            content,
            children: RwLock::new(BTreeMap::new()),
            filesystem: RwLock::new(None),
            file_id,
        })
    }
    
    /// Add a child to this directory node
    pub fn add_child(&self, name: String, child: Arc<CpioNode>) -> Result<(), FileSystemError> {
        if self.file_type != FileType::Directory {
            return Err(FileSystemError::new(
                FileSystemErrorKind::NotADirectory,
                "Cannot add child to non-directory node"
            ));
        }
        
        let mut children = self.children.write();
        children.insert(name, child);
        Ok(())
    }
    
    /// Get a child by name
    pub fn get_child(&self, name: &str) -> Option<Arc<CpioNode>> {
        let children = self.children.read();
        children.get(name).cloned()
    }
}

impl VfsNode for CpioNode {
    fn filesystem(&self) -> Option<Weak<dyn FileSystemOperations>> {
        let fs_guard = self.filesystem.read();
        fs_guard.as_ref().map(|fs| Arc::downgrade(fs) as Weak<dyn FileSystemOperations>)
    }
    
    fn metadata(&self) -> Result<FileMetadata, FileSystemError> {
        Ok(FileMetadata {
            file_type: self.file_type,
            size: self.content.len(),
            created_time: 0,
            modified_time: 0,
            accessed_time: 0,
            permissions: FilePermission {
                read: true,
                write: false,
                execute: false,
            },
            file_id: self.file_id as u64,
            link_count: 1,
        })
    }
    
    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl CpioFS {
    /// Create a new CpioFS from CPIO archive data
    pub fn new(name: String, cpio_data: &[u8]) -> Result<Arc<Self>, FileSystemError> {
        let root_node = CpioNode::new("/".to_string(), FileType::Directory, Vec::new(), 1);
        
        let filesystem = Arc::new(Self {
            root_node: Arc::clone(&root_node),
            name,
        });
        
        // Set filesystem reference in root node
        {
            let mut fs_guard = root_node.filesystem.write();
            *fs_guard = Some(Arc::clone(&filesystem));
        }
        
        // Parse CPIO data and build directory tree
        // For now, simplified implementation - just create root
        // In a real implementation, you would parse the CPIO format here
        
        Ok(filesystem)
    }
    
    /// Parse CPIO archive and build directory tree (simplified)
    fn parse_cpio_archive(&self, _data: &[u8]) -> Result<(), FileSystemError> {
        // TODO: Implement actual CPIO parsing
        // This would extract files and directories from the CPIO archive
        // and build the directory tree by calling add_child on nodes
        Ok(())
    }
}

impl FileSystemOperations for CpioFS {
    fn lookup(
        &self,
        parent_node: Arc<dyn VfsNode>,
        name: &String,
    ) -> Result<Arc<dyn VfsNode>, FileSystemError> {
        // Downcast to CpioNode
        let cpio_parent = parent_node.as_any()
            .downcast_ref::<CpioNode>()
            .ok_or_else(|| FileSystemError::new(
                FileSystemErrorKind::NotSupported,
                "Invalid node type for CpioFS"
            ))?;
        
        // Look up child
        cpio_parent.get_child(name)
            .map(|child| child as Arc<dyn VfsNode>)
            .ok_or_else(|| FileSystemError::new(
                FileSystemErrorKind::NotFound,
                "File not found"
            ))
    }
    
    fn open(
        &self,
        node: Arc<dyn VfsNode>,
        _flags: u32,
    ) -> Result<Arc<dyn FileObject>, FileSystemError> {
        let cpio_node = node.as_any()
            .downcast_ref::<CpioNode>()
            .ok_or_else(|| FileSystemError::new(
                FileSystemErrorKind::NotSupported,
                "Invalid node type for CpioFS"
            ))?;
        
        match cpio_node.file_type {
            FileType::RegularFile => {
                Ok(Arc::new(CpioFileObject::new(node)))
            },
            FileType::Directory => {
                Ok(Arc::new(CpioDirectoryObject::new(node)))
            },
            _ => Err(FileSystemError::new(
                FileSystemErrorKind::NotSupported,
                "Unsupported file type"
            ))
        }
    }
    
    fn create(
        &self,
        _parent_node: Arc<dyn VfsNode>,
        _name: &String,
        _file_type: FileType,
        _mode: u32,
    ) -> Result<Arc<dyn VfsNode>, FileSystemError> {
        Err(FileSystemError::new(
            FileSystemErrorKind::ReadOnly,
            "CPIO filesystem is read-only"
        ))
    }
    
    fn remove(
        &self,
        _parent_node: Arc<dyn VfsNode>,
        _name: &String,
    ) -> Result<(), FileSystemError> {
        Err(FileSystemError::new(
            FileSystemErrorKind::ReadOnly,
            "CPIO filesystem is read-only"
        ))
    }
    
    fn root_node(&self) -> Arc<dyn VfsNode> {
        Arc::clone(&self.root_node) as Arc<dyn VfsNode>
    }
    
    fn name(&self) -> &str {
        &self.name
    }
    
    fn is_read_only(&self) -> bool {
        true
    }
    
    fn readdir(
        &self,
        node: Arc<dyn VfsNode>,
    ) -> Result<Vec<super::DirectoryEntryInternal>, FileSystemError> {
        todo!()
    }
}

/// File object for CPIO regular files
pub struct CpioFileObject {
    node: Arc<dyn VfsNode>,
    position: RwLock<u64>,
}

impl CpioFileObject {
    pub fn new(node: Arc<dyn VfsNode>) -> Self {
        Self {
            node,
            position: RwLock::new(0),
        }
    }
}

impl StreamOps for CpioFileObject {
    fn read(&self, buf: &mut [u8]) -> Result<usize, StreamError> {
        let cpio_node = self.node.as_any()
            .downcast_ref::<CpioNode>()
            .ok_or(StreamError::IoError)?;
        
        let mut pos = self.position.write();
        let start = *pos as usize;
        let end = (start + buf.len()).min(cpio_node.content.len());
        
        if start >= cpio_node.content.len() {
            return Ok(0); // EOF
        }
        
        let bytes_to_read = end - start;
        buf[..bytes_to_read].copy_from_slice(&cpio_node.content[start..end]);
        *pos += bytes_to_read as u64;
        
        Ok(bytes_to_read)
    }
    
    fn write(&self, _buf: &[u8]) -> Result<usize, StreamError> {
        Err(StreamError::PermissionDenied)
    }
}

impl FileObject for CpioFileObject {
    fn seek(&self, whence: crate::fs::SeekFrom) -> Result<u64, StreamError> {
        let cpio_node = self.node.as_any()
            .downcast_ref::<CpioNode>()
            .ok_or(StreamError::IoError)?;
        
        let mut pos = self.position.write();
        let file_size = cpio_node.content.len() as u64;
        
        let new_pos = match whence {
            crate::fs::SeekFrom::Start(offset) => offset,
            crate::fs::SeekFrom::Current(offset) => {
                if offset >= 0 {
                    *pos + offset as u64
                } else {
                    pos.saturating_sub((-offset) as u64)
                }
            },
            crate::fs::SeekFrom::End(offset) => {
                if offset >= 0 {
                    file_size + offset as u64
                } else {
                    file_size.saturating_sub((-offset) as u64)
                }
            }
        };
        
        *pos = new_pos.min(file_size);
        Ok(*pos)
    }
    
    fn metadata(&self) -> Result<crate::fs::FileMetadata, StreamError> {
        self.node.metadata().map_err(StreamError::from)
    }
    
    fn truncate(&self, _size: u64) -> Result<(), StreamError> {
        Err(StreamError::PermissionDenied)
    }
}

/// Directory object for CPIO directories
pub struct CpioDirectoryObject {
    node: Arc<dyn VfsNode>,
    position: RwLock<u64>,
}

impl CpioDirectoryObject {
    pub fn new(node: Arc<dyn VfsNode>) -> Self {
        Self {
            node,
            position: RwLock::new(0),
        }
    }
}

impl StreamOps for CpioDirectoryObject {
    fn read(&self, _buf: &mut [u8]) -> Result<usize, StreamError> {
        // Directory reading not implemented for simplified version
        Err(StreamError::NotSupported)
    }
    
    fn write(&self, _buf: &[u8]) -> Result<usize, StreamError> {
        Err(StreamError::FileSystemError(FileSystemError::new(
            FileSystemErrorKind::ReadOnly,
            "CPIO filesystem is read-only"
        )))
    }
}

impl FileObject for CpioDirectoryObject {
    fn seek(&self, _whence: crate::fs::SeekFrom) -> Result<u64, StreamError> {
        // Seeking in directories not supported
        Err(StreamError::NotSupported)
    }
    
    fn metadata(&self) -> Result<crate::fs::FileMetadata, StreamError> {
        self.node.metadata().map_err(StreamError::from)
    }
    
    fn truncate(&self, _size: u64) -> Result<(), StreamError> {
        Err(StreamError::FileSystemError(FileSystemError::new(
            FileSystemErrorKind::ReadOnly,
            "CPIO filesystem is read-only"
        )))
    }
}
