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
    
    /// Lookup a specific file in a directory cluster
    fn lookup_file_in_directory(&self, cluster: u32, target_name: &str) -> Result<Fat32DirectoryEntryInternal, FileSystemError> {
        let mut current_cluster = cluster;
        
        loop {
            // Read the current cluster
            let cluster_data = self.read_cluster_data(current_cluster)?;
            
            // Parse directory entries in this cluster
            let entries_per_cluster = (self.sectors_per_cluster * self.bytes_per_sector) / 32;
            let mut lfn_parts: Vec<String> = Vec::new(); // Collect LFN parts in order
            
            for i in 0..entries_per_cluster {
                let offset = (i * 32) as usize;
                if offset + 32 > cluster_data.len() {
                    break;
                }
                
                let entry_bytes = &cluster_data[offset..offset + 32];
                
                // Safety: We know the slice is exactly 32 bytes (size of Fat32DirectoryEntry)
                let dir_entry = unsafe {
                    core::ptr::read(entry_bytes.as_ptr() as *const structures::Fat32DirectoryEntry)
                };
                
                // Skip free entries
                if dir_entry.is_free() {
                    lfn_parts.clear(); // Reset LFN accumulation
                    continue;
                }
                
                // Handle LFN entries
                if dir_entry.is_long_filename() {
                    // Cast to LFN entry
                    let lfn_entry = unsafe { &*(entry_bytes.as_ptr() as *const structures::Fat32LFNEntry) };
                    
                    // Extract characters from this LFN entry
                    let chars = lfn_entry.extract_chars();
                    
                    // Convert UTF-16 to UTF-8
                    let mut part = String::new();
                    for &ch in &chars {
                        if ch == 0 || ch == 0xFFFF {
                            break; // End of string or padding
                        }
                        if let Some(c) = char::from_u32(ch as u32) {
                            part.push(c);
                        }
                    }
                    
                    // LFN entries are stored with highest sequence number first
                    if lfn_entry.is_last_lfn() {
                        lfn_parts.clear(); // Start fresh for new LFN sequence
                        lfn_parts.push(part); // Add this part to the end
                    } else {
                        // Add at the end, we'll reverse the entire collection later
                        lfn_parts.push(part);
                    }
                    continue;
                }
                
                // Skip dot entries
                if dir_entry.name[0] == b'.' {
                    lfn_parts.clear();
                    continue;
                }
                
                // Create internal entry
                let mut internal_entry = Fat32DirectoryEntryInternal::from_raw(dir_entry);
                
                // Assemble complete LFN if available
                if !lfn_parts.is_empty() {
                    // Reverse the parts since LFN entries are stored in reverse order
                    lfn_parts.reverse();
                    let long_filename = lfn_parts.join("");
                    internal_entry.set_long_filename(long_filename);
                }
                
                let filename = internal_entry.name();
                
                // FAT32 is case-insensitive, so compare in uppercase
                if filename.to_uppercase() == target_name.to_uppercase() {
                    return Ok(internal_entry);
                }
                
                // Clear LFN for next entry
                lfn_parts.clear();
            }
            
            // Get next cluster in the chain
            let next_cluster = self.read_fat_entry(current_cluster)?;
            if next_cluster >= 0x0FFFFFF8 {
                // End of cluster chain
                break;
            }
            current_cluster = next_cluster;
        }
        
        Err(FileSystemError::new(
            FileSystemErrorKind::NotFound,
            &format!("File '{}' not found", target_name),
        ))
    }
    
    /// Read complete cluster data
    fn read_cluster_data(&self, cluster: u32) -> Result<Vec<u8>, FileSystemError> {
        let cluster_size = (self.sectors_per_cluster * self.bytes_per_sector) as usize;
        let cluster_sector = self.cluster_to_sector(cluster);
        
        // Batch read all sectors in the cluster
        let mut requests = Vec::new();
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
            requests.push(());
        }
        
        // Process all requests in batch
        let results = self.block_device.process_requests();
        
        if results.len() != requests.len() {
            return Err(FileSystemError::new(
                FileSystemErrorKind::IoError,
                format!("Expected {} results, got {}", requests.len(), results.len())
            ));
        }
        
        // Assemble cluster data from results
        let mut cluster_data = vec![0u8; cluster_size];
        for (i, result) in results.iter().enumerate() {
            match &result.result {
                Ok(_) => {
                    let start_offset = i * self.bytes_per_sector as usize;
                    let end_offset = start_offset + self.bytes_per_sector as usize;
                    cluster_data[start_offset..end_offset].copy_from_slice(&result.request.buffer);
                },
                Err(e) => {
                    return Err(FileSystemError::new(
                        FileSystemErrorKind::IoError,
                        format!("Failed to read cluster sector {}: {}", i, e)
                    ));
                }
            }
        }
        
        Ok(cluster_data)
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
        
        // Read all sectors of the cluster
        for i in 0..self.sectors_per_cluster {
            let sector_buffer = vec![0u8; self.bytes_per_sector as usize];
            let request = Box::new(crate::device::block::request::BlockIORequest {
                request_type: crate::device::block::request::BlockIORequestType::Read,
                sector: (cluster_sector + i) as usize,
                sector_count: 1,
                head: 0,
                cylinder: 0,
                buffer: sector_buffer,
            });
            
            self.block_device.enqueue_request(request);
            let results = self.block_device.process_requests();
            
            if let Some(result) = results.first() {
                match &result.result {
                    Ok(_) => {
                        let start_offset = (i * self.bytes_per_sector) as usize;
                        let end_offset = start_offset + self.bytes_per_sector as usize;
                        if end_offset <= buffer.len() && result.request.buffer.len() >= self.bytes_per_sector as usize {
                            buffer[start_offset..end_offset].copy_from_slice(&result.request.buffer[..self.bytes_per_sector as usize]);
                        }
                    },
                    Err(e) => {
                        return Err(FileSystemError::new(
                            FileSystemErrorKind::IoError,
                            format!("Failed to read cluster {} sector {}: {}", cluster, cluster_sector + i, e)
                        ));
                    }
                }
            } else {
                return Err(FileSystemError::new(
                    FileSystemErrorKind::IoError,
                    format!("No result from block device for cluster {} sector {}", cluster, cluster_sector + i)
                ));
            }
        }
        
        Ok(buffer)
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
        #[cfg(test)]
        {
            // use crate::early_println;
            // early_println!("[FAT32] read_file_content: start_cluster={}, size={}", start_cluster, size);
        }
        
        if start_cluster < 2 {
            return Ok(Vec::new()); // Empty file
        }
        
        let mut content = Vec::new();
        let mut current_cluster = start_cluster;
        let cluster_size = (self.sectors_per_cluster * self.bytes_per_sector) as usize;
        
        loop {                #[cfg(test)]
                {
                    // use crate::early_println;
                    // early_println!("[FAT32] reading cluster {}", current_cluster);
                }
            
            // Read current cluster
            let cluster_data = self.read_cluster(current_cluster)?;
            
            #[cfg(test)]
            {
                use crate::early_println;
                // early_println!("[FAT32] read cluster {} data: {} bytes, first 8 bytes: {:?}", 
                //     current_cluster, cluster_data.len(), 
                //     &cluster_data[..core::cmp::min(8, cluster_data.len())]);
            }
            
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
    
    /// Write file content to disk and return the starting cluster
    pub fn write_file_content(&self, current_cluster: u32, content: &[u8]) -> Result<u32, FileSystemError> {
        // Debug output for large file operations
        #[cfg(test)]
        {
            // use crate::early_println;
            // early_println!("[FAT32] write_file_content: cluster={}, content_len={}", current_cluster, content.len());
        }
        
        // If content is empty, free the cluster chain
        if content.is_empty() {
            if current_cluster != 0 {
                self.free_cluster_chain(current_cluster)?;
            }
            return Ok(0);
        }
        
        let cluster_size = (self.sectors_per_cluster * self.bytes_per_sector) as usize;
        let clusters_needed = (content.len() + cluster_size - 1) / cluster_size;
        
        // Debug output for allocation
        #[cfg(test)]
        {
            use crate::early_println;
            // early_println!("[FAT32] clusters_needed={}, cluster_size={}", clusters_needed, cluster_size);
        }
        
        // Free existing chain if we're overwriting
        if current_cluster != 0 {
            #[cfg(test)]
            {
                use crate::early_println;
                // early_println!("[FAT32] freeing existing cluster chain starting from cluster {}", current_cluster);
            }
            self.free_cluster_chain(current_cluster)?;
            #[cfg(test)]
            {
                use crate::early_println;
                // early_println!("[FAT32] finished freeing cluster chain");
            }
        }
        
        // Allocate new cluster chain
        let mut clusters = Vec::new();
        for cluster_index in 0..clusters_needed {
            #[cfg(test)]
            {
                use crate::early_println;
                // early_println!("[FAT32] allocating cluster {} of {}", cluster_index + 1, clusters_needed);
            }
            match self.allocate_cluster() {
                Ok(new_cluster) => {
                    #[cfg(test)]
                    {
                        use crate::early_println;
                        // early_println!("[FAT32] allocated cluster: {}", new_cluster);
                    }
                    clusters.push(new_cluster);
                }
                Err(e) => {
                    #[cfg(test)]
                    {
                        use crate::early_println;
                        early_println!("[FAT32] failed to allocate cluster {} of {}: {:?}", cluster_index + 1, clusters_needed, e);
                    }
                    return Err(e);
                }
            }
        }
        
        // Chain the clusters together in FAT
        #[cfg(test)]
        {
            use crate::early_println;
            // early_println!("[FAT32] setting up FAT chain for {} clusters", clusters.len());
        }
        for i in 0..clusters.len() - 1 {
            #[cfg(test)]
            {
                use crate::early_println;
                // early_println!("[FAT32] setting FAT entry: cluster {} -> {}", clusters[i], clusters[i + 1]);
            }
            self.write_fat_entry(clusters[i], clusters[i + 1])?;
        }
        // Mark the last cluster as end of chain
        if !clusters.is_empty() {
            #[cfg(test)]
            {
                use crate::early_println;
                // early_println!("[FAT32] marking last cluster {} as end of chain", clusters[clusters.len() - 1]);
            }
            self.write_fat_entry(clusters[clusters.len() - 1], 0x0FFFFFF8)?; // End of chain marker
        }
        
        // Write content to clusters
        #[cfg(test)]
        {
            use crate::early_println;
            // early_println!("[FAT32] writing content to {} clusters", clusters.len());
        }
        for (i, &cluster) in clusters.iter().enumerate() {
            let start_offset = i * cluster_size;
            let end_offset = core::cmp::min(start_offset + cluster_size, content.len());
            
            if start_offset < content.len() {
                let chunk = &content[start_offset..end_offset];
                
                #[cfg(test)]
                {
                    use crate::early_println;
                    // early_println!("[FAT32] writing cluster {}: {} bytes (offset {}..{})", cluster, chunk.len(), start_offset, end_offset);
                }
                
                self.write_cluster_data(cluster, chunk)?;
            }
        }
        
        #[cfg(test)]
        {
            use crate::early_println;
            // early_println!("[FAT32] write_file_content completed, start_cluster={}", clusters.first().copied().unwrap_or(0));
        }
        
        Ok(clusters.first().copied().unwrap_or(0))
    }
    
    /// Read FAT entry directly from disk without caching
    fn read_fat_entry_direct(&self, cluster: u32) -> Result<u32, FileSystemError> {
        #[cfg(test)]
        {
            use crate::early_println;
            // early_println!("[FAT32] reading FAT entry for cluster {} directly from disk", cluster);
        }
        
        // Calculate FAT sector and offset
        let fat_offset = cluster * 4; // 4 bytes per FAT32 entry
        let fat_sector = self.boot_sector.reserved_sectors as u32 + (fat_offset / self.bytes_per_sector);
        let entry_offset = (fat_offset % self.bytes_per_sector) as usize;
        
        #[cfg(test)]
        {
            use crate::early_println;
            // early_println!("[FAT32] fat_sector={}, entry_offset={}", fat_sector, entry_offset);
        }
        
        // Read FAT sector
        let buffer = vec![0u8; self.bytes_per_sector as usize];
        let request = Box::new(crate::device::block::request::BlockIORequest {
            request_type: crate::device::block::request::BlockIORequestType::Read,
            sector: fat_sector as usize,
            sector_count: 1,
            head: 0,
            cylinder: 0,
            buffer,
        });
        
        #[cfg(test)]
        {
            use crate::early_println;
            // early_println!("[FAT32] enqueuing FAT read request for sector {}", fat_sector);
        }
        
        self.block_device.enqueue_request(request);
        let results = self.block_device.process_requests();
        
        #[cfg(test)]
        {
            use crate::early_println;
            // early_println!("[FAT32] FAT read request completed, results.len()={}", results.len());
        }
        
        if results.is_empty() || results[0].result.is_err() {
            return Err(FileSystemError::new(
                FileSystemErrorKind::IoError,
                "Failed to read FAT sector"
            ));
        }
        
        // Get the buffer back from the result
        let buffer = &results[0].request.buffer;
        
        // Extract FAT entry (little-endian, mask off top 4 bits for FAT32)
        let entry = u32::from_le_bytes([
            buffer[entry_offset],
            buffer[entry_offset + 1], 
            buffer[entry_offset + 2],
            buffer[entry_offset + 3],
        ]) & 0x0FFFFFFF;
        
        #[cfg(test)]
        {
            use crate::early_println;
            // early_println!("[FAT32] read FAT entry for cluster {}: {:#x}", cluster, entry);
        }
        
        Ok(entry)
    }

    /// Allocate a free cluster from the FAT and mark it as allocated
    fn allocate_cluster(&self) -> Result<u32, FileSystemError> {
        #[cfg(test)]
        {
            use crate::early_println;
            // early_println!("[FAT32] searching for free cluster...");
        }
        
        // Simple allocation: find first free cluster starting from cluster 2
        for cluster in 2..100 { // Reduced search range for faster debugging
            #[cfg(test)]
            {
                use crate::early_println;
                // early_println!("[FAT32] checking cluster {}", cluster);
            }
            
            // Read FAT entry directly without caching to avoid stale cache issues
            let fat_entry = self.read_fat_entry_direct(cluster)?;
            
            #[cfg(test)]
            {
                use crate::early_println;
                if cluster <= 20 || fat_entry == 0 {
                    // early_println!("[FAT32] cluster {} has FAT entry: {:#x}", cluster, fat_entry);
                }
            }
            
            if fat_entry == 0 {
                #[cfg(test)]
                {
                    use crate::early_println;
                    // early_println!("[FAT32] found free cluster: {}", cluster);
                }
                // Mark as allocated immediately to prevent duplicate allocation
                self.write_fat_entry(cluster, 0x0FFFFFF8)?; // End of chain marker (will be updated later if part of chain)
                
                #[cfg(test)]
                {
                    use crate::early_println;
                    // early_println!("[FAT32] allocated cluster: {}", cluster);
                }
                return Ok(cluster);
            }
        }
        
        Err(FileSystemError::new(
            FileSystemErrorKind::NoSpace,
            "No free clusters available"
        ))
    }
    
    /// Free a cluster chain starting from the given cluster
    fn free_cluster_chain(&self, start_cluster: u32) -> Result<(), FileSystemError> {
        #[cfg(test)]
        {
            use crate::early_println;
            // early_println!("[FAT32] free_cluster_chain: starting from cluster {}", start_cluster);
        }
        
        let mut current = start_cluster;
        
        // Only process valid cluster numbers (>= 2)
        while current >= 2 && current < 0x0FFFFFF0 {
            #[cfg(test)]
            {
                use crate::early_println;
                // early_println!("[FAT32] freeing cluster {}, reading next cluster...", current);
            }
            
            let next = self.read_fat_entry(current)?;
            
            #[cfg(test)]
            {
                use crate::early_println;
                // early_println!("[FAT32] cluster {} next = {:#x}, marking as free", current, next);
            }
            
            self.write_fat_entry(current, 0)?; // Mark as free
            
            // Check if we've reached the end of chain or invalid cluster
            if next >= 0x0FFFFFF8 || next == 0 || next == 1 {
                #[cfg(test)]
                {
                    use crate::early_println;
                    // early_println!("[FAT32] reached end of chain at cluster {} (next={:#x})", current, next);
                }
                break; // End of chain or invalid next cluster
            }
            current = next;
        }
        
        #[cfg(test)]
        {
            // use crate::early_println;
            // early_println!("[FAT32] free_cluster_chain completed");
        }
        
        Ok(())
    }
    
    /// Read data to a cluster
    fn write_cluster_data(&self, cluster: u32, data: &[u8]) -> Result<(), FileSystemError> {
        // #[cfg(test)]
        // {
        //     use crate::early_println;
        //     early_println!("[FAT32] write_cluster_data: cluster={}, data_len={}, first 8 bytes: {:?}", 
        //         cluster, data.len(), &data[..core::cmp::min(8, data.len())]);
        // }
        
        if cluster < 2 {
            return Err(FileSystemError::new(
                FileSystemErrorKind::InvalidData,
                "Invalid cluster number"
            ));
        }
        
        let first_data_sector = self.boot_sector.reserved_sectors as u32 
            + (self.boot_sector.fat_count as u32 * self.boot_sector.sectors_per_fat);
        let first_sector_of_cluster = first_data_sector + (cluster - 2) * self.sectors_per_cluster;
        
        // #[cfg(test)]
        // {
        //     use crate::early_println;
        //     early_println!("[FAT32] writing cluster {} at sector {}", cluster, first_sector_of_cluster);
        // }
        
        let cluster_size = (self.sectors_per_cluster * self.bytes_per_sector) as usize;
        let mut buffer = vec![0u8; cluster_size];
        
        // Copy data to buffer, pad with zeros if necessary
        let copy_len = core::cmp::min(data.len(), cluster_size);
        buffer[..copy_len].copy_from_slice(&data[..copy_len]);
        
        // Write cluster data in batch
        let mut requests = Vec::new();
        for sector_offset in 0..self.sectors_per_cluster {
            let sector = first_sector_of_cluster + sector_offset;
            let buffer_offset = (sector_offset * self.bytes_per_sector) as usize;
            let sector_data = buffer[buffer_offset..buffer_offset + self.bytes_per_sector as usize].to_vec();
            
            let request = Box::new(crate::device::block::request::BlockIORequest {
                request_type: crate::device::block::request::BlockIORequestType::Write,
                sector: sector as usize,
                sector_count: 1,
                head: 0,
                cylinder: 0,
                buffer: sector_data,
            });
            
            self.block_device.enqueue_request(request);
            requests.push(());
        }
        
        // Process all requests in batch
        let results = self.block_device.process_requests();
        
        if results.len() != requests.len() {
            return Err(FileSystemError::new(
                FileSystemErrorKind::IoError,
                format!("Expected {} results, got {}", requests.len(), results.len())
            ));
        }
        
        // Check all results
        for (i, result) in results.iter().enumerate() {
            if result.result.is_err() {
                return Err(FileSystemError::new(
                    FileSystemErrorKind::IoError,
                    format!("Failed to write cluster {} sector {}", cluster, first_sector_of_cluster + i as u32)
                ));
            }
        }
        
        Ok(())
    }
    
    /// Convert cluster number to first sector number
    fn cluster_to_sector(&self, cluster: u32) -> u32 {
        let first_data_sector = self.boot_sector.reserved_sectors as u32 
            + (self.boot_sector.fat_count as u32 * self.boot_sector.sectors_per_fat);
        first_data_sector + (cluster - 2) * self.sectors_per_cluster
    }
    
    /// Read directory entries from a cluster
    fn read_directory_entries(&self, cluster: u32, entries: &mut Vec<Fat32DirectoryEntryInternal>) -> Result<(), FileSystemError> {
        let data = self.read_cluster_data(cluster)?;
        let entry_size = 32; // Each directory entry is 32 bytes
        let mut lfn_parts: Vec<String> = Vec::new(); // Collect LFN parts in order
        
        for chunk in data.chunks(entry_size) {
            if chunk.len() < entry_size {
                break;
            }
            
            // Check if entry is valid (not deleted and not empty)
            if chunk[0] == 0x00 {
                break; // End of directory
            }
            if chunk[0] == 0xE5 {
                lfn_parts.clear(); // Clear LFN parts on deleted entry
                continue; // Deleted entry
            }
            
            // Parse directory entry
            let attributes = chunk[11];
            
            // Handle LFN entries
            if attributes & 0x0F == 0x0F {
                // This is a LFN entry
                let lfn_entry = unsafe { &*(chunk.as_ptr() as *const structures::Fat32LFNEntry) };
                
                // Extract characters from this LFN entry
                let chars = lfn_entry.extract_chars();
                
                // Convert UTF-16 to UTF-8
                let mut part = String::new();
                for &ch in &chars {
                    if ch == 0 || ch == 0xFFFF {
                        break; // End of string or padding
                    }
                    if let Some(c) = char::from_u32(ch as u32) {
                        part.push(c);
                    }
                }
                
                // LFN entries are stored with highest sequence number first
                // We need to collect them and reverse the order when assembling
                if lfn_entry.is_last_lfn() {
                    lfn_parts.clear(); // Start fresh for new LFN sequence
                    lfn_parts.push(part); // Add this part to the end
                } else {
                    // Add at the end, we'll reverse the entire collection later
                    lfn_parts.push(part);
                }
                continue;
            }
            
            // Skip volume labels
            if attributes & 0x08 != 0 {
                lfn_parts.clear();
                continue;
            }
            
            // This is a regular SFN directory entry
            let mut name_bytes = [0u8; 11];
            name_bytes.copy_from_slice(&chunk[0..11]);
            
            let cluster = ((chunk[21] as u32) << 24) | ((chunk[20] as u32) << 16) | 
                         ((chunk[27] as u32) << 8) | (chunk[26] as u32);
            let size = u32::from_le_bytes([chunk[28], chunk[29], chunk[30], chunk[31]]);
            
            // Create Fat32DirectoryEntry structure first
            let raw_entry = structures::Fat32DirectoryEntry {
                name: name_bytes,
                attributes,
                nt_reserved: 0,
                creation_time_tenths: 0,
                creation_time: 0,
                creation_date: 0,
                last_access_date: 0,
                cluster_high: ((cluster >> 16) & 0xFFFF) as u16,
                modification_time: 0,
                modification_date: 0,
                cluster_low: (cluster & 0xFFFF) as u16,
                file_size: size,
            };
            
            // Create internal entry from raw entry
            let mut internal_entry = Fat32DirectoryEntryInternal::from_raw(raw_entry);
            
            // Assemble complete LFN if available
            if !lfn_parts.is_empty() {
                // Reverse the parts since LFN entries are stored in reverse order
                lfn_parts.reverse();
                let long_filename = lfn_parts.join("");
                internal_entry.set_long_filename(long_filename);
            }
            
            entries.push(internal_entry);
            
            // Clear LFN for next entry
            lfn_parts.clear();
        }
        
        Ok(())
    }
    
    /// Write a new directory entry with LFN support to the specified directory cluster
    fn write_directory_entry_with_name(&self, dir_cluster: u32, filename: &str, cluster: u32, size: u32, is_directory: bool) -> Result<(), FileSystemError> {
        #[cfg(test)]
        {
            use crate::early_println;
            early_println!("[FAT32] write_directory_entry_with_name: dir_cluster={}, filename='{}'", 
                      dir_cluster, filename);
        }
        
        // Generate a unique SFN for this filename
        let unique_sfn = self.generate_unique_sfn(dir_cluster, filename)?;
        
        #[cfg(test)]
        {
            use crate::early_println;
            let sfn_str = core::str::from_utf8(&unique_sfn).unwrap_or("<invalid>");
            early_println!("[FAT32] generated unique SFN for '{}': '{}'", filename, sfn_str);
        }
        
        // Create the SFN entry with the generated SFN
        let entry = if is_directory {
            structures::Fat32DirectoryEntry {
                name: unique_sfn,
                attributes: 0x10, // Directory
                nt_reserved: 0,
                creation_time_tenths: 0,
                creation_time: 0,
                creation_date: 0,
                last_access_date: 0,
                cluster_high: (cluster >> 16) as u16,
                modification_time: 0,
                modification_date: 0,
                cluster_low: (cluster & 0xFFFF) as u16,
                file_size: 0,
            }
        } else {
            structures::Fat32DirectoryEntry {
                name: unique_sfn,
                attributes: 0x00, // Regular file
                nt_reserved: 0,
                creation_time_tenths: 0,
                creation_time: 0,
                creation_date: 0,
                last_access_date: 0,
                cluster_high: (cluster >> 16) as u16,
                modification_time: 0,
                modification_date: 0,
                cluster_low: (cluster & 0xFFFF) as u16,
                file_size: size,
            }
        };
        
        // Check if LFN is required
        let needs_lfn = Self::requires_lfn(filename);
        // #[cfg(test)]
        // {
        //     use crate::early_println;
        //     early_println!("[FAT32] needs_lfn: {}", needs_lfn);
        // }
        
        let mut entries_to_write = Vec::new();
        
        if needs_lfn {
            // Generate LFN entries
            let sfn_checksum = Self::calculate_sfn_checksum(&entry.name);
            let lfn_entries = Self::generate_lfn_entries(&filename, sfn_checksum);
            // #[cfg(test)]
            // {
            //     use crate::early_println;
            //     early_println!("[FAT32] generated {} LFN entries", lfn_entries.len());
            // }
            
            // Add LFN entries first (they come before the SFN entry)
            for lfn_entry in lfn_entries {
                entries_to_write.push(EntryToWrite::LFN(lfn_entry));
            }
        }
        
        // Add the SFN entry
        entries_to_write.push(EntryToWrite::SFN(entry));
        
        let total_entries_needed = entries_to_write.len();
        // #[cfg(test)]
        // {
        //     use crate::early_println;
        //     early_println!("[FAT32] total entries needed: {}", total_entries_needed);
        // }
        
        // Find space for all entries
        let mut current_cluster = dir_cluster;
        
        loop {
            // Read the current cluster
            let mut cluster_data = self.read_cluster_data(current_cluster)?;
            // #[cfg(test)]
            // {
            //     use crate::early_println;
            //     early_println!("[FAT32] read directory cluster {} data: {} bytes", current_cluster, cluster_data.len());
            // }
            
            // Look for consecutive empty slots
            let entries_per_cluster = (self.sectors_per_cluster * self.bytes_per_sector) / 32;
            // #[cfg(test)]
            // {
            //     use crate::early_println;
            //     early_println!("[FAT32] scanning {} directory entries for {} consecutive free slots", 
            //                   entries_per_cluster, total_entries_needed);
            // }
            
            for start_i in 0..entries_per_cluster {
                let start_offset = (start_i * 32) as usize;
                if start_offset + (total_entries_needed * 32) > cluster_data.len() {
                    break;
                }
                
                // Check if we have enough consecutive free slots
                let mut all_free = true;
                for j in 0..total_entries_needed {
                    let offset = start_offset + (j * 32);
                    if cluster_data[offset] != 0x00 && cluster_data[offset] != 0xE5 {
                        all_free = false;
                        break;
                    }
                }
                
                if all_free {
                    // #[cfg(test)]
                    // {
                    //     use crate::early_println;
                    //     early_println!("[FAT32] found {} consecutive free slots starting at entry {}, offset {}", 
                    //                   total_entries_needed, start_i, start_offset);
                    // }
                    
                    // Write all entries
                    for (j, entry_to_write) in entries_to_write.iter().enumerate() {
                        let offset = start_offset + (j * 32);
                        
                        match entry_to_write {
                            EntryToWrite::LFN(lfn_entry) => {
                                let entry_bytes = unsafe {
                                    core::slice::from_raw_parts(
                                        lfn_entry as *const _ as *const u8,
                                        32
                                    )
                                };
                                cluster_data[offset..offset + 32].copy_from_slice(entry_bytes);

                                // #[cfg(test)]
                                // {
                                //     use crate::early_println;
                                //     early_println!("[FAT32] wrote LFN entry at offset {} in cluster {}", offset, current_cluster);
                                // }
                            }
                            EntryToWrite::SFN(sfn_entry) => {
                                let entry_bytes = unsafe {
                                    core::slice::from_raw_parts(
                                        sfn_entry as *const _ as *const u8,
                                        32
                                    )
                                };
                                cluster_data[offset..offset + 32].copy_from_slice(entry_bytes);

                                // #[cfg(test)]
                                // {
                                //     use crate::early_println;
                                //     early_println!("[FAT32] wrote SFN entry at offset {}, first 8 bytes: {:02x?}", 
                                //               offset, &entry_bytes[0..8]);
                                // }
                            }
                        }
                    }
                    
                    // Write the modified cluster back to disk

                    // #[cfg(test)]
                    // {
                    //     use crate::early_println;
                    //     early_println!("[FAT32] writing modified directory cluster back to disk");
                    // }
                    
                    self.write_cluster_data(current_cluster, &cluster_data)?;
                    
                    // #[cfg(test)]
                    // {
                    //     use crate::early_println;
                    //     early_println!("[FAT32] write_directory_entry completed successfully");
                    // }

                    return Ok(());
                }
            }
            
            // No space found in this cluster, check next cluster in chain
            let next_cluster = self.read_fat_entry(current_cluster)?;
            if next_cluster >= 0x0FFFFFF8 {
                // End of cluster chain, need to allocate new cluster
                // #[cfg(test)]
                // {
                //     early_println!("[FAT32] extending directory: allocating new cluster");
                // }
                
                // Find a free cluster
                let new_cluster = self.allocate_cluster()?;
                // #[cfg(test)]
                // {
                //     early_println!("[FAT32] allocated new cluster {} for directory extension", new_cluster);
                // }
                // Link the new cluster to the directory chain
                self.write_fat_entry(current_cluster, new_cluster)?;
                // #[cfg(test)]
                // {
                //     early_println!("[FAT32] linked cluster {} -> {}", current_cluster, new_cluster);
                // }

                // Mark the new cluster as end of chain
                self.write_fat_entry(new_cluster, 0x0FFFFFF8)?;
                // #[cfg(test)]
                // {
                //     early_println!("[FAT32] marked cluster {} as end of chain", new_cluster);
                // }
                
                // Clear the new cluster (fill with zeros)
                let cluster_size = (self.sectors_per_cluster * self.bytes_per_sector) as usize;
                let mut empty_cluster = vec![0u8; cluster_size];
                
                // Write all entries to the new cluster
                for (j, entry_to_write) in entries_to_write.iter().enumerate() {
                    let offset = j * 32;
                    
                    match entry_to_write {
                        EntryToWrite::LFN(lfn_entry) => {
                            let entry_bytes = unsafe {
                                core::slice::from_raw_parts(
                                    lfn_entry as *const _ as *const u8,
                                    32
                                )
                            };
                            empty_cluster[offset..offset + 32].copy_from_slice(entry_bytes);

                            // #[cfg(test)]
                            // {
                            //     use crate::early_println;
                            //     early_println!("[FAT32] wrote LFN entry at offset {} in new cluster", offset);
                            // }
                        }
                        EntryToWrite::SFN(sfn_entry) => {
                            let entry_bytes = unsafe {
                                core::slice::from_raw_parts(
                                    sfn_entry as *const _ as *const u8,
                                    32
                                )
                            };
                            empty_cluster[offset..offset + 32].copy_from_slice(entry_bytes);

                            // #[cfg(test)]
                            // {
                            //     use crate::early_println;
                            //     early_println!("[FAT32] wrote SFN entry at offset {} in new cluster, first 8 bytes: {:02x?}", 
                            //               offset, &entry_bytes[0..8]);
                            // }
                        }
                    }
                }
                
                // Write the entries to the new cluster
                self.write_cluster_data(new_cluster, &empty_cluster)?;
                // early_println!("[FAT32] wrote directory entries to new cluster {}", new_cluster);
                
                return Ok(());
            }
            current_cluster = next_cluster;
        }
    }

    /// Update an existing directory entry in the specified directory cluster
    /// Supports both LFN and SFN matching
    pub fn update_directory_entry(&self, dir_cluster: u32, filename: &str, entry: &structures::Fat32DirectoryEntry) -> Result<(), FileSystemError> {
        // early_println!("[FAT32] update_directory_entry: searching for '{}' in cluster {}", filename, dir_cluster);
        
        let mut current_cluster = dir_cluster;
        
        loop {
            // Read the current cluster
            let mut cluster_data = self.read_cluster_data(current_cluster)?;
            
            // Parse directory entries in this cluster
            let entries_per_cluster = (self.sectors_per_cluster * self.bytes_per_sector) / 32;
            let mut lfn_parts: Vec<String> = Vec::new();
            let mut found_entry_offset: Option<usize> = None;
            let mut lfn_start_offset: Option<usize> = None;
            
            for i in 0..entries_per_cluster {
                let offset = (i * 32) as usize;
                if offset + 32 > cluster_data.len() {
                    break;
                }
                
                let entry_bytes = &cluster_data[offset..offset + 32];
                
                // Safety: We know the slice is exactly 32 bytes (size of Fat32DirectoryEntry)
                let existing_entry = unsafe {
                    core::ptr::read(entry_bytes.as_ptr() as *const structures::Fat32DirectoryEntry)
                };
                
                // Skip free entries
                if existing_entry.is_free() {
                    lfn_parts.clear();
                    lfn_start_offset = None;
                    continue;
                }
                
                // Handle LFN entries
                if existing_entry.is_long_filename() {
                    let lfn_entry = unsafe { &*(entry_bytes.as_ptr() as *const structures::Fat32LFNEntry) };
                    
                    // Extract characters from this LFN entry
                    let chars = lfn_entry.extract_chars();
                    
                    // Convert UTF-16 to UTF-8
                    let mut part = String::new();
                    for &ch in &chars {
                        if ch == 0 || ch == 0xFFFF {
                            break;
                        }
                        if let Some(c) = char::from_u32(ch as u32) {
                            part.push(c);
                        }
                    }
                    
                    // LFN entries are stored with highest sequence number first
                    if lfn_entry.is_last_lfn() {
                        // This is the first LFN entry we encounter (which contains the last part)
                        lfn_parts.clear();
                        lfn_parts.push(part);
                        lfn_start_offset = Some(offset);
                    } else {
                        // This is a subsequent LFN entry (which contains earlier parts)
                        lfn_parts.push(part);
                        if lfn_start_offset.is_none() {
                            lfn_start_offset = Some(offset);
                        }
                    }
                    continue;
                }
                
                // Skip dot entries
                if existing_entry.name[0] == b'.' {
                    lfn_parts.clear();
                    lfn_start_offset = None;
                    continue;
                }
                
                // This is a regular directory entry (SFN)
                let sfn_filename = existing_entry.filename();
                let full_filename = if !lfn_parts.is_empty() {
                    // Reverse the parts since LFN entries are stored in reverse order
                    lfn_parts.reverse();
                    lfn_parts.join("")
                } else {
                    sfn_filename.clone()
                };
                
                // #[cfg(test)]
                // {
                //     use crate::early_println;
                //     early_println!("[FAT32] checking entry: sfn='{}', lfn='{}', looking_for='{}'", 
                //                   sfn_filename, full_filename, filename);
                // }
                
                // Check if this matches the filename we're looking for
                let matches = full_filename == filename || 
                              sfn_filename == filename ||
                              full_filename.to_lowercase() == filename.to_lowercase() ||
                              sfn_filename.to_lowercase() == filename.to_lowercase();
                              
                if matches {
                    // early_println!("[FAT32] found matching entry at offset {}, updating cluster and size", offset);
                    
                    // Parse the existing entry to preserve its SFN and other metadata
                    let mut existing_entry = existing_entry; // Use the already parsed entry
                    
                    // Update only the cluster and file size fields
                    existing_entry.update_cluster_and_size(entry.cluster(), entry.file_size);
                    
                    // Write the updated entry back
                    let updated_entry_bytes = unsafe {
                        core::slice::from_raw_parts(&existing_entry as *const _ as *const u8, 32)
                    };
                    cluster_data[offset..offset + 32].copy_from_slice(updated_entry_bytes);
                    found_entry_offset = Some(offset);
                    break;
                }
                
                // Clear LFN accumulation for next entry
                lfn_parts.clear();
                lfn_start_offset = None;
            }
            
            if found_entry_offset.is_some() {
                // Write the modified cluster back to disk
                self.write_cluster_data(current_cluster, &cluster_data)?;
                // early_println!("[FAT32] directory entry updated successfully");
                return Ok(());
            }
            
            // Get next cluster in the chain
            let next_cluster = self.read_fat_entry(current_cluster)?;
            if next_cluster >= 0x0FFFFFF8 {
                // End of cluster chain
                break;
            }
            current_cluster = next_cluster;
        }
        
        // early_println!("[FAT32] directory entry for '{}' not found for update", filename);
        Err(FileSystemError::new(
            FileSystemErrorKind::NotFound,
            &format!("Directory entry for '{}' not found for update", filename),
        ))
    }
    
    /// Check if a filename requires LFN (Long File Name) entries
    fn requires_lfn(filename: &str) -> bool {
        // LFN is required if:
        // 1. Filename has lowercase letters
        // 2. Filename is longer than 8.3 format
        // 3. Filename contains invalid SFN characters
        // 4. Extension is longer than 3 characters
        // 5. Filename contains spaces or other special characters
        
        if filename.len() > 12 || filename.contains(' ') {
            return true;
        }
        
        if let Some(dot_pos) = filename.rfind('.') {
            let name_part = &filename[..dot_pos];
            let ext_part = &filename[dot_pos + 1..];
            
            // Check name part
            if name_part.len() > 8 || ext_part.len() > 3 {
                return true;
            }
            
            // Check for lowercase or invalid characters
            if name_part.chars().any(|c| c.is_lowercase() || !c.is_ascii_alphanumeric() && c != '_') ||
               ext_part.chars().any(|c| c.is_lowercase() || !c.is_ascii_alphanumeric()) {
                return true;
            }
        } else {
            // No extension
            if filename.len() > 8 {
                return true;
            }
            
            // Check for lowercase or invalid characters
            if filename.chars().any(|c| c.is_lowercase() || !c.is_ascii_alphanumeric() && c != '_') {
                return true;
            }
        }
        
        false
    }
    
    /// Generate LFN entries for a given filename
    fn generate_lfn_entries(filename: &str, sfn_checksum: u8) -> Vec<structures::Fat32LFNEntry> {
        let mut entries = Vec::new();
        let chars: Vec<u16> = filename.encode_utf16().collect();
        
        // Each LFN entry can hold 13 characters
        let entries_needed = (chars.len() + 12) / 13; // Round up division
        
        for entry_num in 0..entries_needed {
            let start_idx = entry_num * 13;
            
            let mut lfn_entry = structures::Fat32LFNEntry {
                sequence: (entry_num + 1) as u8,
                name1: [0xFFFF; 5],
                attributes: 0x0F, // LFN attribute
                entry_type: 0,
                checksum: sfn_checksum,
                name2: [0xFFFF; 6],
                cluster: 0,
                name3: [0xFFFF; 2],
            };
            
            // Mark last entry
            if entry_num == entries_needed - 1 {
                lfn_entry.sequence |= 0x40; // Last LFN entry flag
            }
            
            // Fill in characters
            let mut char_idx = start_idx;
            
            // Fill name1 (5 characters)
            for i in 0..5 {
                if char_idx < chars.len() {
                    lfn_entry.name1[i] = chars[char_idx];
                    char_idx += 1;
                } else if char_idx == chars.len() {
                    lfn_entry.name1[i] = 0x0000; // Null terminator
                    char_idx += 1;
                } else {
                    lfn_entry.name1[i] = 0xFFFF; // Padding
                }
            }
            
            // Fill name2 (6 characters)
            for i in 0..6 {
                if char_idx < chars.len() {
                    lfn_entry.name2[i] = chars[char_idx];
                    char_idx += 1;
                } else if char_idx == chars.len() {
                    lfn_entry.name2[i] = 0x0000; // Null terminator
                    char_idx += 1;
                } else {
                    lfn_entry.name2[i] = 0xFFFF; // Padding
                }
            }
            
            // Fill name3 (2 characters)
            for i in 0..2 {
                if char_idx < chars.len() {
                    lfn_entry.name3[i] = chars[char_idx];
                    char_idx += 1;
                } else if char_idx == chars.len() {
                    lfn_entry.name3[i] = 0x0000; // Null terminator
                    char_idx += 1;
                } else {
                    lfn_entry.name3[i] = 0xFFFF; // Padding
                }
            }
            
            entries.push(lfn_entry);
        }
        
        // LFN entries need to be in reverse order (last entry first)
        entries.reverse();
        entries
    }
    
    /// Calculate checksum for SFN (used in LFN entries)
    fn calculate_sfn_checksum(sfn: &[u8; 11]) -> u8 {
        let mut checksum = 0u8;
        for &byte in sfn {
            checksum = ((checksum & 1) << 7).wrapping_add(checksum >> 1).wrapping_add(byte);
        }
        checksum
    }

    /// Remove a directory entry from the specified directory cluster
    fn remove_directory_entry(&self, dir_cluster: u32, filename: &str) -> Result<(), FileSystemError> {
        let entries_per_cluster = (self.sectors_per_cluster * self.bytes_per_sector) / 32;
        let mut current_cluster = dir_cluster;

        // #[cfg(test)]
        // {
        //     use crate::early_println;
        //     early_println!("[FAT32] remove_directory_entry: searching for '{}' in cluster {}", filename, dir_cluster);
        // }

        loop {
            let mut cluster_data = self.read_cluster_data(current_cluster)?;
            let mut entries_to_remove = Vec::new();
            let mut i = 0;
            
            while i < entries_per_cluster {
                let entry_offset = (i * 32) as usize;
                if entry_offset + 32 > cluster_data.len() {
                    break;
                }

                let entry_bytes = &cluster_data[entry_offset..entry_offset + 32];
                
                // Check if this is the end of directory
                if entry_bytes[0] == 0x00 {
                    break;
                }

                // Skip free entries
                if entry_bytes[0] == 0xE5 {
                    i += 1;
                    continue;
                }

                // Check if this is an LFN entry
                if entry_bytes[11] == 0x0F {
                    // This is an LFN entry - we'll need to handle LFN chains later
                    i += 1;
                    continue;
                }

                // This is a regular SFN entry
                let dir_entry = unsafe {
                    core::ptr::read_unaligned(entry_bytes.as_ptr() as *const structures::Fat32DirectoryEntry)
                };

                let entry_filename = dir_entry.filename().to_lowercase();
                
                // Also check if this entry has LFN entries and read the long filename
                let mut full_filename = entry_filename.clone();
                
                // Look backwards for LFN entries that belong to this SFN
                let mut lfn_start = i;
                let mut lfn_name = String::new();
                
                // Find the start of LFN entries
                for j in (0..i).rev() {
                    let lfn_offset = (j * 32) as usize;
                    let lfn_bytes = &cluster_data[lfn_offset..lfn_offset + 32];
                    
                    // Check if this is an LFN entry
                    if lfn_bytes[11] == 0x0F {
                        lfn_start = j;
                    } else {
                        break;
                    }
                }
                
                // If we found LFN entries, reconstruct the full filename
                if lfn_start < i {
                    let mut lfn_entries = Vec::new();
                    
                    // Collect all LFN entries
                    for j in lfn_start..i {
                        let lfn_offset = (j * 32) as usize;
                        let lfn_bytes = &cluster_data[lfn_offset..lfn_offset + 32];
                        
                        if lfn_bytes[11] == 0x0F {
                            let lfn_entry = unsafe {
                                core::ptr::read_unaligned(lfn_bytes.as_ptr() as *const structures::Fat32LFNEntry)
                            };
                            lfn_entries.push(lfn_entry);
                        }
                    }
                    
                    // Sort LFN entries by their sequence number (ascending order)
                    lfn_entries.sort_by(|a, b| {
                        let seq_a = a.sequence & 0x1F; // Remove last entry flag
                        let seq_b = b.sequence & 0x1F;
                        seq_a.cmp(&seq_b) // Ascending order (lowest sequence first)
                    });
                    
                    // Reconstruct the filename from sorted LFN entries
                    for lfn_entry in lfn_entries {
                        // Extract characters from name1, name2, name3
                        for k in 0..5 {
                            let ch = lfn_entry.name1[k];
                            if ch != 0x0000 && ch != 0xFFFF {
                                if let Some(c) = char::from_u32(ch as u32) {
                                    lfn_name.push(c);
                                }
                            } else {
                                break;
                            }
                        }
                        for k in 0..6 {
                            let ch = lfn_entry.name2[k];
                            if ch != 0x0000 && ch != 0xFFFF {
                                if let Some(c) = char::from_u32(ch as u32) {
                                    lfn_name.push(c);
                                }
                            } else {
                                break;
                            }
                        }
                        for k in 0..2 {
                            let ch = lfn_entry.name3[k];
                            if ch != 0x0000 && ch != 0xFFFF {
                                if let Some(c) = char::from_u32(ch as u32) {
                                    lfn_name.push(c);
                                }
                            } else {
                                break;
                            }
                        }
                    }
                    
                    if !lfn_name.is_empty() {
                        full_filename = lfn_name;
                    }
                }
                
                #[cfg(test)]
                {
                    use crate::early_println;
                    early_println!("[FAT32] checking entry: sfn='{}', lfn='{}', looking_for='{}'", entry_filename, full_filename, filename);
                }

                // Check both SFN and LFN
                if entry_filename == filename.to_lowercase() || full_filename == filename {
                    #[cfg(test)]
                    {
                        use crate::early_println;
                        early_println!("[FAT32] found matching entry at offset {}", entry_offset);
                    }

                    // Mark all entries (LFN + SFN) for removal
                    for remove_i in lfn_start..=i {
                        entries_to_remove.push(remove_i);
                    }

                    break;
                }

                i += 1;
            }

            // Remove the entries by marking them as deleted (0xE5)
            if !entries_to_remove.is_empty() {
                for &remove_i in &entries_to_remove {
                    let entry_offset = (remove_i * 32) as usize;
                    cluster_data[entry_offset] = 0xE5; // Mark as deleted
                    
                    // #[cfg(test)]
                    // {
                    //     use crate::early_println;
                    //     early_println!("[FAT32] marked entry {} as deleted at offset {}", remove_i, entry_offset);
                    // }
                }

                // Write the modified cluster back to disk
                self.write_cluster_data(current_cluster, &cluster_data)?;
                
                // #[cfg(test)]
                // {
                //     use crate::early_println;
                //     early_println!("[FAT32] remove_directory_entry completed successfully");
                // }
                
                return Ok(());
            }

            // Move to next cluster in chain
            let next_cluster = self.read_fat_entry(current_cluster)?;
            if next_cluster >= 0x0FFFFFF8 || next_cluster == 0 {
                break;
            }
            current_cluster = next_cluster;
        }

        // File not found
        Err(FileSystemError::new(
            FileSystemErrorKind::NotFound,
            format!("Directory entry '{}' not found", filename)
        ))
    }
    
    /// Generate a FAT32-compliant SFN (Short File Name) from a long filename
    /// This implementation follows the Microsoft FAT32 specification
    fn generate_sfn(filename: &str, numeric_tail: Option<u32>) -> [u8; 11] {
        let mut sfn = [b' '; 11];
        
        // Convert to uppercase and filter invalid characters
        let clean_filename = filename.to_uppercase();
        
        // Split into name and extension
        let (name_part, ext_part) = if let Some(dot_pos) = clean_filename.rfind('.') {
            (&clean_filename[..dot_pos], Some(&clean_filename[dot_pos + 1..]))
        } else {
            (clean_filename.as_str(), None)
        };
        
        // Generate the name part (first 8 characters)
        let mut name_chars = Vec::new();
        for ch in name_part.chars() {
            if Self::is_valid_sfn_char(ch) {
                name_chars.push(ch as u8);
            } else if ch == ' ' {
                // Skip spaces
                continue;
            } else {
                // Replace invalid characters with underscore
                name_chars.push(b'_');
            }
        }
        
        // Handle numeric tail for duplicate names (~1, ~2, etc.)
        let base_name_len = if let Some(tail) = numeric_tail {
            let tail_str = format!("~{}", tail);
            let max_base_len = 8 - tail_str.len();
            core::cmp::min(name_chars.len(), max_base_len)
        } else {
            core::cmp::min(name_chars.len(), 8)
        };
        
        // Copy base name
        for i in 0..base_name_len {
            sfn[i] = name_chars[i];
        }
        
        // Add numeric tail if present
        if let Some(tail) = numeric_tail {
            let tail_str = format!("~{}", tail);
            let tail_bytes = tail_str.as_bytes();
            for (i, &byte) in tail_bytes.iter().enumerate() {
                if base_name_len + i < 8 {
                    sfn[base_name_len + i] = byte;
                }
            }
        }
        
        // Handle extension part
        if let Some(ext) = ext_part {
            let mut ext_chars = Vec::new();
            for ch in ext.chars().take(3) { // Max 3 characters for extension
                if Self::is_valid_sfn_char(ch) {
                    ext_chars.push(ch as u8);
                } else if ch != ' ' {
                    ext_chars.push(b'_');
                }
            }
            
            // Copy extension
            for (i, &byte) in ext_chars.iter().enumerate() {
                if i < 3 {
                    sfn[8 + i] = byte;
                }
            }
        }
        
        sfn
    }
    
    /// Check if a character is valid for SFN according to FAT32 specification
    fn is_valid_sfn_char(ch: char) -> bool {
        match ch {
            'A'..='Z' | '0'..='9' => true,
            '!' | '#' | '$' | '%' | '&' | '\'' | '(' | ')' | '-' | '@' | '^' | '_' | '`' | '{' | '}' | '~' => true,
            _ => false,
        }
    }
    
    /// Check if filename needs numeric tail (Linux-style behavior)
    /// Returns true ONLY for filenames that cannot fit in 8.3 format or contain invalid characters
    fn filename_needs_numeric_tail(&self, filename: &str) -> bool {
        // Find the last dot to separate name and extension
        if let Some(dot_pos) = filename.rfind('.') {
            let main_name = &filename[..dot_pos];
            let extension = &filename[dot_pos + 1..];
            
            #[cfg(test)]
            {
                use crate::early_println;
                early_println!("[FAT32] analyzing filename: '{}' -> name='{}' ({} chars), ext='{}' ({} chars)", 
                    filename, main_name, main_name.len(), extension, extension.len());
            }
            
            // Check if main name is longer than 8 chars or extension longer than 3 chars
            if main_name.len() > 8 || extension.len() > 3 {
                #[cfg(test)]
                {
                    use crate::early_println;
                    early_println!("[FAT32] filename_needs_numeric_tail: '{}' -> true (length exceeded)", filename);
                }
                return true;
            }
            
            // Check main name for invalid characters
            for ch in main_name.chars() {
                if ch == ' ' || ch == '+' || ch == '=' || ch == '[' || ch == ']' || ch == ',' || ch == ';' {
                    #[cfg(test)]
                    {
                        use crate::early_println;
                        early_println!("[FAT32] filename_needs_numeric_tail: '{}' -> true (invalid char in name: '{}')", filename, ch);
                    }
                    return true;
                }
            }
            
            // Check extension for invalid characters  
            for ch in extension.chars() {
                if ch == ' ' || ch == '+' || ch == '=' || ch == '[' || ch == ']' || ch == ',' || ch == ';' {
                    #[cfg(test)]
                    {
                        use crate::early_println;
                        early_println!("[FAT32] filename_needs_numeric_tail: '{}' -> true (invalid char in ext: '{}')", filename, ch);
                    }
                    return true;
                }
            }
        } else {
            // No extension - check if name is longer than 8 chars
            if filename.len() > 8 {
                #[cfg(test)]
                {
                    use crate::early_println;
                    early_println!("[FAT32] filename_needs_numeric_tail: '{}' -> true (no ext, length={})", filename, filename.len());
                }
                return true;
            }
            
            // Check for invalid characters
            for ch in filename.chars() {
                if ch == ' ' || ch == '+' || ch == '=' || ch == '[' || ch == ']' || ch == ',' || ch == ';' {
                    #[cfg(test)]
                    {
                        use crate::early_println;
                        early_println!("[FAT32] filename_needs_numeric_tail: '{}' -> true (invalid char: '{}')", filename, ch);
                    }
                    return true;
                }
            }
        }
        
        #[cfg(test)]
        {
            use crate::early_println;
            early_println!("[FAT32] filename_needs_numeric_tail: '{}' -> false (fits in 8.3)", filename);
        }
        false
    }
    
    /// Generate a unique SFN by checking for duplicates in the directory
    fn generate_unique_sfn(&self, dir_cluster: u32, desired_filename: &str) -> Result<[u8; 11], FileSystemError> {
        // Check if the filename needs numeric tail (Linux-style: long names get ~1 from start)
        let needs_tail = self.filename_needs_numeric_tail(desired_filename);
        
        if !needs_tail {
            // Short filename that fits in 8.3 - try without tail first
            let base_sfn = Self::generate_sfn(desired_filename, None);
            if !self.sfn_exists_in_directory(dir_cluster, &base_sfn)? {
                return Ok(base_sfn);
            }
        }
        
        // Try with numeric tails ~1, ~2, ..., ~999999 (Linux-style)
        for tail in 1..=999999 {
            let sfn_with_tail = Self::generate_sfn(desired_filename, Some(tail));
            
            if !self.sfn_exists_in_directory(dir_cluster, &sfn_with_tail)? {
                return Ok(sfn_with_tail);
            }
        }
        
        Err(FileSystemError::new(
            FileSystemErrorKind::NoSpace,
            "Cannot generate unique SFN: too many similar filenames"
        ))
    }
    
    /// Check if an SFN already exists in the specified directory
    fn sfn_exists_in_directory(&self, dir_cluster: u32, sfn: &[u8; 11]) -> Result<bool, FileSystemError> {
        let mut current_cluster = dir_cluster;
        let entries_per_cluster = (self.sectors_per_cluster * self.bytes_per_sector / 32) as usize;
        
        loop {
            let cluster_data = self.read_cluster_data(current_cluster)?;
            
            for i in 0..entries_per_cluster {
                let entry_offset = i * 32;
                if entry_offset + 32 > cluster_data.len() {
                    break;
                }
                
                let entry_bytes = &cluster_data[entry_offset..entry_offset + 32];
                
                // End of directory
                if entry_bytes[0] == 0x00 {
                    return Ok(false);
                }
                
                // Skip free entries and LFN entries
                if entry_bytes[0] == 0xE5 || entry_bytes[11] == 0x0F {
                    continue;
                }
                
                // Compare SFN
                let existing_sfn = &entry_bytes[0..11];
                if existing_sfn == sfn {
                    return Ok(true);
                }
            }
            
            // Move to next cluster in chain
            let next_cluster = self.read_fat_entry(current_cluster)?;
            if next_cluster >= 0x0FFFFFF8 {
                break;
            }
            current_cluster = next_cluster;
        }
        
        Ok(false)
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
        
        // Get the starting cluster for the directory
        let parent_cluster = *fat32_parent.cluster.read();
        let starting_cluster = if parent_cluster == 0 {
            self.boot_sector.root_cluster
        } else {
            parent_cluster
        };
        
        // Search for the file in the directory
        let found_entry = self.lookup_file_in_directory(starting_cluster, name)?;
         // Create a new Fat32Node for the found entry
        let node = if found_entry.is_directory() {
            let dir_node = Fat32Node::new_directory(found_entry.name(), 0, found_entry.cluster());
            // Set filesystem reference from parent
            if let Some(fs_ref) = fat32_parent.filesystem() {
                dir_node.set_filesystem(fs_ref);
            }
            dir_node
        } else {
            let file_node = Fat32Node::new_file(found_entry.name(), 0, found_entry.cluster());
            // Update file size
            {
                let mut metadata = file_node.metadata.write();
                metadata.size = found_entry.size() as usize;
            }
            // Set filesystem reference from parent
            if let Some(fs_ref) = fat32_parent.filesystem() {
                file_node.set_filesystem(fs_ref);
            }
            file_node
        };

        Ok(Arc::new(node))
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
                // Get parent from parent node
                let parent_cluster = if let Some(parent_ref) = fat32_node.parent.read().as_ref() {
                    if let Some(parent_node) = parent_ref.upgrade() {
                        *parent_node.cluster.read()
                    } else {
                        0 // Default to 0 if parent is not available
                    }
                } else {
                    0 // Default to 0 if no parent reference
                };
                
                Ok(Arc::new(Fat32FileObject::new(Arc::new(fat32_node.clone()), parent_cluster)))
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
        
        // Check if file already exists (FAT32 is case-insensitive)
        let parent_cluster = *fat32_parent.cluster.read();
        let actual_parent_cluster = if parent_cluster == 0 {
            self.root_cluster  // Use root cluster for root directory
        } else {
            parent_cluster
        };
        
        // Check on disk for case-insensitive duplicates
        if let Ok(_existing_entry) = self.lookup_file_in_directory(actual_parent_cluster, name) {
            return Err(FileSystemError::new(
                FileSystemErrorKind::AlreadyExists,
                format!("File '{}' already exists (case-insensitive)", name)
            ));
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

        // Set parent reference
        {
            let parent_arc: Arc<Fat32Node> = Arc::new(fat32_parent.clone());
            let parent_weak = Arc::downgrade(&parent_arc);
            *new_node.parent.write() = Some(parent_weak);
        }

        // Write directory entry to the parent directory's cluster
        let parent_cluster = *fat32_parent.cluster.read();
        let actual_parent_cluster = if parent_cluster == 0 {
            self.root_cluster  // Use root cluster for root directory
        } else {
            parent_cluster
        };
        
        // #[cfg(test)]
        // {
        //     use crate::early_println;
        //     early_println!("[FAT32] Creating file '{}' in parent cluster {} (original: {})", 
        //                   name, actual_parent_cluster, parent_cluster);
        // }
        
        self.write_directory_entry_with_name(actual_parent_cluster, name, 0, 0, file_type == FileType::Directory)?;

        // Add to parent directory (in-memory)
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

        // Get the file node to retrieve cluster information
        let file_node = {
            let children = fat32_parent.children.read();
            match children.get(name) {
                Some(node) => {
                    // Downcast to Fat32Node to get cluster information
                    node.as_any()
                        .downcast_ref::<Fat32Node>()
                        .ok_or_else(|| FileSystemError::new(
                            FileSystemErrorKind::NotSupported,
                            "Invalid node type for FAT32"
                        ))?.clone()
                },
                None => return Err(FileSystemError::new(
                    FileSystemErrorKind::NotFound,
                    format!("File '{}' not found", name)
                )),
            }
        };

        // Get the starting cluster and deallocate the cluster chain
        let start_cluster = file_node.cluster();
        if start_cluster != 0 {
            self.free_cluster_chain(start_cluster)?;
        }

        // Remove the directory entry from disk
        let parent_cluster = *fat32_parent.cluster.read();
        let actual_parent_cluster = if parent_cluster == 0 {
            self.root_cluster // Use root cluster for root directory
        } else {
            parent_cluster
        };

        // #[cfg(test)]
        // {
        //     use crate::early_println;
        //     early_println!("[FAT32] Removing file '{}' from parent cluster {} (original: {})", 
        //                   name, actual_parent_cluster, parent_cluster);
        // }

        self.remove_directory_entry(actual_parent_cluster, name)?;

        // Remove from parent directory (in-memory)
        {
            let mut children = fat32_parent.children.write();
            children.remove(name);
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
        
        // Load directory entries from disk if not already loaded
        let mut fat32_entries = Vec::new();
        let cluster = *fat32_node.cluster.read();
        
        // #[cfg(test)]
        // {
        //     use crate::early_println;
        //     early_println!("[FAT32] Reading directory entries from cluster {}", cluster);
        // }
        
        if cluster == 0 {
            // This is likely the root directory - handle FAT32 root directory differently
            // #[cfg(test)]
            // {
            //     use crate::early_println;
            //     early_println!("[FAT32] Reading FAT32 root directory (cluster 0, using root_cluster {})", self.root_cluster);
            // }
            self.read_directory_entries(self.root_cluster, &mut fat32_entries)?;
        } else {
            self.read_directory_entries(cluster, &mut fat32_entries)?;
        }
        
        // Convert Fat32DirectoryEntryInternal to DirectoryEntryInternal
        let mut entries = Vec::new();
        for fat32_entry in fat32_entries {
            let file_type = if fat32_entry.is_directory() {
                FileType::Directory
            } else {
                FileType::RegularFile
            };
            
            let entry = DirectoryEntryInternal {
                name: fat32_entry.name(),
                file_type,
                file_id: fat32_entry.cluster() as u64, // Use cluster as file_id
            };
            entries.push(entry);
        }
        
        // #[cfg(test)]
        // {
        //     use crate::early_println;
        //     early_println!("[FAT32] Found {} directory entries", entries.len());
        // }
        
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

/// Helper enum for writing directory entries with LFN support
#[derive(Debug, Clone)]
enum EntryToWrite {
    LFN(structures::Fat32LFNEntry),
    SFN(structures::Fat32DirectoryEntry),
}

