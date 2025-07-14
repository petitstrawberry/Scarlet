//! CpioFS v2 - CPIO filesystem implementation for initramfs
//!
//! This is a simplified read-only filesystem for handling CPIO archives
//! used as initramfs. It implements the VFS v2 architecture.

use alloc::{
    boxed::Box, collections::BTreeMap, format, string::{String, ToString}, sync::Arc, vec::Vec
};
use spin::RwLock;
use core::any::Any;
use alloc::sync::Weak;

use crate::{driver_initcall, fs::{core::DirectoryEntryInternal, get_fs_driver_manager, vfs_v2::core::{
    FileSystemOperations, VfsNode
}, FileSystemDriver, FileSystemType}, vm::vmem::MemoryArea};
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

    /// Parent node (weak reference)
    parent: RwLock<Option<Weak<CpioNode>>>,
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
            parent: RwLock::new(None),
        })
    }
    
    /// Add a child to this directory node
    pub fn add_child(self: &Arc<Self>, name: String, child: Arc<CpioNode>) -> Result<(), FileSystemError> {
        if self.file_type != FileType::Directory {
            return Err(FileSystemError::new(
                FileSystemErrorKind::NotADirectory,
                "Cannot add child to non-directory node"
            ));
        }
        // Set parent pointer
        *child.parent.write() = Some(Arc::downgrade(self));
        let mut children = self.children.write();
        children.insert(name, child);
        Ok(())
    }
    
    /// Get a child by name
    pub fn get_child(&self, name: &str) -> Option<Arc<CpioNode>> {
        let children = self.children.read();
        children.get(name).cloned()
    }

    pub fn parent_file_id(&self) -> Option<u64> {
        self.parent.read().as_ref()?.upgrade().map(|p| p.file_id as u64)
    }

    /// Helper to convert from Arc<dyn VfsNode> to Arc<CpioNode>
    pub fn from_vfsnode_arc(node: &Arc<dyn VfsNode>) -> Option<Arc<CpioNode>> {
        match Arc::downcast::<CpioNode>(node.clone()) {
            Ok(cpio_node) => Some(cpio_node),
            Err(_) => None,
        }
    }
}

impl VfsNode for CpioNode {
    fn id(&self) -> u64 {
        self.file_id as u64
    }

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
    
    fn read_link(&self) -> Result<String, FileSystemError> {
        // Check if this is actually a symbolic link
        if self.file_type != FileType::SymbolicLink {
            return Err(FileSystemError::new(
                FileSystemErrorKind::NotSupported,
                "Not a symbolic link"
            ));
        }
        
        // Read the target path from content (stored as UTF-8 bytes)
        String::from_utf8(self.content.clone()).map_err(|_| {
            FileSystemError::new(
                FileSystemErrorKind::InvalidData,
                "Invalid UTF-8 in symbolic link target"
            )
        })
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
        {
            let mut fs_guard = root_node.filesystem.write();
            *fs_guard = Some(Arc::clone(&filesystem));
        }
        // Parse CPIO data and build directory tree
        filesystem.parse_cpio_archive(cpio_data)?;
        Ok(filesystem)
    }

    /// VFS v2 driver registration API: create from option string
    /// Example: option = Some("initramfs_addr=0x80000000,size=65536")
    pub fn create_from_option_string(option: Option<&str>, cpio_data: &[u8]) -> Arc<dyn FileSystemOperations> {
        // Name is fixed, cpio_data is assumed to be provided externally
        let name = "cpiofs".to_string();
        // Extend option parsing as needed
        CpioFS::new(name, cpio_data).expect("Failed to create CpioFS") as Arc<dyn FileSystemOperations>
    }
    
