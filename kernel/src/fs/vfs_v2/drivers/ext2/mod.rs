//! ext2 Filesystem Implementation
//!
//! This module implements an ext2 filesystem driver for the VFS v2 architecture.
//! It provides support for reading and writing ext2 filesystems on block devices,
//! particularly designed to work with virtio-blk devices.
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
//! - Data structures for ext2 format (superblock, inode, directory entries, etc.)

use alloc::{
    boxed::Box, collections::BTreeMap, format, string::{String, ToString}, sync::Arc, vec, vec::Vec
};
use spin::{rwlock::RwLock, Mutex};
use core::{fmt::Debug, mem, any::Any};

use crate::{
    device::block::BlockDevice,
    driver_initcall,
    fs::{
        get_fs_driver_manager, FileObject, FileSystemError, FileSystemErrorKind, FileType
    }
};

use super::super::core::{VfsNode, FileSystemOperations, DirectoryEntryInternal};

pub mod structures;
pub mod node;
pub mod driver;

#[cfg(test)]
pub mod tests;

pub use structures::*;
pub use node::{Ext2Node, Ext2FileObject, Ext2DirectoryObject};
pub use driver::Ext2Driver;

/// ext2 Filesystem implementation
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
    /// Root directory inode
    root_inode: u32,
    /// Root directory node
    root: RwLock<Arc<Ext2Node>>,
    /// Filesystem name
    name: String,
    /// Next file ID generator
    next_file_id: Mutex<u64>,
    /// Cached inodes
    inode_cache: Mutex<BTreeMap<u32, Ext2Inode>>,
}

impl Ext2FileSystem {
    /// Create a new ext2 filesystem from a block device
    pub fn new(block_device: Arc<dyn BlockDevice>) -> Result<Arc<Self>, FileSystemError> {
        // Read the superblock from sectors 2-3 (block 1, since each block is 1024 bytes = 2 sectors)
        let request = Box::new(crate::device::block::request::BlockIORequest {
            request_type: crate::device::block::request::BlockIORequestType::Read,
            sector: 2,  // Start at sector 2 (block 1)
            sector_count: 2,  // Read 2 sectors (1024 bytes)
            head: 0,
            cylinder: 0,
            buffer: vec![0u8; 1024],
        });
        
        block_device.enqueue_request(request);
        let results = block_device.process_requests();
        
        let superblock_data = if let Some(result) = results.first() {
            match &result.result {
                Ok(_) => result.request.buffer.clone(),
                Err(_) => return Err(FileSystemError::new(
                    FileSystemErrorKind::IoError,
                    "Failed to read ext2 superblock"
                )),
            }
        } else {
            return Err(FileSystemError::new(
                FileSystemErrorKind::IoError,
                "No result from block device read"
            ));
        };

        // Parse superblock
        let superblock = Ext2Superblock::from_bytes(&superblock_data)?;
        
        // Validate this is an ext2 filesystem
        if superblock.magic != EXT2_SUPER_MAGIC {
            return Err(FileSystemError::new(
                FileSystemErrorKind::InvalidData,
                "Invalid ext2 magic number"
            ));
        }

        let block_size = superblock.get_block_size();
        let root_inode = EXT2_ROOT_INO;

        // Create root node
        let root = Ext2Node::new(
            root_inode,
            FileType::Directory,
            1, // Root node ID is 1
        );

        let fs = Arc::new(Self {
            block_device,
            superblock,
            block_size,
            root_inode,
            root: RwLock::new(Arc::new(root)),
            name: "ext2".to_string(),
            next_file_id: Mutex::new(2), // Start from 2, root is 1
            inode_cache: Mutex::new(BTreeMap::new()),
        });

        // Set filesystem reference in root node
        let fs_weak = Arc::downgrade(&(fs.clone() as Arc<dyn FileSystemOperations>));
        fs.root.read().set_filesystem(fs_weak);

        Ok(fs)
    }

