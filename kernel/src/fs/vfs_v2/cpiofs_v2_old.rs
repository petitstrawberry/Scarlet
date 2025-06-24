//! CpioFS v2 - CPIO filesystem implementation for VFS v2
//!
//! VFS v2のVfsNodeアーキテクチャに基づいたCPIO（Copy In/Out）ファイルシステム実装。
//! initramfsなどの読み取り専用アーカイブファイルシステムとして使用される。
//!
//! # 特徴
//! - VfsNodeベースの新しいアーキテクチャ
//! - 読み取り専用ファイルシステム
//! - "070701" (new ASCII) 形式のCPIOアーカイブをサポート
//! - 効率的なメモリ使用（共有データ参照）
//! - FileSystemOperationsトレイト準拠

use alloc::{boxed::Box, format, string::{String, ToString}, sync::Arc, vec::Vec};
use spin::{Mutex, RwLock};
use core::fmt;

use crate::fs::vfs_v2::core::{
    VfsNode, FileSystemOperations, FileSystemError, FileSystemRef
};
use crate::fs::{
    FileMetadata, FilePermission, FileObject, FileSystemErrorKind, FileType, SeekFrom
};
use crate::object::capability::{StreamOps, StreamError};

/// CPIO archive header (new ASCII format)
#[repr(C)]
#[derive(Debug, Clone)]
struct CpioHeader {
    magic: [u8; 6],        // "070701"
    ino: [u8; 8],          // inode number
    mode: [u8; 8],         // file mode
    uid: [u8; 8],          // user ID
    gid: [u8; 8],          // group ID  
    nlink: [u8; 8],        // number of links
    mtime: [u8; 8],        // modification time
    filesize: [u8; 8],     // file size
    devmajor: [u8; 8],     // device major
    devminor: [u8; 8],     // device minor
    rdevmajor: [u8; 8],    // rdev major
    rdevminor: [u8; 8],    // rdev minor
    namesize: [u8; 8],     // name size
    check: [u8; 8],        // checksum
}

impl CpioHeader {
    const HEADER_SIZE: usize = 110;
    const MAGIC_NEW_ASCII: &'static [u8] = b"070701";
    
    /// Parse a CPIO header from bytes
    fn parse(data: &[u8]) -> Result<Self, FileSystemError> {
        if data.len() < Self::HEADER_SIZE {
            return Err(FileSystemError {
                kind: FileSystemErrorKind::InvalidData,
                message: "CPIO header too short".to_string(),
            });
        }
        
        let header = unsafe { 
            core::ptr::read_unaligned(data.as_ptr() as *const CpioHeader)
        };
        
        if &header.magic != Self::MAGIC_NEW_ASCII {
            return Err(FileSystemError {
                kind: FileSystemErrorKind::InvalidData,
                message: format!("Invalid CPIO magic: {:?}", header.magic),
            });
        }
        
        Ok(header)
    }
    
    /// Get numeric value from hex string field
    fn get_hex_value(field: &[u8]) -> Result<usize, FileSystemError> {
        let hex_str = core::str::from_utf8(field)
            .map_err(|_| FileSystemError {
                kind: FileSystemErrorKind::InvalidData,
                message: "Invalid UTF-8 in CPIO header".to_string(),
            })?;
            
        usize::from_str_radix(hex_str, 16)
            .map_err(|_| FileSystemError {
                kind: FileSystemErrorKind::InvalidData,
                message: format!("Invalid hex value: {}", hex_str),
            })
    }
    
    /// Get file mode
    fn file_mode(&self) -> Result<u32, FileSystemError> {
        Ok(Self::get_hex_value(&self.mode)? as u32)
    }
    
    /// Get file size
    fn file_size(&self) -> Result<usize, FileSystemError> {
        Self::get_hex_value(&self.filesize)
    }
    
    /// Get name size
    fn name_size(&self) -> Result<usize, FileSystemError> {
        Self::get_hex_value(&self.namesize)
    }
    
    /// Get modification time
    fn mtime(&self) -> Result<u64, FileSystemError> {
        Ok(Self::get_hex_value(&self.mtime)? as u64)
    }
    
    /// Get inode number
    fn inode(&self) -> Result<u64, FileSystemError> {
        Ok(Self::get_hex_value(&self.ino)? as u64)
    }
}

/// Parsed CPIO entry
#[derive(Debug, Clone)]
pub struct CpioEntry {
    pub name: String,
    pub node_type: VfsNodeType,
    pub size: usize,
    pub mtime: u64,
    pub inode: u64,
    pub data_offset: usize,
}