    /// Parse CPIO archive and build directory tree
    fn parse_cpio_archive(self: &Arc<Self>, data: &[u8]) -> Result<(), FileSystemError> {
        // CPIO new ASCII format: magic "070701"
        let mut offset = 0;
        let mut file_id = 2;
        while offset + 110 <= data.len() {
            // Parse header
            let magic = &data[offset..offset+6];
            if magic != b"070701" {
                break;
            }
            let inode = match core::str::from_utf8(&data[offset+6..offset+14]) {
                Ok(s) => u32::from_str_radix(s, 16).map_err(|_| FileSystemError::new(FileSystemErrorKind::InvalidData, "Invalid inode value"))?,
                Err(_) => return Err(FileSystemError::new(FileSystemErrorKind::InvalidData, "Invalid UTF-8 in inode field")),
            };
            let mode = match core::str::from_utf8(&data[offset+14..offset+22]) {
                Ok(s) => u32::from_str_radix(s, 16).map_err(|_| FileSystemError::new(FileSystemErrorKind::InvalidData, "Invalid mode value"))?,
                Err(_) => return Err(FileSystemError::new(FileSystemErrorKind::InvalidData, "Invalid UTF-8 in mode field")),
            };
            let namesize = match core::str::from_utf8(&data[offset+94..offset+102]) {
                Ok(s) => usize::from_str_radix(s, 16).map_err(|_| FileSystemError::new(FileSystemErrorKind::InvalidData, "Invalid namesize value"))?,
                Err(_) => return Err(FileSystemError::new(FileSystemErrorKind::InvalidData, "Invalid UTF-8 in namesize field")),
            };
            let filesize = match core::str::from_utf8(&data[offset+54..offset+62]) {
                Ok(s) => usize::from_str_radix(s, 16).map_err(|_| FileSystemError::new(FileSystemErrorKind::InvalidData, "Invalid filesize value"))?,
                Err(_) => return Err(FileSystemError::new(FileSystemErrorKind::InvalidData, "Invalid UTF-8 in filesize field")),
            };
            let name_start = offset + 110;
            let name_end = name_start + namesize;
            if name_end > data.len() { break; }
            let name = &data[name_start..name_end-1]; // remove trailing NUL
            let name_str = core::str::from_utf8(name).unwrap_or("").to_string();
            let file_start = (name_end + 3) & !3; // 4-byte align
            let file_end = file_start + filesize;
            if file_end > data.len() { break; }
            if name_str == "TRAILER!!!" { break; }
            // Determine file type
            let file_type = match mode & 0o170000 {
                0o040000 => FileType::Directory,
                0o100000 => FileType::RegularFile,
                0o120000 => FileType::SymbolicLink,
                _ => FileType::RegularFile,
            };
            let content = if file_type == FileType::RegularFile || file_type == FileType::SymbolicLink {
                data[file_start..file_end].to_vec()
            } else {
                Vec::new()
            };
            // Build node and insert into tree
            let base_name = if let Some(pos) = name_str.rfind('/') {
                &name_str[pos+1..]
            } else {
                &name_str[..]
            };
            
            // Skip "." and ".." entries as they are handled automatically by the VFS
            if base_name == "." || base_name == ".." {
                offset = (file_end + 3) & !3;
                continue;
            }
            
            let node = CpioNode::new(base_name.to_string(), file_type, content, file_id);
            {
                let mut fs_guard = node.filesystem.write();
                *fs_guard = Some(Arc::clone(self));
            }
            file_id += 1;
            // Insert into parent
            let parent_path = if let Some(pos) = name_str.rfind('/') { &name_str[..pos] } else { "" };
            let parent = if parent_path.is_empty() {
                Arc::clone(&self.root_node)
            } else {
                // Traverse from root to find parent
                let mut cur = Arc::clone(&self.root_node);
                for part in parent_path.split('/') {
                    if part.is_empty() { continue; }
                    if let Some(child) = cur.get_child(part) {
                        cur = child;
                    } else {
                        // Create intermediate directory if missing
                        let dir = CpioNode::new(part.to_string(), FileType::Directory, Vec::new(), file_id);
                        {
                            let mut fs_guard = dir.filesystem.write();
                            *fs_guard = Some(Arc::clone(self));
                        }
                        file_id += 1;
                        cur.add_child(part.to_string(), Arc::clone(&dir)).ok();
                        cur = dir;
                    }
                }
                cur
            };
            parent.add_child(base_name.to_string(), Arc::clone(&node)).ok();
            offset = (file_end + 3) & !3;
        }
        Ok(())
    }
}

impl FileSystemOperations for CpioFS {
    fn lookup(
        &self,
        parent_node: &Arc<dyn VfsNode>,
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
            .map(|n| n as Arc<dyn VfsNode>)
            .ok_or_else(|| FileSystemError::new(
                FileSystemErrorKind::NotFound,
                format!("File not found: {} in {}", name, cpio_parent.name)
            ))
    }
    