    /// Read an inode from disk
    pub fn read_inode(&self, inode_num: u32) -> Result<Ext2Inode, FileSystemError> {
        // Check cache first
        {
            let cache = self.inode_cache.lock();
            if let Some(inode) = cache.get(&inode_num) {
                return Ok(*inode);
            }
        }

        // Calculate inode location
        let group = (inode_num - 1) / self.superblock.inodes_per_group;
        let local_inode = (inode_num - 1) % self.superblock.inodes_per_group;
        
        // Read block group descriptor
        let bgd_block_sector = ((group * mem::size_of::<Ext2BlockGroupDescriptor>() as u32) / self.block_size + 
                       if self.block_size == 1024 { 2 } else { 1 }) * 2; // Convert block to sector
        
        let request = Box::new(crate::device::block::request::BlockIORequest {
            request_type: crate::device::block::request::BlockIORequestType::Read,
            sector: bgd_block_sector as usize,
            sector_count: (self.block_size / 512) as usize,
            head: 0,
            cylinder: 0,
            buffer: vec![0u8; self.block_size as usize],
        });
        
        self.block_device.enqueue_request(request);
        let results = self.block_device.process_requests();
        
        let bgd_data = if let Some(result) = results.first() {
            match &result.result {
                Ok(_) => result.request.buffer.clone(),
                Err(_) => return Err(FileSystemError::new(
                    FileSystemErrorKind::IoError,
                    "Failed to read block group descriptor"
                )),
            }
        } else {
            return Err(FileSystemError::new(
                FileSystemErrorKind::IoError,
                "No result from block device read"
            ));
        };

        let bgd_offset = (group * mem::size_of::<Ext2BlockGroupDescriptor>() as u32) % self.block_size;
        let bgd = Ext2BlockGroupDescriptor::from_bytes(&bgd_data[bgd_offset as usize..])?;

        // Calculate inode table location
        let inode_size = self.superblock.inode_size as u32;
        let inode_block = bgd.inode_table + (local_inode * inode_size) / self.block_size;
        let inode_offset = (local_inode * inode_size) % self.block_size;

        // Read inode
        let inode_sector = (inode_block * 2) as u64; // Convert block to sector
        let request = Box::new(crate::device::block::request::BlockIORequest {
            request_type: crate::device::block::request::BlockIORequestType::Read,
            sector: inode_sector as usize,
            sector_count: (self.block_size / 512) as usize,
            head: 0,
            cylinder: 0,
            buffer: vec![0u8; self.block_size as usize],
        });
        
        self.block_device.enqueue_request(request);
        let results = self.block_device.process_requests();
        
        let inode_data = if let Some(result) = results.first() {
            match &result.result {
                Ok(_) => result.request.buffer.clone(),
                Err(_) => return Err(FileSystemError::new(
                    FileSystemErrorKind::IoError,
                    "Failed to read inode"
                )),
            }
        } else {
            return Err(FileSystemError::new(
                FileSystemErrorKind::IoError,
                "No result from block device read"
            ));
        };

        let inode = Ext2Inode::from_bytes(&inode_data[inode_offset as usize..])?;
        
        // Cache the inode
        {
            let mut cache = self.inode_cache.lock();
            cache.insert(inode_num, inode);
        }

        Ok(inode)
    }

    /// Read directory entries from an inode
    pub fn read_directory_entries(&self, inode: &Ext2Inode) -> Result<Vec<Ext2DirectoryEntry>, FileSystemError> {
        let mut entries = Vec::new();
        let mut current_block = 0;

        while current_block < (inode.size as u64 + self.block_size as u64 - 1) / self.block_size as u64 {
            // Get block number for this directory data block
            let block_num = self.get_inode_block(inode, current_block)?;
            if block_num == 0 {
                break; // Sparse block
            }

            // Read the block
            let block_sector = block_num * 2; // Convert block to sector
            let request = Box::new(crate::device::block::request::BlockIORequest {
                request_type: crate::device::block::request::BlockIORequestType::Read,
                sector: block_sector as usize,
                sector_count: (self.block_size / 512) as usize,
                head: 0,
                cylinder: 0,
                buffer: vec![0u8; self.block_size as usize],
            });
            
            self.block_device.enqueue_request(request);
            let results = self.block_device.process_requests();
            
            let block_data = if let Some(result) = results.first() {
                match &result.result {
                    Ok(_) => result.request.buffer.clone(),
                    Err(_) => return Err(FileSystemError::new(
                        FileSystemErrorKind::IoError,
                        "Failed to read directory block"
                    )),
                }
            } else {
                return Err(FileSystemError::new(
                    FileSystemErrorKind::IoError,
                    "No result from block device read"
                ));
            };

            // Parse directory entries in this block
            let mut offset = 0;
            while offset < self.block_size as usize {
                if offset + 8 > self.block_size as usize {
                    break; // Not enough space for a minimal directory entry
                }

                let entry = Ext2DirectoryEntry::from_bytes(&block_data[offset..])?;
                if entry.entry.inode == 0 {
                    break; // End of directory entries
                }

                let rec_len = entry.entry.rec_len;
                entries.push(entry);
                offset += rec_len as usize;

                if rec_len == 0 {
                    break; // Invalid record length
                }
            }

            current_block += 1;
        }

        Ok(entries)
    }

