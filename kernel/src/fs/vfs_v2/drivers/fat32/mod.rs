//! FAT32 Filesystem Implementation
//!
//! This module implements a FAT32 filesystem driver for the VFS v2 architecture.
//! It provides support for reading and writing FAT32 filesystems on block devices,
//! particularly designed to work with virtio-blk devices.
//!
//! ## Features
//!
//! - Full FAT32 filesystem support
//! - Read and write operations
//! - Directory navigation
//! - File creation, deletion, and modification
//! - Integration with VFS v2 architecture
//! - Block device compatibility
//!
//! ## Architecture
//!
//! The FAT32 implementation consists of:
//! - `Fat32FileSystem`: Main filesystem implementation
//! - `Fat32Node`: VFS node implementation for files and directories
//! - `Fat32Driver`: Filesystem driver for registration
//! - Data structures for FAT32 format (boot sector, directory entries, etc.)

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

use super::super::core::{VfsNode, FileSystemOperations, DirectoryEntryInternal};

pub mod structures;
pub mod node;
pub mod driver;

#[cfg(test)]
pub mod tests;

pub use structures::*;
pub use node::{Fat32Node, Fat32FileObject, Fat32DirectoryObject};
pub use driver::Fat32Driver;

/// FAT32 Filesystem implementation
///
/// This struct implements a FAT32 filesystem that can be mounted on block devices.
/// It maintains the block device reference and provides filesystem operations
/// through the VFS v2 interface.
pub struct Fat32FileSystem {
    /// Reference to the underlying block device
    block_device: Arc<dyn BlockDevice>,
    /// Boot sector information
    boot_sector: Fat32BootSector,
    /// Root directory cluster
    root_cluster: u32,
    /// Sectors per cluster
    sectors_per_cluster: u32,
    /// Bytes per sector
    bytes_per_sector: u32,
    /// Root directory node
    root: RwLock<Arc<Fat32Node>>,
    /// Filesystem name
    name: String,
    /// Next file ID generator
    next_file_id: Mutex<u64>,
    /// Cached FAT table entries
    fat_cache: Mutex<BTreeMap<u32, u32>>,
}

impl Debug for Fat32FileSystem {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Fat32FileSystem")
            .field("name", &self.name)
            .field("root_cluster", &self.root_cluster)
            .field("sectors_per_cluster", &self.sectors_per_cluster)
            .field("bytes_per_sector", &self.bytes_per_sector)
            .finish()
    }
}

impl Fat32FileSystem {
    /// Create a new FAT32 filesystem from a block device
    pub fn new(block_device: Arc<dyn BlockDevice>) -> Result<Arc<Self>, FileSystemError> {
        // Read boot sector
        let boot_sector = Self::read_boot_sector(&*block_device)?;
        
        // Validate FAT32 filesystem
        Self::validate_fat32(&boot_sector)?;
        
        // Calculate filesystem parameters
        let sectors_per_cluster = boot_sector.sectors_per_cluster as u32;
        let bytes_per_sector = boot_sector.bytes_per_sector as u32;
        let root_cluster = boot_sector.root_cluster;
        
        // Create root directory node
        let root = Arc::new(Fat32Node::new_directory("/".to_string(), 1, root_cluster));
        
        let fs = Arc::new(Self {
            block_device,
            boot_sector,
            root_cluster,
            sectors_per_cluster,
            bytes_per_sector,
            root: RwLock::new(Arc::clone(&root)),
            name: "fat32".to_string(),
            next_file_id: Mutex::new(2), // Start from 2, root is 1
            fat_cache: Mutex::new(BTreeMap::new()),
        });
        
        // Set filesystem reference in root node
        let fs_weak = Arc::downgrade(&(fs.clone() as Arc<dyn FileSystemOperations>));
        root.set_filesystem(fs_weak);
        
        Ok(fs)
    }
    