    fn open(
        &self,
        node: &Arc<dyn VfsNode>,
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
                Ok(Arc::new(CpioFileObject::new(Arc::clone(node))))
            },
            FileType::Directory => {
                Ok(Arc::new(CpioDirectoryObject::new(Arc::clone(node))))
            },
            _ => Err(FileSystemError::new(
                FileSystemErrorKind::NotSupported,
                "Unsupported file type"
            ))
        }
    }
    
    fn create(
        &self,
        _parent_node: &Arc<dyn VfsNode>,
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
        _parent_node: &Arc<dyn VfsNode>,
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
        node: &Arc<dyn VfsNode>,
    ) -> Result<Vec<DirectoryEntryInternal>, FileSystemError> {
        let cpio_node = node.as_any()
            .downcast_ref::<CpioNode>()
            .ok_or_else(|| FileSystemError::new(
                FileSystemErrorKind::NotSupported,
                "Invalid node type for CpioFS"
            ))?;
        if cpio_node.file_type != FileType::Directory {
            return Err(FileSystemError::new(
                FileSystemErrorKind::NotADirectory,
                "Not a directory"
            ));
        }
        let mut entries = Vec::new();
        // Add "." and ".." entries
        entries.push(DirectoryEntryInternal {
            name: ".".to_string(),
            file_type: FileType::Directory,
            file_id: cpio_node.file_id as u64,
        });
        // .. entry should have the parent directory's file_id
        let parent_file_id = cpio_node.parent_file_id().unwrap_or(0);
        entries.push(DirectoryEntryInternal {
            name: "..".to_string(),
            file_type: FileType::Directory,
            file_id: parent_file_id,
        });
        // Add children
        for child in cpio_node.children.read().values() {
            entries.push(DirectoryEntryInternal {
                name: child.name.clone(),
                file_type: child.file_type,
                file_id: child.file_id as u64,
            });
        }
        Ok(entries)
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
    fn read(&self, buf: &mut [u8]) -> Result<usize, StreamError> {
        let cpio_node = self.node.as_any()
            .downcast_ref::<CpioNode>()
            .ok_or(StreamError::NotSupported)?;
        if cpio_node.file_type != FileType::Directory {
            return Err(StreamError::NotSupported);
        }
        let mut all_entries = Vec::new();
        // . entry
        all_entries.push(crate::fs::DirectoryEntryInternal {
            name: ".".to_string(),
            file_type: FileType::Directory,
            size: 0,
            file_id: cpio_node.file_id as u64,
            metadata: None,
        });
        // .. entry
        let parent_file_id = cpio_node.parent_file_id().unwrap_or(0);
        all_entries.push(crate::fs::DirectoryEntryInternal {
            name: "..".to_string(),
            file_type: FileType::Directory,
            size: 0,
            file_id: parent_file_id,
            metadata: None,
        });
        // children entries
        for child in cpio_node.children.read().values() {
            all_entries.push(crate::fs::DirectoryEntryInternal {
                name: child.name.clone(),
                file_type: child.file_type,
                size: child.content.len(),
                file_id: child.file_id as u64,
                metadata: None,
            });
        }
        
        let position = *self.position.read() as usize;
        if position >= all_entries.len() {
            return Ok(0); // EOF
        }
        let internal_entry = &all_entries[position];
        let dir_entry = crate::fs::DirectoryEntry::from_internal(internal_entry);
        let entry_size = dir_entry.entry_size();
        if buf.len() < entry_size {
            return Err(StreamError::InvalidArgument); 
        }
        let entry_bytes = unsafe {
            core::slice::from_raw_parts(
                &dir_entry as *const _ as *const u8,
                entry_size
            )
        };
        buf[..entry_size].copy_from_slice(entry_bytes);
        *self.position.write() += 1;
        Ok(entry_size)
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


/// Driver for CPIO-format filesystems (initramfs)
/// 
/// This driver creates filesystems from memory areas only.
pub struct CpiofsDriver;

impl FileSystemDriver for CpiofsDriver {
    fn name(&self) -> &'static str {
        "cpiofs"
    }
    
    /// This filesystem only supports creation from memory
    fn filesystem_type(&self) -> FileSystemType {
        FileSystemType::Memory
    }
    
    /// Create a file system from memory area
    /// 
    /// # Arguments
    /// 
    /// * `memory_area` - A reference to the memory area containing the CPIO filesystem data
    /// 
    /// # Returns
    /// 
    /// A result containing a boxed CPIO filesystem or an error
    /// 
    fn create_from_memory(&self, memory_area: &MemoryArea) -> Result<Arc<dyn FileSystemOperations>, FileSystemError> {
        let data = unsafe { memory_area.as_slice() };
        // Create the Cpiofs from the memory data
        match CpioFS::new("cpiofs".to_string(), data) {
            Ok(cpio_fs) => Ok(cpio_fs),
            Err(err) => Err(FileSystemError {
                kind: FileSystemErrorKind::InvalidData,
                message: format!("Failed to create CPIO filesystem from memory: {}", err.message),
            })
        }
    }

    fn create_from_params(&self, params: &dyn crate::fs::params::FileSystemParams) -> Result<Arc<dyn FileSystemOperations>, FileSystemError> {
        use crate::fs::params::*;
        
        // Try to downcast to CpioFSParams
        if let Some(_cpio_params) = params.as_any().downcast_ref::<CpioFSParams>() {
            // CPIO filesystem requires memory area for creation, so we cannot create from parameters alone
            return Err(FileSystemError {
                kind: FileSystemErrorKind::NotSupported,
                message: "CPIO filesystem requires memory area for creation. Use create_from_memory instead.".to_string(),
            });
        }
        
        // Try to downcast to BasicFSParams for compatibility
        if let Some(_basic_params) = params.as_any().downcast_ref::<BasicFSParams>() {
            return Err(FileSystemError {
                kind: FileSystemErrorKind::NotSupported,
                message: "CPIO filesystem requires memory area for creation. Use create_from_memory instead.".to_string(),
            });
        }
        
        // If all downcasts fail, return error
        Err(FileSystemError {
            kind: FileSystemErrorKind::NotSupported,
            message: "CPIO filesystem requires CpioFSParams and memory area for creation".to_string(),
        })
    }
}

fn register_driver() {
    let fs_driver_manager = get_fs_driver_manager();
    fs_driver_manager.register_driver(Box::new(CpiofsDriver));
}

driver_initcall!(register_driver);

#[cfg(test)]
#[path = "cpiofs_tests.rs"]
mod tests;