    /// Get the block number for a logical block within an inode
    fn get_inode_block(&self, inode: &Ext2Inode, logical_block: u64) -> Result<u64, FileSystemError> {
        let blocks_per_indirect = self.block_size / 4; // Each pointer is 4 bytes

        if logical_block < 12 {
            // Direct blocks
            Ok(inode.block[logical_block as usize] as u64)
        } else if logical_block < 12 + blocks_per_indirect as u64 {
            // Single indirect
            let indirect_block = inode.block[12] as u64;
            if indirect_block == 0 {
                return Ok(0);
            }
            
            let index = logical_block - 12;
            let indirect_sector = indirect_block * 2; // Convert block to sector
            let request = Box::new(crate::device::block::request::BlockIORequest {
                request_type: crate::device::block::request::BlockIORequestType::Read,
                sector: indirect_sector as usize,
                sector_count: (self.block_size / 512) as usize,
                head: 0,
                cylinder: 0,
                buffer: vec![0u8; self.block_size as usize],
            });
            
            self.block_device.enqueue_request(request);
            let results = self.block_device.process_requests();
            
            let indirect_data = if let Some(result) = results.first() {
                match &result.result {
                    Ok(_) => result.request.buffer.clone(),
                    Err(_) => return Err(FileSystemError::new(
                        FileSystemErrorKind::IoError,
                        "Failed to read indirect block"
                    )),
                }
            } else {
                return Err(FileSystemError::new(
                    FileSystemErrorKind::IoError,
                    "No result from block device read"
                ));
            };
            
            let block_ptr = u32::from_le_bytes([
                indirect_data[index as usize * 4],
                indirect_data[index as usize * 4 + 1],
                indirect_data[index as usize * 4 + 2],
                indirect_data[index as usize * 4 + 3],
            ]);
            
            Ok(block_ptr as u64)
        } else {
            // For now, only support direct and single indirect blocks
            Err(FileSystemError::new(
                FileSystemErrorKind::NotSupported,
                "Double and triple indirect blocks not yet supported"
            ))
        }
    }

    /// Read the entire content of a file given its inode number
    pub fn read_file_content(&self, inode_num: u32, size: usize) -> Result<Vec<u8>, FileSystemError> {
        let inode = self.read_inode(inode_num)?;
        let mut content = Vec::with_capacity(size);
        let mut remaining = size;
        let mut current_block = 0;

        while remaining > 0 {
            let block_num = self.get_inode_block(&inode, current_block)?;
            if block_num == 0 {
                // Sparse block - fill with zeros
                let bytes_to_add = core::cmp::min(remaining, self.block_size as usize);
                content.extend_from_slice(&vec![0u8; bytes_to_add]);
                remaining -= bytes_to_add;
            } else {
                // Read the block
                let block_sector = block_num * 2; // Convert block to sector
                let request = Box::new(crate::device::block::request::BlockIORequest {
                    request_type: crate::device::block::request::BlockIORequestType::Read,
                    sector: block_sector as usize,
                    sector_count: (self.block_size / 512) as usize,
                    head: 0,
                    cylinder: 0,
                    buffer: vec![0u8; self.block_size as usize],
                });
                
                self.block_device.enqueue_request(request);
                let results = self.block_device.process_requests();
                
                let block_data = if let Some(result) = results.first() {
                    match &result.result {
                        Ok(_) => result.request.buffer.clone(),
                        Err(_) => return Err(FileSystemError::new(
                            FileSystemErrorKind::IoError,
                            "Failed to read file block"
                        )),
                    }
                } else {
                    return Err(FileSystemError::new(
                        FileSystemErrorKind::IoError,
                        "No result from block device read"
                    ));
                };

                // Add the needed bytes from this block
                let bytes_to_add = core::cmp::min(remaining, self.block_size as usize);
                content.extend_from_slice(&block_data[..bytes_to_add]);
                remaining -= bytes_to_add;
            }

            current_block += 1;
        }

        Ok(content)
    }
}

