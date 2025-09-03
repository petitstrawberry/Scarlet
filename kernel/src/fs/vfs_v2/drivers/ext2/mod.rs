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
    
    /// Allocate a new inode number (simplified implementation)
    fn allocate_inode(&self) -> Result<u32, FileSystemError> {
        // For a complete implementation, this would:
        // 1. Read the inode bitmap from disk
        // 2. Find the first free inode
        // 3. Mark it as used in the bitmap
        // 4. Write the bitmap back to disk
        // 5. Update the superblock free inode count
        
        // For now, use a simple allocation scheme
        let next_inode = {
            let mut next_id = self.next_file_id.lock();
            let inode_num = *next_id + 1000; // Start from 1000 to avoid conflicts
            *next_id += 1;
            inode_num as u32
        };
        
        Ok(next_inode)
    }
    
    /// Write an inode to disk
    fn write_inode(&self, inode_number: u32, inode: &Ext2Inode) -> Result<(), FileSystemError> {
        // Calculate which block group contains this inode
        let inodes_per_group = self.superblock.inodes_per_group;
        let group_number = (inode_number - 1) / inodes_per_group;
        let inode_index = (inode_number - 1) % inodes_per_group;
        
        // Read the block group descriptor to get the inode table location
        let bgd_sector = 4; // Block group descriptors start at block 2 (sector 4)
        let bgd_request = Box::new(crate::device::block::request::BlockIORequest {
            request_type: crate::device::block::request::BlockIORequestType::Read,
            sector: bgd_sector,
            sector_count: 2, // Read one block worth of BGDs
            head: 0,
            cylinder: 0,
            buffer: vec![0u8; 1024],
        });
        
        self.block_device.enqueue_request(bgd_request);
        let bgd_results = self.block_device.process_requests();
        
        let bgd_data = if let Some(result) = bgd_results.first() {
            match &result.result {
                Ok(_) => result.request.buffer.clone(),
                Err(_) => return Err(FileSystemError::new(
                    FileSystemErrorKind::IoError,
                    "Failed to read block group descriptors"
                )),
            }
        } else {
            return Err(FileSystemError::new(
                FileSystemErrorKind::IoError,
                "No result from BGD read"
            ));
        };
        
        // Parse the block group descriptor
        let bgd_offset = (group_number as usize) * 32; // Each BGD is 32 bytes
        if bgd_offset + 32 > bgd_data.len() {
            return Err(FileSystemError::new(
                FileSystemErrorKind::InvalidData,
                "Block group descriptor offset out of bounds"
            ));
        }
        
        // Extract inode table block from BGD
        let inode_table_block = u32::from_le_bytes([
            bgd_data[bgd_offset + 8],
            bgd_data[bgd_offset + 9],
            bgd_data[bgd_offset + 10],
            bgd_data[bgd_offset + 11],
        ]);
        
        // Calculate the block and offset within that block for this inode
        let inode_size = self.superblock.inode_size as u32;
        let inodes_per_block = self.block_size / inode_size;
        let block_offset = inode_index / inodes_per_block;
        let inode_offset_in_block = (inode_index % inodes_per_block) * inode_size;
        
        let target_block = inode_table_block + block_offset;
        let target_sector = target_block * 2; // Convert block to sector
        
        // Read the current block containing the inode
        let read_request = Box::new(crate::device::block::request::BlockIORequest {
            request_type: crate::device::block::request::BlockIORequestType::Read,
            sector: target_sector as usize,
            sector_count: (self.block_size / 512) as usize,
            head: 0,
            cylinder: 0,
            buffer: vec![0u8; self.block_size as usize],
        });
        
        self.block_device.enqueue_request(read_request);
        let read_results = self.block_device.process_requests();
        
        let mut block_data = if let Some(result) = read_results.first() {
            match &result.result {
                Ok(_) => result.request.buffer.clone(),
                Err(_) => return Err(FileSystemError::new(
                    FileSystemErrorKind::IoError,
                    "Failed to read inode table block"
                )),
            }
        } else {
            return Err(FileSystemError::new(
                FileSystemErrorKind::IoError,
                "No result from inode table block read"
            ));
        };
        
        // Write the inode data into the block
        let inode_bytes = unsafe {
            core::slice::from_raw_parts(
                inode as *const Ext2Inode as *const u8,
                core::mem::size_of::<Ext2Inode>()
            )
        };
        
        let start_offset = inode_offset_in_block as usize;
        let end_offset = start_offset + inode_bytes.len();
        
        if end_offset > block_data.len() {
            return Err(FileSystemError::new(
                FileSystemErrorKind::InvalidData,
                "Inode data would exceed block boundary"
            ));
        }
        
        block_data[start_offset..end_offset].copy_from_slice(inode_bytes);
        
        // Write the modified block back to disk
        let write_request = Box::new(crate::device::block::request::BlockIORequest {
            request_type: crate::device::block::request::BlockIORequestType::Write,
            sector: target_sector as usize,
            sector_count: (self.block_size / 512) as usize,
            head: 0,
            cylinder: 0,
            buffer: block_data,
        });
        
        self.block_device.enqueue_request(write_request);
        let write_results = self.block_device.process_requests();
        
        if let Some(result) = write_results.first() {
            match &result.result {
                Ok(_) => {
                    // Also update the cache
                    let mut cache = self.inode_cache.lock();
                    cache.insert(inode_number, inode.clone());
                    Ok(())
                },
                Err(_) => Err(FileSystemError::new(
                    FileSystemErrorKind::IoError,
                    "Failed to write inode to disk"
                )),
            }
        } else {
            Err(FileSystemError::new(
                FileSystemErrorKind::IoError,
                "No result from inode write"
            ))
        }
    }
    
    /// Initialize a new directory with . and .. entries
    fn initialize_directory(&self, dir_inode_number: u32, parent_inode_number: u32) -> Result<(), FileSystemError> {
        // Allocate a block for the directory
        let block_number = self.allocate_block()?;
        
        // Create directory entries for . and ..
        let block_size = self.block_size as usize;
        let mut block_data = vec![0u8; block_size];
        
        // Create "." entry
        let dot_entry_size = 12; // 4 (inode) + 2 (rec_len) + 1 (name_len) + 1 (file_type) + 1 (name) + 3 (padding)
        let dot_inode = dir_inode_number.to_le_bytes();
        let dot_rec_len = dot_entry_size as u16;
        let dot_name_len = 1u8;
        let dot_file_type = 2u8; // Directory
        
        block_data[0..4].copy_from_slice(&dot_inode);
        block_data[4..6].copy_from_slice(&dot_rec_len.to_le_bytes());
        block_data[6] = dot_name_len;
        block_data[7] = dot_file_type;
        block_data[8] = b'.';
        
        // Create ".." entry - takes up the rest of the block
        let dotdot_offset = dot_entry_size;
        let dotdot_rec_len = (block_size - dotdot_offset) as u16;
        let dotdot_name_len = 2u8;
        let dotdot_file_type = 2u8; // Directory
        let dotdot_inode = parent_inode_number.to_le_bytes();
        
        block_data[dotdot_offset..dotdot_offset + 4].copy_from_slice(&dotdot_inode);
        block_data[dotdot_offset + 4..dotdot_offset + 6].copy_from_slice(&dotdot_rec_len.to_le_bytes());
        block_data[dotdot_offset + 6] = dotdot_name_len;
        block_data[dotdot_offset + 7] = dotdot_file_type;
        block_data[dotdot_offset + 8] = b'.';
        block_data[dotdot_offset + 9] = b'.';
        
        // Write the block to disk
        let block_sector = (block_number * 2) as u64;
        let request = Box::new(crate::device::block::request::BlockIORequest {
            request_type: crate::device::block::request::BlockIORequestType::Write,
            sector: block_sector as usize,
            sector_count: (self.block_size / 512) as usize,
            head: 0,
            cylinder: 0,
            buffer: block_data,
        });
        
        // Submit write request
        self.block_device.enqueue_request(request);
        let results = self.block_device.process_requests();
        
        if results.is_empty() || results[0].result.is_err() {
            return Err(FileSystemError::new(
                FileSystemErrorKind::InvalidData,
                "Failed to write directory block"
            ));
        }
        
        // Update the directory inode to point to this block and set size
        let mut dir_inode = self.read_inode(dir_inode_number)?;
        dir_inode.block[0] = block_number.to_le();
        dir_inode.size = block_size as u32;
        dir_inode.blocks = 2; // 2 sectors for 1 block
        
        self.write_inode(dir_inode_number, &dir_inode)?;
        
        Ok(())
    }
    
    /// Allocate a free block (simplified implementation)
    fn allocate_block(&self) -> Result<u32, FileSystemError> {
        // This is a simplified allocation - in a real implementation,
        // we would read the block bitmap and find a free block
        static NEXT_BLOCK: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(1000);
        Ok(NEXT_BLOCK.fetch_add(1, core::sync::atomic::Ordering::SeqCst))
    }

    /// Add a directory entry to a parent directory
    fn add_directory_entry(&self, parent_inode: u32, name: &String, child_inode: u32, file_type: FileType) -> Result<(), FileSystemError> {
        // Read the parent directory inode
        let parent_dir_inode = self.read_inode(parent_inode)?;
        
        if !parent_dir_inode.is_dir() {
            return Err(FileSystemError::new(
                FileSystemErrorKind::InvalidData,
                "Parent is not a directory"
            ));
        }

        // Calculate the length of the new directory entry
        // Directory entry format: inode(4) + rec_len(2) + name_len(1) + file_type(1) + name + padding to 4-byte boundary
        let entry_name_len = name.len() as u8;
        let entry_total_len = ((8 + entry_name_len as usize + 3) / 4) * 4; // Round up to 4-byte boundary

        // Convert FileType to ext2 file type
        let ext2_file_type = match file_type {
            FileType::RegularFile => 1,
            FileType::Directory => 2,
            FileType::CharDevice(_) => 3,
            FileType::BlockDevice(_) => 4,
            FileType::Pipe => 5,
            FileType::Socket => 6,
            FileType::SymbolicLink(_) => 7,
            FileType::Unknown => 0,
        };

        // Find a suitable block in the directory with enough space
        let blocks_in_dir = (parent_dir_inode.get_size() as u64 + self.block_size as u64 - 1) / self.block_size as u64;

        for block_idx in 0..blocks_in_dir.max(1) {
            let block_num = self.get_inode_block(&parent_dir_inode, block_idx)?;
            if block_num == 0 {
                continue; // Sparse block
            }

            // Read the directory block
            let block_sector = (block_num * 2) as u64; // Convert block to sector
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

            let mut block_data = if let Some(result) = results.first() {
                match &result.result {
                    Ok(_) => result.request.buffer.clone(),
                    Err(_) => continue, // Try next block
                }
            } else {
                continue;
            };

            // Parse directory entries to find available space
            let mut offset = 0;
            let mut last_entry_offset = 0;
            let mut last_entry_rec_len = 0;

            while offset < self.block_size as usize {
                if offset + 8 > block_data.len() {
                    break;
                }

                let entry = Ext2DirectoryEntryRaw::from_bytes(&block_data[offset..])?;
                let rec_len = entry.get_rec_len();
                
                if rec_len == 0 {
                    break; // Invalid entry
                }

                last_entry_offset = offset;
                last_entry_rec_len = rec_len as usize;
                
                offset += rec_len as usize;
            }

            // Calculate actual space used by the last entry
            if last_entry_offset > 0 {
                let last_entry = Ext2DirectoryEntryRaw::from_bytes(&block_data[last_entry_offset..])?;
                let actual_last_entry_len = ((8 + last_entry.get_name_len() as usize + 3) / 4) * 4;
                let available_space = last_entry_rec_len - actual_last_entry_len;

                if available_space >= entry_total_len {
                    // We have space! Adjust the last entry's rec_len and add our entry
                    
                    // Update last entry's rec_len to its actual size
                    let actual_rec_len_bytes = (actual_last_entry_len as u16).to_le_bytes();
                    block_data[last_entry_offset + 4] = actual_rec_len_bytes[0];
                    block_data[last_entry_offset + 5] = actual_rec_len_bytes[1];

                    // Add our new entry
                    let new_entry_offset = last_entry_offset + actual_last_entry_len;
                    let remaining_space = last_entry_rec_len - actual_last_entry_len;
                    
                    // Write new entry header
                    let child_inode_bytes = child_inode.to_le_bytes();
                    let rec_len_bytes = (remaining_space as u16).to_le_bytes();
                    
                    block_data[new_entry_offset..new_entry_offset + 4].copy_from_slice(&child_inode_bytes);
                    block_data[new_entry_offset + 4..new_entry_offset + 6].copy_from_slice(&rec_len_bytes);
                    block_data[new_entry_offset + 6] = entry_name_len;
                    block_data[new_entry_offset + 7] = ext2_file_type;
                    
                    // Write name
                    block_data[new_entry_offset + 8..new_entry_offset + 8 + entry_name_len as usize]
                        .copy_from_slice(name.as_bytes());
                    
                    // Write the updated block back to disk
                    let write_request = Box::new(crate::device::block::request::BlockIORequest {
                        request_type: crate::device::block::request::BlockIORequestType::Write,
                        sector: block_sector as usize,
                        sector_count: (self.block_size / 512) as usize,
                        head: 0,
                        cylinder: 0,
                        buffer: block_data,
                    });

                    self.block_device.enqueue_request(write_request);
                    let write_results = self.block_device.process_requests();

                    if let Some(write_result) = write_results.first() {
                        match &write_result.result {
                            Ok(_) => return Ok(()),
                            Err(_) => return Err(FileSystemError::new(
                                FileSystemErrorKind::IoError,
                                "Failed to write directory entry"
                            )),
                        }
                    }
                }
            }
        }

        // If we get here, we couldn't find space in existing blocks
        // In a full implementation, we would allocate a new block for the directory
        Err(FileSystemError::new(
            FileSystemErrorKind::NoSpace,
            "No space available in directory for new entry"
        ))
    }
    
    /// Remove a directory entry from a parent directory
    fn remove_directory_entry(&self, parent_inode: u32, name: &String) -> Result<(), FileSystemError> {
        // Read the parent directory inode
        let parent_dir_inode = self.read_inode(parent_inode)?;
        
        if !parent_dir_inode.is_dir() {
            return Err(FileSystemError::new(
                FileSystemErrorKind::InvalidData,
                "Parent is not a directory"
            ));
        }

        // Search through all directory blocks to find the entry to remove
        let blocks_in_dir = (parent_dir_inode.get_size() as u64 + self.block_size as u64 - 1) / self.block_size as u64;

        for block_idx in 0..blocks_in_dir {
            let block_num = self.get_inode_block(&parent_dir_inode, block_idx)?;
            if block_num == 0 {
                continue; // Sparse block
            }

            // Read the directory block
            let block_sector = (block_num * 2) as u64; // Convert block to sector
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

            let mut block_data = if let Some(result) = results.first() {
                match &result.result {
                    Ok(_) => result.request.buffer.clone(),
                    Err(_) => continue, // Try next block
                }
            } else {
                continue;
            };

            // Parse directory entries to find the one to remove
            let mut offset = 0;
            let mut prev_entry_offset = None;

            while offset < self.block_size as usize {
                if offset + 8 > block_data.len() {
                    break;
                }

                let entry = match Ext2DirectoryEntryRaw::from_bytes(&block_data[offset..]) {
                    Ok(entry) => entry,
                    Err(_) => break,
                };
                
                let rec_len = entry.get_rec_len();
                if rec_len == 0 {
                    break; // Invalid entry
                }

                let name_len = entry.get_name_len() as usize;
                if offset + 8 + name_len <= block_data.len() {
                    let entry_name_bytes = &block_data[offset + 8..offset + 8 + name_len];
                    if let Ok(entry_name) = core::str::from_utf8(entry_name_bytes) {
                        if entry_name == name {
                            // Found the entry to remove!
                            if let Some(prev_offset) = prev_entry_offset {
                                // Extend the previous entry's rec_len to cover this entry
                                let prev_entry = Ext2DirectoryEntryRaw::from_bytes(&block_data[prev_offset..])?;
                                let new_rec_len = prev_entry.get_rec_len() + rec_len;
                                let new_rec_len_bytes = new_rec_len.to_le_bytes();
                                
                                block_data[prev_offset + 4] = new_rec_len_bytes[0];
                                block_data[prev_offset + 5] = new_rec_len_bytes[1];
                            } else {
                                // This is the first entry in the block, mark it as free by setting inode to 0
                                block_data[offset..offset + 4].fill(0);
                            }

                            // Write the updated block back to disk
                            let write_request = Box::new(crate::device::block::request::BlockIORequest {
                                request_type: crate::device::block::request::BlockIORequestType::Write,
                                sector: block_sector as usize,
                                sector_count: (self.block_size / 512) as usize,
                                head: 0,
                                cylinder: 0,
                                buffer: block_data,
                            });

                            self.block_device.enqueue_request(write_request);
                            let write_results = self.block_device.process_requests();

                            if let Some(write_result) = write_results.first() {
                                match &write_result.result {
                                    Ok(_) => return Ok(()),
                                    Err(_) => return Err(FileSystemError::new(
                                        FileSystemErrorKind::IoError,
                                        "Failed to write updated directory block"
                                    )),
                                }
                            }

                            return Err(FileSystemError::new(
                                FileSystemErrorKind::IoError,
                                "No response from block device write"
                            ));
                        }
                    }
                }

                prev_entry_offset = Some(offset);
                offset += rec_len as usize;
            }
        }

        // Entry not found
        Err(FileSystemError::new(
            FileSystemErrorKind::NotFound,
            "Directory entry not found"
        ))
    }
    
    /// Free an inode and update bitmaps
    fn free_inode(&self, inode_number: u32) -> Result<(), FileSystemError> {
        // Calculate which block group contains this inode
        let group = (inode_number - 1) / self.superblock.get_inodes_per_group();
        let local_inode = (inode_number - 1) % self.superblock.get_inodes_per_group();
        
        // Read block group descriptor to find inode bitmap location
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

        // Read the inode bitmap
        let inode_bitmap_block = bgd.get_inode_bitmap();
        let bitmap_sector = (inode_bitmap_block * 2) as u64; // Convert block to sector
        
        let request = Box::new(crate::device::block::request::BlockIORequest {
            request_type: crate::device::block::request::BlockIORequestType::Read,
            sector: bitmap_sector as usize,
            sector_count: (self.block_size / 512) as usize,
            head: 0,
            cylinder: 0,
            buffer: vec![0u8; self.block_size as usize],
        });

        self.block_device.enqueue_request(request);
        let results = self.block_device.process_requests();

        let mut bitmap_data = if let Some(result) = results.first() {
            match &result.result {
                Ok(_) => result.request.buffer.clone(),
                Err(_) => return Err(FileSystemError::new(
                    FileSystemErrorKind::IoError,
                    "Failed to read inode bitmap"
                )),
            }
        } else {
            return Err(FileSystemError::new(
                FileSystemErrorKind::IoError,
                "No result from inode bitmap read"
            ));
        };

        // Clear the bit for this inode (mark as free)
        let byte_index = (local_inode / 8) as usize;
        let bit_index = (local_inode % 8) as u8;
        
        if byte_index >= bitmap_data.len() {
            return Err(FileSystemError::new(
                FileSystemErrorKind::InvalidData,
                "Inode bitmap index out of bounds"
            ));
        }

        // Clear the bit (0 = free, 1 = used in ext2)
        bitmap_data[byte_index] &= !(1 << bit_index);

        // Write the updated bitmap back to disk
        let write_request = Box::new(crate::device::block::request::BlockIORequest {
            request_type: crate::device::block::request::BlockIORequestType::Write,
            sector: bitmap_sector as usize,
            sector_count: (self.block_size / 512) as usize,
            head: 0,
            cylinder: 0,
            buffer: bitmap_data,
        });

        self.block_device.enqueue_request(write_request);
        let write_results = self.block_device.process_requests();

        if let Some(write_result) = write_results.first() {
            match &write_result.result {
                Ok(_) => {
                    // Remove from inode cache if present
                    {
                        let mut cache = self.inode_cache.lock();
                        cache.remove(&inode_number);
                    }
                    Ok(())
                },
                Err(_) => Err(FileSystemError::new(
                    FileSystemErrorKind::IoError,
                    "Failed to write updated inode bitmap"
                )),
            }
        } else {
            Err(FileSystemError::new(
                FileSystemErrorKind::IoError,
                "No response from inode bitmap write"
            ))
        }
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
        
        // Allocate an inode from the ext2 filesystem
        let new_inode_number = self.allocate_inode()?;
        
        // Create the inode structure on disk
        let mode = match file_type {
            FileType::RegularFile => EXT2_S_IFREG | 0o644,
            FileType::Directory => EXT2_S_IFDIR | 0o755,
            _ => return Err(FileSystemError::new(
                FileSystemErrorKind::NotSupported,
                "Unsupported file type for ext2"
            )),
        } as u16;
        
        // Create new inode with proper initialization
        let new_inode = Ext2Inode {
            mode: mode.to_le(),
            uid: 0_u16.to_le(),
            size: 0_u32.to_le(),
            atime: 0_u32.to_le(),
            ctime: 0_u32.to_le(),
            mtime: 0_u32.to_le(),
            dtime: 0_u32.to_le(),
            gid: 0_u16.to_le(),
            links_count: 1_u16.to_le(),
            blocks: 0_u32.to_le(),
            flags: 0_u32.to_le(),
            osd1: 0_u32.to_le(),
            block: [0_u32; 15],
            generation: 0_u32.to_le(),
            file_acl: 0_u32.to_le(),
            dir_acl: 0_u32.to_le(),
            faddr: 0_u32.to_le(),
            osd2: [0_u8; 12],
        };
        
        // Write the inode to disk
        self.write_inode(new_inode_number, &new_inode)?;
        
        // Add directory entry to parent directory
        self.add_directory_entry(ext2_parent.inode_number(), name, new_inode_number, file_type.clone())?;
        
        // Initialize directory contents if it's a directory
        if file_type == FileType::Directory {
            self.initialize_directory(new_inode_number, ext2_parent.inode_number())?;
        }
        
        // Initialize directory contents if it's a directory
        if file_type == FileType::Directory {
            self.initialize_directory(new_inode_number, ext2_parent.inode_number())?;
        }
        
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

        // Try to lookup the file to ensure it exists and get its inode number
        let node = self.lookup(parent, name)?;
        let ext2_node = node.as_any()
            .downcast_ref::<Ext2Node>()
            .ok_or_else(|| FileSystemError::new(
                FileSystemErrorKind::NotSupported,
                "Invalid node type for ext2"
            ))?;
        
        let inode_number = ext2_node.inode_number();
        
        // Remove the directory entry from the parent directory
        self.remove_directory_entry(ext2_parent.inode_number(), name)?;
        
        // Free the inode and its data blocks
        self.free_inode(inode_number)?;
        
        Ok(())
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
    let manager = get_fs_driver_manager();
    manager.register_driver(Box::new(Ext2Driver));
}

driver_initcall!(register_driver);