    /// Read boot sector from block device
    fn read_boot_sector(block_device: &dyn BlockDevice) -> Result<Fat32BootSector, FileSystemError> {
        // Create read request for sector 0 (boot sector)
        let request = Box::new(crate::device::block::request::BlockIORequest {
            request_type: crate::device::block::request::BlockIORequestType::Read,
            sector: 0,
            sector_count: 1,
            head: 0,
            cylinder: 0,
            buffer: vec![0u8; 512], // Boot sector is always 512 bytes
        });
        
        block_device.enqueue_request(request);
        let results = block_device.process_requests();
        
        if let Some(result) = results.first() {
            match &result.result {
                Ok(_) => {
                    // Parse boot sector
                    if result.request.buffer.len() < mem::size_of::<Fat32BootSector>() {
                        return Err(FileSystemError::new(
                            FileSystemErrorKind::IoError,
                            "Boot sector read incomplete"
                        ));
                    }
                    
                    // Convert bytes to boot sector structure
                    let boot_sector = unsafe {
                        core::ptr::read(result.request.buffer.as_ptr() as *const Fat32BootSector)
                    };
                    
                    Ok(boot_sector)
                },
                Err(e) => {
                    Err(FileSystemError::new(
                        FileSystemErrorKind::IoError,
                        format!("Failed to read boot sector: {}", e)
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
    
    /// Validate that this is a FAT32 filesystem
    fn validate_fat32(boot_sector: &Fat32BootSector) -> Result<(), FileSystemError> {
        // Check signature
        if boot_sector.signature != 0xAA55 {
            return Err(FileSystemError::new(
                FileSystemErrorKind::InvalidData,
                "Invalid boot sector signature"
            ));
        }
        
        // Check bytes per sector (must be 512, 1024, 2048, or 4096)
        match boot_sector.bytes_per_sector {
            512 | 1024 | 2048 | 4096 => {},
            _ => return Err(FileSystemError::new(
                FileSystemErrorKind::InvalidData,
                "Invalid bytes per sector"
            ))
        }
        
        // Check sectors per cluster (must be power of 2)
        if boot_sector.sectors_per_cluster == 0 || 
           (boot_sector.sectors_per_cluster & (boot_sector.sectors_per_cluster - 1)) != 0 {
            return Err(FileSystemError::new(
                FileSystemErrorKind::InvalidData,
                "Invalid sectors per cluster"
            ));
        }
        
        Ok(())
    }
    
    /// Generate next unique file ID
    fn generate_file_id(&self) -> u64 {
        let mut next_id = self.next_file_id.lock();
        let id = *next_id;
        *next_id += 1;
        id
    }
    
    /// Read cluster data from the block device
    fn read_cluster(&self, cluster: u32) -> Result<Vec<u8>, FileSystemError> {
        if cluster < 2 {
            return Err(FileSystemError::new(
                FileSystemErrorKind::InvalidData,
                "Invalid cluster number"
            ));
        }
        
        // Calculate sector number for this cluster
        let first_data_sector = self.boot_sector.reserved_sectors as u32 +
            (self.boot_sector.fat_count as u32 * self.boot_sector.sectors_per_fat);
        let cluster_sector = first_data_sector + (cluster - 2) * self.sectors_per_cluster;
        
        // Read cluster data
        let cluster_size = (self.sectors_per_cluster * self.bytes_per_sector) as usize;
        let mut buffer = vec![0u8; cluster_size];
        
        for i in 0..self.sectors_per_cluster {
            let request = Box::new(crate::device::block::request::BlockIORequest {
                request_type: crate::device::block::request::BlockIORequestType::Read,
                sector: (cluster_sector + i) as usize,
                sector_count: 1,
                head: 0,
                cylinder: 0,
                buffer: vec![0u8; self.bytes_per_sector as usize],
            });
            
            self.block_device.enqueue_request(request);
            let results = self.block_device.process_requests();
            
            if let Some(result) = results.first() {
                match &result.result {
                    Ok(_) => {
                        let start_offset = (i * self.bytes_per_sector) as usize;
                        let end_offset = start_offset + self.bytes_per_sector as usize;
                        buffer[start_offset..end_offset].copy_from_slice(&result.request.buffer);
                    },
                    Err(e) => {
                        return Err(FileSystemError::new(
                            FileSystemErrorKind::IoError,
                            format!("Failed to read cluster sector: {}", e)
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
        
        Ok(buffer)
    }
    
    /// Write cluster data to the block device
    fn write_cluster(&self, cluster: u32, data: &[u8]) -> Result<(), FileSystemError> {
        if cluster < 2 {
            return Err(FileSystemError::new(
                FileSystemErrorKind::InvalidData,
                "Invalid cluster number"
            ));
        }
        
        // Calculate sector number for this cluster
        let first_data_sector = self.boot_sector.reserved_sectors as u32 +
            (self.boot_sector.fat_count as u32 * self.boot_sector.sectors_per_fat);
        let cluster_sector = first_data_sector + (cluster - 2) * self.sectors_per_cluster;
        
        let cluster_size = (self.sectors_per_cluster * self.bytes_per_sector) as usize;
        
        // Ensure data doesn't exceed cluster size
        if data.len() > cluster_size {
            return Err(FileSystemError::new(
                FileSystemErrorKind::InvalidData,
                "Data exceeds cluster size"
            ));
        }
        
        // Prepare full cluster data (pad with zeros if needed)
        let mut cluster_data = vec![0u8; cluster_size];
        cluster_data[..data.len()].copy_from_slice(data);
        
        // Write cluster data sector by sector
        for i in 0..self.sectors_per_cluster {
            let start_offset = (i * self.bytes_per_sector) as usize;
            let end_offset = start_offset + self.bytes_per_sector as usize;
            let sector_data = cluster_data[start_offset..end_offset].to_vec();
            
            let request = Box::new(crate::device::block::request::BlockIORequest {
                request_type: crate::device::block::request::BlockIORequestType::Write,
                sector: (cluster_sector + i) as usize,
                sector_count: 1,
                head: 0,
                cylinder: 0,
                buffer: sector_data,
            });
            
            self.block_device.enqueue_request(request);
            let results = self.block_device.process_requests();
            
            if let Some(result) = results.first() {
                if let Err(e) = &result.result {
                    return Err(FileSystemError::new(
                        FileSystemErrorKind::IoError,
                        format!("Failed to write cluster sector: {}", e)
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
    
    /// Read FAT entry for a given cluster
    fn read_fat_entry(&self, cluster: u32) -> Result<u32, FileSystemError> {
        // Check for cached entry first
        {
            let cache = self.fat_cache.lock();
            if let Some(&entry) = cache.get(&cluster) {
                return Ok(entry);
            }
        }
        
        // Calculate FAT offset and sector
        let fat_offset = cluster * 4; // FAT32 uses 4 bytes per entry
        let fat_sector = self.boot_sector.reserved_sectors as u32 + (fat_offset / self.bytes_per_sector);
        let sector_offset = (fat_offset % self.bytes_per_sector) as usize;
        
        // Read FAT sector
        let request = Box::new(crate::device::block::request::BlockIORequest {
            request_type: crate::device::block::request::BlockIORequestType::Read,
            sector: fat_sector as usize,
            sector_count: 1,
            head: 0,
            cylinder: 0,
            buffer: vec![0u8; self.bytes_per_sector as usize],
        });
        
        self.block_device.enqueue_request(request);
        let results = self.block_device.process_requests();
        
        if let Some(result) = results.first() {
            match &result.result {
                Ok(_) => {
                    // Extract 32-bit FAT entry (only lower 28 bits are used in FAT32)
                    if sector_offset + 4 > result.request.buffer.len() {
                        return Err(FileSystemError::new(
                            FileSystemErrorKind::IoError,
                            "FAT entry spans sector boundary"
                        ));
                    }
                    
                    let fat_entry = u32::from_le_bytes([
                        result.request.buffer[sector_offset],
                        result.request.buffer[sector_offset + 1],
                        result.request.buffer[sector_offset + 2],
                        result.request.buffer[sector_offset + 3],
                    ]) & 0x0FFFFFFF; // Mask to 28 bits for FAT32
                    
                    // Cache the entry
                    {
                        let mut cache = self.fat_cache.lock();
                        cache.insert(cluster, fat_entry);
                    }
                    
                    Ok(fat_entry)
                },
                Err(e) => {
                    Err(FileSystemError::new(
                        FileSystemErrorKind::IoError,
                        format!("Failed to read FAT sector: {}", e)
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
    
    /// Write FAT entry for a given cluster
    fn write_fat_entry(&self, cluster: u32, value: u32) -> Result<(), FileSystemError> {
        // Calculate FAT offset and sector
        let fat_offset = cluster * 4; // FAT32 uses 4 bytes per entry
        let fat_sector = self.boot_sector.reserved_sectors as u32 + (fat_offset / self.bytes_per_sector);
        let sector_offset = (fat_offset % self.bytes_per_sector) as usize;
        
        // Read current FAT sector
        let mut request = Box::new(crate::device::block::request::BlockIORequest {
            request_type: crate::device::block::request::BlockIORequestType::Read,
            sector: fat_sector as usize,
            sector_count: 1,
            head: 0,
            cylinder: 0,
            buffer: vec![0u8; self.bytes_per_sector as usize],
        });
        
        self.block_device.enqueue_request(request);
        let results = self.block_device.process_requests();
        
        let mut sector_buffer = if let Some(result) = results.first() {
            match &result.result {
                Ok(_) => result.request.buffer.clone(),
                Err(e) => {
                    return Err(FileSystemError::new(
                        FileSystemErrorKind::IoError,
                        format!("Failed to read FAT sector: {}", e)
                    ));
                }
            }
        } else {
            return Err(FileSystemError::new(
                FileSystemErrorKind::IoError,
                "No result from block device"
            ));
        };
        
        // Update FAT entry (preserve upper 4 bits, update lower 28 bits)
        if sector_offset + 4 > sector_buffer.len() {
            return Err(FileSystemError::new(
                FileSystemErrorKind::IoError,
                "FAT entry spans sector boundary"
            ));
        }
        
        let current_entry = u32::from_le_bytes([
            sector_buffer[sector_offset],
            sector_buffer[sector_offset + 1],
            sector_buffer[sector_offset + 2],
            sector_buffer[sector_offset + 3],
        ]);
        
        let new_entry = (current_entry & 0xF0000000) | (value & 0x0FFFFFFF);
        let new_bytes = new_entry.to_le_bytes();
        
        sector_buffer[sector_offset..sector_offset + 4].copy_from_slice(&new_bytes);
        
        // Write updated sector back
        let write_request = Box::new(crate::device::block::request::BlockIORequest {
            request_type: crate::device::block::request::BlockIORequestType::Write,
            sector: fat_sector as usize,
            sector_count: 1,
            head: 0,
            cylinder: 0,
            buffer: sector_buffer,
        });
        
        self.block_device.enqueue_request(write_request);
        let write_results = self.block_device.process_requests();
        
        if let Some(result) = write_results.first() {
            match &result.result {
                Ok(_) => {
                    // Update cache
                    {
                        let mut cache = self.fat_cache.lock();
                        cache.insert(cluster, value);
                    }
                    Ok(())
                },
                Err(e) => {
                    Err(FileSystemError::new(
                        FileSystemErrorKind::IoError,
                        format!("Failed to write FAT sector: {}", e)
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
    
    /// Read file content by following cluster chain
    pub fn read_file_content(&self, start_cluster: u32, size: usize) -> Result<Vec<u8>, FileSystemError> {
        if start_cluster < 2 {
            return Ok(Vec::new()); // Empty file
        }
        
        let mut content = Vec::new();
        let mut current_cluster = start_cluster;
        let cluster_size = (self.sectors_per_cluster * self.bytes_per_sector) as usize;
        
        loop {
            // Read current cluster
            let cluster_data = self.read_cluster(current_cluster)?;
            
            // Add data to content (up to requested size)
            let remaining_size = size.saturating_sub(content.len());
            if remaining_size == 0 {
                break;
            }
            
            let bytes_to_copy = core::cmp::min(cluster_data.len(), remaining_size);
            content.extend_from_slice(&cluster_data[..bytes_to_copy]);
            
            // Check if we've read enough
            if content.len() >= size {
                break;
            }
            
            // Get next cluster from FAT
            let fat_entry = self.read_fat_entry(current_cluster)?;
            
            // Check for end of chain
            if fat_entry >= 0x0FFFFFF8 {
                break; // End of file
            }
            
            current_cluster = fat_entry;
        }
        
        // Truncate to exact size if needed
        content.truncate(size);
        Ok(content)
    }
    
    /// Write file content by allocating and writing to clusters
    pub fn write_file_content(&self, start_cluster: u32, data: &[u8]) -> Result<u32, FileSystemError> {
        let cluster_size = (self.sectors_per_cluster * self.bytes_per_sector) as usize;
        let clusters_needed = (data.len() + cluster_size - 1) / cluster_size;
        
        if clusters_needed == 0 {
            return Ok(start_cluster);
        }
        
        let mut current_cluster = start_cluster;
        let mut data_offset = 0;
        
        for i in 0..clusters_needed {
            let data_end = core::cmp::min(data_offset + cluster_size, data.len());
            let chunk = &data[data_offset..data_end];
            
            // Write data to current cluster
            self.write_cluster(current_cluster, chunk)?;
            
            data_offset = data_end;
            
            // If this is the last cluster, mark end of chain
            if i == clusters_needed - 1 {
                self.write_fat_entry(current_cluster, 0x0FFFFFFF)?; // End of chain
                break;
            }
            
            // Allocate next cluster if needed
            // For now, use a simple sequential allocation
            let next_cluster = current_cluster + 1;
            self.write_fat_entry(current_cluster, next_cluster)?;
            current_cluster = next_cluster;
        }
        
        Ok(start_cluster)
    }
}

impl FileSystemOperations for Fat32FileSystem {
    fn lookup(&self, parent: &Arc<dyn VfsNode>, name: &String) -> Result<Arc<dyn VfsNode>, FileSystemError> {
        let fat32_parent = parent.as_any()
            .downcast_ref::<Fat32Node>()
            .ok_or_else(|| FileSystemError::new(
                FileSystemErrorKind::NotSupported,
                "Invalid node type for FAT32"
            ))?;
        
        // Check if it's a directory
        match fat32_parent.file_type() {
            Ok(FileType::Directory) => {},
            Ok(_) => return Err(FileSystemError::new(
                FileSystemErrorKind::NotADirectory,
                "Parent is not a directory"
            )),
            Err(e) => return Err(e),
        }
        
        // Look for the file in the parent directory
        let children = fat32_parent.children.read();
        if let Some(child) = children.get(name) {
            Ok(Arc::clone(child))
        } else {
            Err(FileSystemError::new(
                FileSystemErrorKind::NotFound,
                format!("File '{}' not found", name)
            ))
        }
    }
    
    fn open(&self, node: &Arc<dyn VfsNode>, _flags: u32) -> Result<Arc<dyn FileObject>, FileSystemError> {
        let fat32_node = node.as_any()
            .downcast_ref::<Fat32Node>()
            .ok_or_else(|| FileSystemError::new(
                FileSystemErrorKind::NotSupported,
                "Invalid node type for FAT32"
            ))?;
        
        match fat32_node.file_type() {
            Ok(FileType::RegularFile) => {
                Ok(Arc::new(Fat32FileObject::new(Arc::new(fat32_node.clone()))))
            },
            Ok(FileType::Directory) => {
                Ok(Arc::new(Fat32DirectoryObject::new(Arc::new(fat32_node.clone()))))
            },
            Ok(_) => Err(FileSystemError::new(
                FileSystemErrorKind::NotSupported,
                "Unsupported file type"
            )),
            Err(e) => Err(e),
        }
    }
    
    fn create(&self, parent: &Arc<dyn VfsNode>, name: &String, file_type: FileType, _mode: u32) -> Result<Arc<dyn VfsNode>, FileSystemError> {
        let fat32_parent = parent.as_any()
            .downcast_ref::<Fat32Node>()
            .ok_or_else(|| FileSystemError::new(
                FileSystemErrorKind::NotSupported,
                "Invalid node type for FAT32"
            ))?;
        
        // Check if it's a directory
        match fat32_parent.file_type() {
            Ok(FileType::Directory) => {},
            Ok(_) => return Err(FileSystemError::new(
                FileSystemErrorKind::NotADirectory,
                "Parent is not a directory"
            )),
            Err(e) => return Err(e),
        }
        
        // Check if file already exists
        {
            let children = fat32_parent.children.read();
            if children.contains_key(name) {
                return Err(FileSystemError::new(
                    FileSystemErrorKind::AlreadyExists,
                    format!("File '{}' already exists", name)
                ));
            }
        }
        
        // Create new node
        let file_id = self.generate_file_id();
        let new_node = match file_type {
            FileType::RegularFile => {
                Arc::new(Fat32Node::new_file(name.clone(), file_id, 0)) // No cluster allocated yet
            },
            FileType::Directory => {
                Arc::new(Fat32Node::new_directory(name.clone(), file_id, 0)) // No cluster allocated yet
            },
            _ => {
                return Err(FileSystemError::new(
                    FileSystemErrorKind::NotSupported,
                    "Unsupported file type for FAT32"
                ));
            }
        };
        
        // Set filesystem reference using the parent's filesystem
        if let Some(fs) = fat32_parent.filesystem() {
            if let Some(fs_strong) = fs.upgrade() {
                let fs_weak = Arc::downgrade(&fs_strong);
                new_node.set_filesystem(fs_weak);
            }
        }
        
        // Add to parent directory
        {
            let mut children = fat32_parent.children.write();
            children.insert(name.clone(), Arc::clone(&new_node) as Arc<dyn VfsNode>);
        }
        
        Ok(new_node as Arc<dyn VfsNode>)
    }
    
    fn remove(&self, parent: &Arc<dyn VfsNode>, name: &String) -> Result<(), FileSystemError> {
        let fat32_parent = parent.as_any()
            .downcast_ref::<Fat32Node>()
            .ok_or_else(|| FileSystemError::new(
                FileSystemErrorKind::NotSupported,
                "Invalid node type for FAT32"
            ))?;
        
        // Check if it's a directory
        match fat32_parent.file_type() {
            Ok(FileType::Directory) => {},
            Ok(_) => return Err(FileSystemError::new(
                FileSystemErrorKind::NotADirectory,
                "Parent is not a directory"
            )),
            Err(e) => return Err(e),
        }
        
        // Remove from parent directory
        {
            let mut children = fat32_parent.children.write();
            if children.remove(name).is_none() {
                return Err(FileSystemError::new(
                    FileSystemErrorKind::NotFound,
                    format!("File '{}' not found", name)
                ));
            }
        }
        
        Ok(())
    }
    
    fn readdir(&self, node: &Arc<dyn VfsNode>) -> Result<Vec<DirectoryEntryInternal>, FileSystemError> {
        let fat32_node = node.as_any()
            .downcast_ref::<Fat32Node>()
            .ok_or_else(|| FileSystemError::new(
                FileSystemErrorKind::NotSupported,
                "Invalid node type for FAT32"
            ))?;
        
        // Check if it's a directory
        match fat32_node.file_type() {
            Ok(FileType::Directory) => {},
            Ok(_) => return Err(FileSystemError::new(
                FileSystemErrorKind::NotADirectory,
                "Not a directory"
            )),
            Err(e) => return Err(e),
        }
        
        let mut entries = Vec::new();
        let children = fat32_node.children.read();
        
        for (name, child_node) in children.iter() {
            if let Some(child_fat32_node) = child_node.as_any().downcast_ref::<Fat32Node>() {
                let metadata = child_fat32_node.metadata.read();
                entries.push(DirectoryEntryInternal {
                    name: name.clone(),
                    file_type: child_fat32_node.file_type.read().clone(),
                    file_id: metadata.file_id,
                });
            }
        }
        
        Ok(entries)
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

/// Register the FAT32 driver with the filesystem driver manager
fn register_driver() {
    let fs_driver_manager = get_fs_driver_manager();
    fs_driver_manager.register_driver(Box::new(Fat32Driver));
}

driver_initcall!(register_driver);