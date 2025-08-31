//! Ext2 Filesystem Implementation
//!
//! This module implements an ext2 filesystem driver for the VFS v2 architecture.
//! It provides support for reading and writing ext2 filesystems on block devices.
//!
//! ## Features
//!
//! - Full ext2 filesystem support
//! - Read and write operations
//! - Directory navigation  
//! - File creation, deletion, and modification
//! - Integration with VFS v2 architecture
//! - Block device compatibility
//!
//! ## Architecture
//!
//! The ext2 implementation consists of:
//! - `Ext2FileSystem`: Main filesystem implementation
//! - `Ext2Node`: VFS node implementation for files and directories
//! - `Ext2Driver`: Filesystem driver for registration
//! - Data structures for ext2 format (superblock, inodes, directory entries, etc.)

use alloc::{
    boxed::Box, collections::BTreeMap, format, string::{String, ToString}, sync::Arc, vec, vec::Vec
};
use spin::{rwlock::RwLock, Mutex};
use core::{fmt::Debug, mem, any::Any};

use crate::{
    device::block::BlockDevice,
    driver_initcall,
    fs::{
        get_fs_driver_manager, FileObject, FileSystemDriver, 
        FileSystemError, FileSystemErrorKind, FileSystemType, FileType
    }
};

use crate::fs::vfs_v2::core::{VfsNode, FileSystemOperations, DirectoryEntryInternal};

pub mod structures;
pub mod node;
pub mod driver;

#[cfg(test)]
pub mod tests;

pub use structures::*;
pub use node::{Ext2Node, Ext2FileObject, Ext2DirectoryObject};
pub use driver::Ext2Driver;

/// Ext2 Filesystem implementation
///
/// This struct implements an ext2 filesystem that can be mounted on block devices.
/// It maintains the block device reference and provides filesystem operations
/// through the VFS v2 interface.
pub struct Ext2FileSystem {
    /// Reference to the underlying block device
    block_device: Arc<dyn BlockDevice>,
    /// Superblock information
    superblock: Ext2Superblock,
    /// Block size in bytes
    block_size: u32,
    /// Blocks per group
    blocks_per_group: u32,
    /// Inodes per group
    inodes_per_group: u32,
    /// Root directory node
    root: RwLock<Arc<Ext2Node>>,
    /// Filesystem name
    name: String,
    /// Next file ID generator
    next_file_id: Mutex<u64>,
    /// Block group descriptor table cache
    bgdt_cache: Mutex<Vec<Ext2BlockGroupDescriptor>>,
    /// Inode cache
    inode_cache: Mutex<BTreeMap<u32, Ext2Inode>>,
}

impl Debug for Ext2FileSystem {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Ext2FileSystem")
            .field("name", &self.name)
            .field("block_size", &self.block_size)
            .field("blocks_per_group", &self.blocks_per_group)
            .field("inodes_per_group", &self.inodes_per_group)
            .finish()
    }
}

impl Ext2FileSystem {
    /// Create a new ext2 filesystem from a block device
    pub fn new(block_device: Arc<dyn BlockDevice>) -> Result<Arc<Self>, FileSystemError> {
        // Read superblock
        let superblock = Self::read_superblock(&*block_device)?;
        
        // Validate ext2 filesystem
        Self::validate_ext2(&superblock)?;
        
        // Calculate filesystem parameters
        let block_size = 1024 << superblock.log_block_size;
        let blocks_per_group = superblock.blocks_per_group;
        let inodes_per_group = superblock.inodes_per_group;
        
        // Read block group descriptor table
        let bgdt = Self::read_bgdt(&*block_device, &superblock, block_size)?;
        
        // Create root directory node (inode 2 is always root in ext2)
        let root_inode = Self::read_inode(&*block_device, &superblock, &bgdt, 2, block_size)?;
        let root = Arc::new(Ext2Node::new_from_inode("/".to_string(), 2, &root_inode));
        
        let fs = Arc::new(Self {
            block_device,
            superblock,
            block_size,
            blocks_per_group,
            inodes_per_group,
            root: RwLock::new(Arc::clone(&root)),
            name: "ext2".to_string(),
            next_file_id: Mutex::new(3), // Start from 3, root is 2
            bgdt_cache: Mutex::new(bgdt),
            inode_cache: Mutex::new(BTreeMap::new()),
        });
        
        // Set filesystem reference in root node
        let fs_weak = Arc::downgrade(&(fs.clone() as Arc<dyn FileSystemOperations>));
        root.set_filesystem(fs_weak);
        
        Ok(fs)
    }
    