/// Shared CPIO archive data
#[derive(Debug)]
pub struct SharedCpioData {
    raw_data: *const u8,
    data_size: usize,
}

unsafe impl Send for SharedCpioData {}
unsafe impl Sync for SharedCpioData {}

impl SharedCpioData {
    /// Create new shared CPIO data
    pub fn new(data: &[u8]) -> Self {
        Self {
            raw_data: data.as_ptr(),
            data_size: data.len(),
        }
    }
    
    /// Get data slice
    pub fn as_slice(&self) -> &[u8] {
        unsafe {
            core::slice::from_raw_parts(self.raw_data, self.data_size)
        }
    }
}

/// CPIO VfsNode implementation
pub struct CpioVfsNode {
    entry: CpioEntry,
    shared_data: Arc<SharedCpioData>,
    children: RwLock<Vec<Arc<dyn VfsNode>>>,
}

impl CpioVfsNode {
    /// Create new CPIO VfsNode
    pub fn new(entry: CpioEntry, shared_data: Arc<SharedCpioData>) -> Self {
        Self {
            entry,
            shared_data,
            children: RwLock::new(Vec::new()),
        }
    }
    
    /// Add child node
    pub fn add_child(&self, child: Arc<dyn VfsNode>) {
        self.children.write().push(child);
    }
}

impl VfsNode for CpioVfsNode {
    fn node_type(&self) -> VfsNodeType {
        self.entry.node_type
    }
    
    fn metadata(&self) -> Result<FileMetadata, FileSystemError> {
        Ok(FileMetadata {
            size: self.entry.size,
            permissions: FilePermission {
                read: true,
                write: false,  // CPIO is read-only
                execute: false,
            },
            created_time: self.entry.mtime,
            modified_time: self.entry.mtime,
            accessed_time: self.entry.mtime,
            inode: self.entry.inode,
            link_count: 1,
        })
    }
    
    fn open(&self) -> Result<Box<dyn FileObject>, FileSystemError> {
        match self.entry.node_type {
            VfsNodeType::RegularFile => {
                Ok(Box::new(CpioFileObject {
                    shared_data: Arc::clone(&self.shared_data),
                    data_offset: self.entry.data_offset,
                    data_size: self.entry.size,
                    position: RwLock::new(0),
                    node_type: VfsNodeType::RegularFile,
                    directory_entries: None,
                }))
            },
            VfsNodeType::Directory => {
                let children = self.children.read();
                let mut entries = Vec::new();
                
                // Add . and .. entries
                entries.push((".".to_string(), VfsNodeType::Directory));
                entries.push(("..".to_string(), VfsNodeType::Directory));
                
                // Add child entries
                for child in children.iter() {
                    if let Some(cpio_node) = child.as_any().downcast_ref::<CpioVfsNode>() {
                        let filename = cpio_node.entry.name.split('/').last().unwrap_or("");
                        if !filename.is_empty() {
                            entries.push((filename.to_string(), cpio_node.entry.node_type));
                        }
                    }
                }
                
                Ok(Box::new(CpioFileObject {
                    shared_data: Arc::clone(&self.shared_data),
                    data_offset: 0,
                    data_size: entries.len(),
                    position: RwLock::new(0),
                    node_type: VfsNodeType::Directory,
                    directory_entries: Some(entries),
                }))
            },
            _ => Err(FileSystemError {
                kind: FileSystemErrorKind::NotSupported,
                message: "Unsupported node type for CPIO".to_string(),
            }),
        }
    }
    
    fn lookup(&self, name: &str) -> Result<Option<Arc<dyn VfsNode>>, FileSystemError> {
        if self.entry.node_type != VfsNodeType::Directory {
            return Err(FileSystemError {
                kind: FileSystemErrorKind::NotADirectory,
                message: "Not a directory".to_string(),
            });
        }
        
        // Special cases for . and ..
        if name == "." {
            return Ok(Some(Arc::new(CpioVfsNode::new(
                self.entry.clone(),
                Arc::clone(&self.shared_data)
            )) as Arc<dyn VfsNode>));
        }
        
        if name == ".." {
            // For now, return None (parent lookup should be handled by VfsManager)
            return Ok(None);
        }
        
        let children = self.children.read();
        for child in children.iter() {
            let child_entry = match child.as_any().downcast_ref::<CpioVfsNode>() {
                Some(cpio_node) => &cpio_node.entry,
                None => continue,
            };
            
            // Extract filename from full path
            let filename = child_entry.name.split('/').last().unwrap_or("");
            if filename == name {
                return Ok(Some(Arc::clone(child)));
            }
        }
        
        Ok(None)
    }
    
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
}