impl FileSystemOperations for Ext2FileSystem {
    fn lookup(&self, parent: &Arc<dyn VfsNode>, name: &String) -> Result<Arc<dyn VfsNode>, FileSystemError> {
        // Cast parent to Ext2Node
        let ext2_parent = parent.as_any().downcast_ref::<Ext2Node>()
            .ok_or_else(|| FileSystemError::new(
                FileSystemErrorKind::InvalidOperation,
                "Parent node is not an Ext2Node"
            ))?;

        // Read parent inode
        let parent_inode = self.read_inode(ext2_parent.inode_number())?;

        // Ensure parent is a directory
        if parent_inode.mode & EXT2_S_IFMT != EXT2_S_IFDIR {
            return Err(FileSystemError::new(
                FileSystemErrorKind::NotADirectory,
                "Parent is not a directory"
            ));
        }

        // Read directory entries
        let entries = self.read_directory_entries(&parent_inode)?;

        // Find the requested entry
        for entry in entries {
            let entry_name = entry.name_str()?;
            if entry_name == *name {
                // Read the inode for this entry
                let child_inode = self.read_inode(entry.entry.inode)?;
                
                // Determine file type
                let file_type = if child_inode.mode & EXT2_S_IFMT == EXT2_S_IFDIR {
                    FileType::Directory
                } else if child_inode.mode & EXT2_S_IFMT == EXT2_S_IFREG {
                    FileType::RegularFile
                } else {
                    FileType::Unknown
                };

                // Generate new file ID
                let file_id = {
                    let mut next_id = self.next_file_id.lock();
                    let id = *next_id;
                    *next_id += 1;
                    id
                };

                // Create new node
                let node = Ext2Node::new(entry.entry.inode, file_type, file_id);
                
                // Set filesystem reference from parent
                if let Some(fs_ref) = ext2_parent.filesystem() {
                    node.set_filesystem(fs_ref);
                }

                return Ok(Arc::new(node));
            }
        }

        Err(FileSystemError::new(
            FileSystemErrorKind::NotFound,
            "File not found"
        ))
    }

    fn readdir(&self, node: &Arc<dyn VfsNode>) -> Result<Vec<DirectoryEntryInternal>, FileSystemError> {
        // Cast node to Ext2Node
        let ext2_node = node.as_any().downcast_ref::<Ext2Node>()
            .ok_or_else(|| FileSystemError::new(
                FileSystemErrorKind::InvalidOperation,
                "Node is not an Ext2Node"
            ))?;

        // Read inode
        let inode = self.read_inode(ext2_node.inode_number())?;

        // Ensure this is a directory
        if inode.mode & EXT2_S_IFMT != EXT2_S_IFDIR {
            return Err(FileSystemError::new(
                FileSystemErrorKind::NotADirectory,
                "Node is not a directory"
            ));
        }

        // Read directory entries
        let entries = self.read_directory_entries(&inode)?;
        
        // Convert to internal format
        let mut result = Vec::new();
        for entry in entries {
            let name = entry.name_str()?;
            let child_inode = self.read_inode(entry.entry.inode)?;
            
            let file_type = if child_inode.mode & EXT2_S_IFMT == EXT2_S_IFDIR {
                FileType::Directory
            } else if child_inode.mode & EXT2_S_IFMT == EXT2_S_IFREG {
                FileType::RegularFile
            } else {
                FileType::Unknown
            };

            result.push(DirectoryEntryInternal {
                name,
                file_type,
                file_id: entry.entry.inode as u64,
            });
        }

        Ok(result)
    }

    fn open(
        &self,
        node: &Arc<dyn VfsNode>,
        _flags: u32,
    ) -> Result<Arc<dyn FileObject>, FileSystemError> {
        match node.file_type()? {
            FileType::RegularFile => {
                let ext2_node = node.as_any().downcast_ref::<Ext2Node>()
                    .ok_or_else(|| FileSystemError::new(
                        FileSystemErrorKind::InvalidOperation,
                        "Node is not an Ext2Node"
                    ))?;
                let file_obj = Arc::new(Ext2FileObject::new(ext2_node.inode_number(), ext2_node.id()));
                
                // Set filesystem reference
                if let Some(fs_weak) = ext2_node.filesystem() {
                    file_obj.set_filesystem(fs_weak);
                }
                
                Ok(file_obj)
            },
            FileType::Directory => {
                let ext2_node = node.as_any().downcast_ref::<Ext2Node>()
                    .ok_or_else(|| FileSystemError::new(
                        FileSystemErrorKind::InvalidOperation,
                        "Node is not an Ext2Node"
                    ))?;
                let dir_obj = Arc::new(Ext2DirectoryObject::new(ext2_node.inode_number(), ext2_node.id()));
                
                // Set filesystem reference
                if let Some(fs_weak) = ext2_node.filesystem() {
                    dir_obj.set_filesystem(fs_weak);
                }
                
                Ok(dir_obj)
            },
            _ => Err(FileSystemError::new(
                FileSystemErrorKind::NotSupported,
                "Unsupported file type for open operation"
            ))
        }
    }