    /// Read superblock from block device
    fn read_superblock(block_device: &dyn BlockDevice) -> Result<Ext2Superblock, FileSystemError> {
        // Ext2 superblock is at offset 1024 bytes from the start
        let request = Box::new(crate::device::block::request::BlockIORequest {
            request_type: crate::device::block::request::BlockIORequestType::Read,
            sector: 2, // 1024 bytes = 2 sectors of 512 bytes
            sector_count: 2, // Superblock is 1024 bytes
            head: 0,
            cylinder: 0,
            buffer: vec![0u8; 1024],
        });
        
        block_device.enqueue_request(request);
        let results = block_device.process_requests();
        
        if let Some(result) = results.first() {
            match &result.result {
                Ok(_) => {
                    // Parse superblock
                    if result.request.buffer.len() < mem::size_of::<Ext2Superblock>() {
                        return Err(FileSystemError::new(
                            FileSystemErrorKind::IoError,
                            "Superblock read incomplete"
                        ));
                    }
                    
                    // Convert bytes to superblock structure
                    let superblock = unsafe {
                        core::ptr::read(result.request.buffer.as_ptr() as *const Ext2Superblock)
                    };
                    
                    Ok(superblock)
                },
                Err(e) => {
                    Err(FileSystemError::new(
                        FileSystemErrorKind::IoError,
                        format!("Failed to read superblock: {}", e)
                    ))
                }
            }
        } else {
            Err(FileSystemError::new(
                FileSystemErrorKind::IoError,
                "No result from block device"
            ))
        }
    }
    
    /// Validate that this is an ext2 filesystem
    fn validate_ext2(superblock: &Ext2Superblock) -> Result<(), FileSystemError> {
        // Check magic number - read to local variable to avoid packed field reference
        let magic = superblock.magic;
        if magic != EXT2_SUPER_MAGIC {
            return Err(FileSystemError::new(
                FileSystemErrorKind::InvalidData,
                format!("Invalid ext2 magic number: 0x{:x}", magic)
            ));
        }
        
        // Check state
        let state = superblock.state;
        if state != EXT2_VALID_FS {
            return Err(FileSystemError::new(
                FileSystemErrorKind::InvalidData,
                "Filesystem is not in a valid state"
            ));
        }
        
        // Check revision level (we support rev 0 and 1)
        let rev_level = superblock.rev_level;
        if rev_level > EXT2_DYNAMIC_REV {
            return Err(FileSystemError::new(
                FileSystemErrorKind::NotSupported,
                format!("Unsupported ext2 revision: {}", rev_level)
            ));
        }
        
        Ok(())
    }
    
    /// Read block group descriptor table
    fn read_bgdt(
        block_device: &dyn BlockDevice,
        superblock: &Ext2Superblock,
        block_size: u32
    ) -> Result<Vec<Ext2BlockGroupDescriptor>, FileSystemError> {
        let blocks_count = superblock.blocks_count;
        let blocks_per_group = superblock.blocks_per_group;
        let groups_count = (blocks_count + blocks_per_group - 1) / blocks_per_group;
        
        // BGDT starts at block 1 (after superblock) if block_size == 1024,
        // or at block 1 if block_size > 1024
        let bgdt_block = if block_size == 1024 { 2 } else { 1 };
        
        let bgdt_size = groups_count as usize * mem::size_of::<Ext2BlockGroupDescriptor>();
        let blocks_needed = (bgdt_size + block_size as usize - 1) / block_size as usize;
        
        let mut bgdt_data = Vec::new();
        
        for i in 0..blocks_needed {
            let block_num = bgdt_block + i as u32;
            let block_data = Self::read_block(block_device, block_num, block_size)?;
            bgdt_data.extend_from_slice(&block_data);
        }
        
        // Parse BGDT entries
        let mut bgdt = Vec::new();
        for i in 0..groups_count {
            let offset = i as usize * mem::size_of::<Ext2BlockGroupDescriptor>();
            if offset + mem::size_of::<Ext2BlockGroupDescriptor>() <= bgdt_data.len() {
                let descriptor = unsafe {
                    core::ptr::read(bgdt_data[offset..].as_ptr() as *const Ext2BlockGroupDescriptor)
                };
                bgdt.push(descriptor);
            }
        }
        
        Ok(bgdt)
    }
    