impl fmt::Debug for CpioVfsNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CpioVfsNode")
            .field("name", &self.entry.name)
            .field("node_type", &self.entry.node_type)
            .field("size", &self.entry.size)
            .finish()
    }
}

/// CPIO FileObject implementation
pub struct CpioFileObject {
    shared_data: Arc<SharedCpioData>,
    data_offset: usize,
    data_size: usize,
    position: RwLock<usize>,
    node_type: VfsNodeType,
    directory_entries: Option<Vec<(String, VfsNodeType)>>,
}

impl FileObject for CpioFileObject {
    fn metadata(&self) -> Result<FileMetadata, FileSystemError> {
        Ok(FileMetadata {
            size: self.data_size,
            permissions: FilePermission {
                read: true,
                write: false,
                execute: false,
            },
            created_time: 0,
            modified_time: 0,
            accessed_time: 0,
            inode: 0,
            link_count: 1,
        })
    }
}

impl StreamOps for CpioFileObject {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        match self.node_type {
            VfsNodeType::RegularFile => {
                let mut pos = self.position.write();
                let available = self.data_size.saturating_sub(*pos);
                let to_read = buffer.len().min(available);
                
                if to_read > 0 {
                    let start = self.data_offset + *pos;
                    let data = self.shared_data.as_slice();
                    buffer[..to_read].copy_from_slice(&data[start..start + to_read]);
                    *pos += to_read;
                }
                
                Ok(to_read)
            },
            VfsNodeType::Directory => {
                if let Some(ref entries) = self.directory_entries {
                    let mut pos = self.position.write();
                    
                    if *pos >= entries.len() {
                        return Ok(0); // EOF
                    }
                    
                    let (name, node_type) = &entries[*pos];
                    
                    // Simple directory entry format: name + type
                    let entry_str = format!("{}\t{:?}\n", name, node_type);
                    let entry_bytes = entry_str.as_bytes();
                    
                    if buffer.len() < entry_bytes.len() {
                        return Err(StreamError::InvalidArgument);
                    }
                    
                    buffer[..entry_bytes.len()].copy_from_slice(entry_bytes);
                    *pos += 1;
                    
                    Ok(entry_bytes.len())
                } else {
                    Err(StreamError::InvalidArgument)
                }
            },
            _ => Err(StreamError::NotSupported),
        }
    }
    
    fn write(&self, _buffer: &[u8]) -> Result<usize, StreamError> {
        Err(StreamError::ReadOnlyFileSystem)
    }
    
    fn seek(&self, whence: SeekFrom) -> Result<u64, StreamError> {
        let mut pos = self.position.write();
        match whence {
            SeekFrom::Start(offset) => {
                *pos = offset as usize;
            },
            SeekFrom::Current(offset) => {
                if offset >= 0 {
                    *pos = (*pos).saturating_add(offset as usize);
                } else {
                    *pos = (*pos).saturating_sub((-offset) as usize);
                }
            },
            SeekFrom::End(offset) => {
                if offset >= 0 {
                    *pos = self.data_size.saturating_add(offset as usize);
                } else {
                    *pos = self.data_size.saturating_sub((-offset) as usize);
                }
            },
        }
        Ok(*pos as u64)
    }
}

/// CpioFS v2 - FileSystemOperations implementation
pub struct CpioFSv2 {
    name: String,
    shared_data: Arc<SharedCpioData>,
    root_node: Arc<CpioVfsNode>,
    entries: Vec<CpioEntry>,
}

impl CpioFSv2 {
    /// Create new CpioFS v2 from CPIO archive data
    pub fn new(name: String, data: &[u8]) -> Result<Self, FileSystemError> {
        let shared_data = Arc::new(SharedCpioData::new(data));
        let entries = Self::parse_cpio_archive(data)?;
        let root_node = Self::build_node_tree(&entries, Arc::clone(&shared_data))?;
        
        Ok(Self {
            name,
            shared_data,
            root_node,
            entries,
        })
    }
    