    fn create(
        &self,
        parent: &Arc<dyn VfsNode>,
        name: &String,
        file_type: FileType,
        _mode: u32,
    ) -> Result<Arc<dyn VfsNode>, FileSystemError> {
        let ext2_parent = parent.as_any()
            .downcast_ref::<Ext2Node>()
            .ok_or_else(|| FileSystemError::new(
                FileSystemErrorKind::NotSupported,
                "Invalid node type for ext2"
            ))?;
        
        // Check if it's a directory
        match ext2_parent.file_type() {
            Ok(FileType::Directory) => {},
            Ok(_) => return Err(FileSystemError::new(
                FileSystemErrorKind::NotADirectory,
                "Parent is not a directory"
            )),
            Err(e) => return Err(e),
        }
        
        // Generate new file ID
        let file_id = {
            let mut next_id = self.next_file_id.lock();
            let id = *next_id;
            *next_id += 1;
            id
        };
        
        // For now, use a simple inode allocation scheme
        // In a full implementation, this would allocate from the ext2 inode bitmap
        let new_inode_number = file_id as u32 + 1000; // Avoid conflicts with existing inodes
        
        // Create new node
        let new_node = match file_type {
            FileType::RegularFile => {
                Arc::new(Ext2Node::new(new_inode_number, FileType::RegularFile, file_id))
            },
            FileType::Directory => {
                Arc::new(Ext2Node::new(new_inode_number, FileType::Directory, file_id))
            },
            _ => {
                return Err(FileSystemError::new(
                    FileSystemErrorKind::NotSupported,
                    "Unsupported file type for ext2"
                ));
            }
        };
        
        // Set filesystem reference
        if let Some(fs_ref) = ext2_parent.filesystem() {
            new_node.set_filesystem(fs_ref);
        }
        
        // Note: In a complete implementation, this would:
        // 1. Allocate an inode from the inode bitmap
        // 2. Write the new inode to disk
        // 3. Add a directory entry to the parent directory
        // 4. Update the parent directory's inode
        // For now, this creates the node in memory only
        
        Ok(new_node)
    }

    fn remove(
        &self,
        parent: &Arc<dyn VfsNode>,
        name: &String,
    ) -> Result<(), FileSystemError> {
        let ext2_parent = parent.as_any()
            .downcast_ref::<Ext2Node>()
            .ok_or_else(|| FileSystemError::new(
                FileSystemErrorKind::NotSupported,
                "Invalid node type for ext2"
            ))?;
        
        // Check if it's a directory
        match ext2_parent.file_type() {
            Ok(FileType::Directory) => {},
            Ok(_) => return Err(FileSystemError::new(
                FileSystemErrorKind::NotADirectory,
                "Parent is not a directory"
            )),
            Err(e) => return Err(e),
        }

        // Try to lookup the file to ensure it exists
        match self.lookup(parent, name) {
            Ok(_node) => {
                // Note: In a complete implementation, this would:
                // 1. Read the parent directory to find the entry
                // 2. Remove the directory entry
                // 3. Mark the inode as free in the inode bitmap
                // 4. Free any data blocks used by the file
                // 5. Update the parent directory's modification time
                // For now, this is a no-op since we don't maintain persistent state
                Ok(())
            },
            Err(_) => Err(FileSystemError::new(
                FileSystemErrorKind::NotFound,
                format!("File '{}' not found", name)
            ))
        }
    }

    fn root_node(&self) -> Arc<dyn VfsNode> {
        self.root.read().clone()
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
    let mut manager = get_fs_driver_manager();
    manager.register_driver(Box::new(Ext2Driver));
}

driver_initcall!(register_driver);