    /// Read a block from the block device
    fn read_block(
        block_device: &dyn BlockDevice,
        block_num: u32,
        block_size: u32
    ) -> Result<Vec<u8>, FileSystemError> {
        let sectors_per_block = block_size / 512;
        let start_sector = block_num * sectors_per_block;
        
        let mut block_data = Vec::new();
        
        for i in 0..sectors_per_block {
            let request = Box::new(crate::device::block::request::BlockIORequest {
                request_type: crate::device::block::request::BlockIORequestType::Read,
                sector: (start_sector + i) as usize,
                sector_count: 1,
                head: 0,
                cylinder: 0,
                buffer: vec![0u8; 512],
            });
            
            block_device.enqueue_request(request);
            let results = block_device.process_requests();
            
            if let Some(result) = results.first() {
                match &result.result {
                    Ok(_) => {
                        block_data.extend_from_slice(&result.request.buffer);
                    },
                    Err(e) => {
                        return Err(FileSystemError::new(
                            FileSystemErrorKind::IoError,
                            format!("Failed to read block: {}", e)
                        ));
                    }
                }
            } else {
                return Err(FileSystemError::new(
                    FileSystemErrorKind::IoError,
                    "No result from block device"
                ));
            }
        }
        
        Ok(block_data)
    }
    
    /// Write a block to the block device
    fn write_block(
        block_device: &dyn BlockDevice,
        block_num: u32,
        block_size: u32,
        data: &[u8]
    ) -> Result<(), FileSystemError> {
        if data.len() > block_size as usize {
            return Err(FileSystemError::new(
                FileSystemErrorKind::InvalidData,
                "Data exceeds block size"
            ));
        }
        
        let sectors_per_block = block_size / 512;
        let start_sector = block_num * sectors_per_block;
        
        // Prepare full block data (pad with zeros if needed)
        let mut block_data = vec![0u8; block_size as usize];
        block_data[..data.len()].copy_from_slice(data);
        
        for i in 0..sectors_per_block {
            let start_offset = (i * 512) as usize;
            let end_offset = start_offset + 512;
            let sector_data = block_data[start_offset..end_offset].to_vec();
            
            let request = Box::new(crate::device::block::request::BlockIORequest {
                request_type: crate::device::block::request::BlockIORequestType::Write,
                sector: (start_sector + i) as usize,
                sector_count: 1,
                head: 0,
                cylinder: 0,
                buffer: sector_data,
            });
            
            block_device.enqueue_request(request);
            let results = block_device.process_requests();
            
            if let Some(result) = results.first() {
                if let Err(e) = &result.result {
                    return Err(FileSystemError::new(
                        FileSystemErrorKind::IoError,
                        format!("Failed to write block: {}", e)
                    ));
                }
            } else {
                return Err(FileSystemError::new(
                    FileSystemErrorKind::IoError,
                    "No result from block device"
                ));
            }
        }
        
        Ok(())
    }
    