    /// Parse CPIO archive and extract entries
    fn parse_cpio_archive(data: &[u8]) -> Result<Vec<CpioEntry>, FileSystemError> {
        let mut entries = Vec::new();
        let mut offset = 0;
        
        while offset < data.len() {
            if offset + CpioHeader::HEADER_SIZE > data.len() {
                break;
            }
            
            let header = CpioHeader::parse(&data[offset..])?;
            let name_size = header.name_size()?;
            let file_size = header.file_size()?;
            let file_mode = header.file_mode()?;
            
            // Get filename
            let name_offset = offset + CpioHeader::HEADER_SIZE;
            let name_end = name_offset + name_size - 1; // exclude null terminator
            
            if name_end > data.len() {
                break;
            }
            
            let name = core::str::from_utf8(&data[name_offset..name_end])
                .map_err(|_| FileSystemError {
                    kind: FileSystemErrorKind::InvalidData,
                    message: "Invalid UTF-8 in filename".to_string(),
                })?
                .to_string();
            
            // Check for trailer
            if name == "TRAILER!!!" {
                break;
            }
            
            // Determine node type from mode
            let node_type = match file_mode & 0o170000 {
                0o040000 => VfsNodeType::Directory,
                0o100000 => VfsNodeType::RegularFile,
                _ => VfsNodeType::Unknown,
            };
            
            // Calculate data offset (4-byte aligned)
            let data_offset = (name_offset + name_size + 3) & !3;
            
            entries.push(CpioEntry {
                name,
                node_type,
                size: file_size,
                mtime: header.mtime()?,
                inode: header.inode()?,
                data_offset,
            });
            
            // Move to next entry (4-byte aligned)
            offset = (data_offset + file_size + 3) & !3;
        }
        
        Ok(entries)
    }
    
    /// Build VfsNode tree from entries
    fn build_node_tree(entries: &[CpioEntry], shared_data: Arc<SharedCpioData>) -> Result<Arc<CpioVfsNode>, FileSystemError> {
        use alloc::collections::BTreeMap;
        
        // Create root node
        let root_entry = CpioEntry {
            name: "".to_string(),
            node_type: VfsNodeType::Directory,
            size: 0,
            mtime: 0,
            inode: 0,
            data_offset: 0,
        };
        
        let root_node = Arc::new(CpioVfsNode::new(root_entry, Arc::clone(&shared_data)));
        
        // Create a map to store all nodes by path
        let mut node_map: BTreeMap<String, Arc<CpioVfsNode>> = BTreeMap::new();
        node_map.insert("".to_string(), Arc::clone(&root_node));
        
        // First pass: create all nodes
        for entry in entries {
            let node = Arc::new(CpioVfsNode::new(entry.clone(), Arc::clone(&shared_data)));
            node_map.insert(entry.name.clone(), node);
        }
        
        // Second pass: build directory relationships
        for entry in entries {
            if let Some(last_slash) = entry.name.rfind('/') {
                let parent_path = if last_slash == 0 {
                    "".to_string()  // Root directory
                } else {
                    entry.name[..last_slash].to_string()
                };
                
                if let (Some(parent_node), Some(child_node)) = 
                    (node_map.get(&parent_path), node_map.get(&entry.name)) {
                    parent_node.add_child(Arc::clone(child_node));
                }
            } else {
                // File in root directory
                if let Some(child_node) = node_map.get(&entry.name) {
                    root_node.add_child(Arc::clone(child_node));
                }
            }
        }
        
        Ok(root_node)
    }
}

impl FileSystemOperations for CpioFSv2 {
    fn name(&self) -> &str {
        &self.name
    }
    
    fn root(&self) -> Arc<dyn VfsNode> {
        Arc::clone(&self.root_node) as Arc<dyn VfsNode>
    }
    
    fn create_file(&self, _parent: &dyn VfsNode, _name: &str) -> Result<Arc<dyn VfsNode>, FileSystemError> {
        Err(FileSystemError {
            kind: FileSystemErrorKind::ReadOnlyFileSystem,
            message: "CPIO filesystem is read-only".to_string(),
        })
    }
    
    fn create_directory(&self, _parent: &dyn VfsNode, _name: &str) -> Result<Arc<dyn VfsNode>, FileSystemError> {
        Err(FileSystemError {
            kind: FileSystemErrorKind::ReadOnlyFileSystem,
            message: "CPIO filesystem is read-only".to_string(),
        })
    }
    
    fn remove(&self, _parent: &dyn VfsNode, _name: &str) -> Result<(), FileSystemError> {
        Err(FileSystemError {
            kind: FileSystemErrorKind::ReadOnlyFileSystem,
            message: "CPIO filesystem is read-only".to_string(),
        })
    }
    
    fn rename(&self, _old_parent: &dyn VfsNode, _old_name: &str, _new_parent: &dyn VfsNode, _new_name: &str) -> Result<(), FileSystemError> {
        Err(FileSystemError {
            kind: FileSystemErrorKind::ReadOnlyFileSystem,
            message: "CPIO filesystem is read-only".to_string(),
        })
    }
}

impl fmt::Debug for CpioFSv2 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CpioFSv2")
            .field("name", &self.name)
            .field("entries_count", &self.entries.len())
            .finish()
    }
}