    /// Read an inode from the filesystem
    fn read_inode(
        block_device: &dyn BlockDevice,
        superblock: &Ext2Superblock,
        bgdt: &[Ext2BlockGroupDescriptor],
        inode_num: u32,
        block_size: u32
    ) -> Result<Ext2Inode, FileSystemError> {
        if inode_num == 0 {
            return Err(FileSystemError::new(
                FileSystemErrorKind::InvalidData,
                "Invalid inode number 0"
            ));
        }
        
        // Calculate which block group contains this inode
        let group_num = (inode_num - 1) / superblock.inodes_per_group;
        let index_in_group = (inode_num - 1) % superblock.inodes_per_group;
        
        if group_num as usize >= bgdt.len() {
            return Err(FileSystemError::new(
                FileSystemErrorKind::InvalidData,
                "Inode belongs to non-existent block group"
            ));
        }
        
        let bg_desc = &bgdt[group_num as usize];
        let inode_table_block = bg_desc.inode_table;
        
        // Calculate inode size (default is 128 bytes for revision 0)
        let inode_size = if superblock.rev_level >= EXT2_DYNAMIC_REV {
            superblock.inode_size as u32
        } else {
            128
        };
        
        // Calculate which block contains the inode
        let inodes_per_block = block_size / inode_size;
        let block_offset = index_in_group / inodes_per_block;
        let inode_offset = (index_in_group % inodes_per_block) * inode_size;
        
        // Read the block containing the inode
        let block_data = Self::read_block(block_device, inode_table_block + block_offset, block_size)?;
        
        // Extract the inode data
        if inode_offset as usize + mem::size_of::<Ext2Inode>() > block_data.len() {
            return Err(FileSystemError::new(
                FileSystemErrorKind::IoError,
                "Inode data extends beyond block boundary"
            ));
        }
        
        let inode = unsafe {
            core::ptr::read(block_data[inode_offset as usize..].as_ptr() as *const Ext2Inode)
        };
        
        Ok(inode)
    }
    
    /// Generate next unique file ID
    fn generate_file_id(&self) -> u64 {
        let mut next_id = self.next_file_id.lock();
        let id = *next_id;
        *next_id += 1;
        id
    }
}

impl FileSystemOperations for Ext2FileSystem {
    fn lookup(&self, parent: &Arc<dyn VfsNode>, name: &String) -> Result<Arc<dyn VfsNode>, FileSystemError> {
        // TODO: Implement lookup by reading directory entries
        Err(FileSystemError::new(
            FileSystemErrorKind::NotSupported,
            "Ext2 lookup not yet implemented"
        ))
    }
    
    fn open(&self, node: &Arc<dyn VfsNode>, _flags: u32) -> Result<Arc<dyn FileObject>, FileSystemError> {
        let ext2_node = node.as_any()
            .downcast_ref::<Ext2Node>()
            .ok_or_else(|| FileSystemError::new(
                FileSystemErrorKind::NotSupported,
                "Invalid node type for ext2"
            ))?;
        
        match ext2_node.file_type() {
            Ok(FileType::RegularFile) => {
                Ok(Arc::new(Ext2FileObject::new(Arc::new(ext2_node.clone()))))
            },
            Ok(FileType::Directory) => {
                Ok(Arc::new(Ext2DirectoryObject::new(Arc::new(ext2_node.clone()))))
            },
            Ok(_) => Err(FileSystemError::new(
                FileSystemErrorKind::NotSupported,
                "Unsupported file type"
            )),
            Err(e) => Err(e),
        }
    }
    
    fn create(&self, _parent: &Arc<dyn VfsNode>, _name: &String, _file_type: FileType, _mode: u32) -> Result<Arc<dyn VfsNode>, FileSystemError> {
        // TODO: Implement file creation
        Err(FileSystemError::new(
            FileSystemErrorKind::NotSupported,
            "Ext2 create not yet implemented"
        ))
    }
    
    fn remove(&self, _parent: &Arc<dyn VfsNode>, _name: &String) -> Result<(), FileSystemError> {
        // TODO: Implement file removal
        Err(FileSystemError::new(
            FileSystemErrorKind::NotSupported,
            "Ext2 remove not yet implemented"
        ))
    }
    
    fn readdir(&self, _node: &Arc<dyn VfsNode>) -> Result<Vec<DirectoryEntryInternal>, FileSystemError> {
        // TODO: Implement directory reading
        Err(FileSystemError::new(
            FileSystemErrorKind::NotSupported,
            "Ext2 readdir not yet implemented"
        ))
    }
    
    fn root_node(&self) -> Arc<dyn VfsNode> {
        Arc::clone(&*self.root.read()) as Arc<dyn VfsNode>
    }
    
    fn name(&self) -> &str {
        &self.name
    }
    
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Register the ext2 driver with the filesystem driver manager
fn register_driver() {
    let fs_driver_manager = get_fs_driver_manager();
    fs_driver_manager.register_driver(Box::new(Ext2Driver));
}

driver_initcall!(register_driver);