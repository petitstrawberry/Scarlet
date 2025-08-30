//! FAT32 data structures
//!
//! This module defines the on-disk data structures used by the FAT32 filesystem.
//! All structures are packed and follow the Microsoft FAT32 specification.

use core::mem;

/// FAT32 Boot Sector structure
/// 
/// This structure represents the boot sector (first sector) of a FAT32 filesystem.
/// It contains essential information about the filesystem layout and parameters.
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct Fat32BootSector {
    /// Jump instruction (3 bytes)
    pub jump_instruction: [u8; 3],
    /// OEM name (8 bytes)
    pub oem_name: [u8; 8],
    /// Bytes per sector (typically 512)
    pub bytes_per_sector: u16,
    /// Sectors per cluster (must be power of 2)
    pub sectors_per_cluster: u8,
    /// Number of reserved sectors (including boot sector)
    pub reserved_sectors: u16,
    /// Number of FAT copies (typically 2)
    pub fat_count: u8,
    /// Maximum number of root directory entries (0 for FAT32)
    pub max_root_entries: u16,
    /// Total sectors (16-bit, 0 for FAT32)
    pub total_sectors_16: u16,
    /// Media descriptor
    pub media_descriptor: u8,
    /// Sectors per FAT (16-bit, 0 for FAT32)
    pub sectors_per_fat_16: u16,
    /// Sectors per track
    pub sectors_per_track: u16,
    /// Number of heads
    pub heads: u16,
    /// Hidden sectors
    pub hidden_sectors: u32,
    /// Total sectors (32-bit)
    pub total_sectors_32: u32,
    /// Sectors per FAT (32-bit)
    pub sectors_per_fat: u32,
    /// Extended flags
    pub extended_flags: u16,
    /// Filesystem version
    pub fs_version: u16,
    /// Root directory cluster number
    pub root_cluster: u32,
    /// Filesystem info sector number
    pub fs_info_sector: u16,
    /// Backup boot sector location
    pub backup_boot_sector: u16,
    /// Reserved bytes
    pub reserved: [u8; 12],
    /// Drive number
    pub drive_number: u8,
    /// Reserved
    pub reserved1: u8,
    /// Boot signature
    pub boot_signature: u8,
    /// Volume serial number
    pub volume_serial: u32,
    /// Volume label
    pub volume_label: [u8; 11],
    /// Filesystem type string
    pub fs_type: [u8; 8],
    /// Boot code
    pub boot_code: [u8; 420],
    /// Boot sector signature (0xAA55)
    pub signature: u16,
}

impl Fat32BootSector {
    /// Check if this is a valid FAT32 boot sector
    pub fn is_valid(&self) -> bool {
        // Check signature
        if self.signature != 0xAA55 {
            return false;
        }
        
        // Check bytes per sector
        match self.bytes_per_sector {
            512 | 1024 | 2048 | 4096 => {},
            _ => return false,
        }
        
        // Check sectors per cluster (must be power of 2)
        if self.sectors_per_cluster == 0 || 
           (self.sectors_per_cluster & (self.sectors_per_cluster - 1)) != 0 {
            return false;
        }
        
        // For FAT32, max_root_entries should be 0
        if self.max_root_entries != 0 {
            return false;
        }
        
        // For FAT32, sectors_per_fat_16 should be 0
        if self.sectors_per_fat_16 != 0 {
            return false;
        }
        
        true
    }
    
    /// Get the total number of sectors
    pub fn total_sectors(&self) -> u32 {
        if self.total_sectors_16 != 0 {
            self.total_sectors_16 as u32
        } else {
            self.total_sectors_32
        }
    }
    
    /// Calculate the first data sector
    pub fn first_data_sector(&self) -> u32 {
        self.reserved_sectors as u32 + (self.fat_count as u32 * self.sectors_per_fat)
    }
    
    /// Calculate the number of data sectors
    pub fn data_sectors(&self) -> u32 {
        let total_sectors = self.total_sectors();
        let first_data_sector = self.first_data_sector();
        total_sectors - first_data_sector
    }
    
    /// Calculate the number of clusters
    pub fn cluster_count(&self) -> u32 {
        self.data_sectors() / self.sectors_per_cluster as u32
    }
}

/// FAT32 Directory Entry structure
/// 
/// This structure represents a single directory entry in a FAT32 directory.
/// Each entry is exactly 32 bytes.
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct Fat32DirectoryEntry {
    /// Filename (8.3 format, padded with spaces)
    pub name: [u8; 11],
    /// File attributes
    pub attributes: u8,
    /// Reserved for Windows NT
    pub nt_reserved: u8,
    /// Creation time (tenths of a second)
    pub creation_time_tenths: u8,
    /// Creation time
    pub creation_time: u16,
    /// Creation date
    pub creation_date: u16,
    /// Last access date
    pub last_access_date: u16,
    /// High 16 bits of cluster number
    pub cluster_high: u16,
    /// Last modification time
    pub modification_time: u16,
    /// Last modification date
    pub modification_date: u16,
    /// Low 16 bits of cluster number
    pub cluster_low: u16,
    /// File size in bytes
    pub file_size: u32,
}

impl Fat32DirectoryEntry {
    /// Check if this entry is free (available for use)
    pub fn is_free(&self) -> bool {
        self.name[0] == 0x00 || self.name[0] == 0xE5
    }
    
    /// Check if this is the last entry in the directory
    pub fn is_last(&self) -> bool {
        self.name[0] == 0x00
    }
    
    /// Check if this is a long filename entry
    pub fn is_long_filename(&self) -> bool {
        self.attributes == 0x0F
    }
    
    /// Get the starting cluster number
    pub fn cluster(&self) -> u32 {
        (self.cluster_high as u32) << 16 | (self.cluster_low as u32)
    }
    
    /// Set the starting cluster number
    pub fn set_cluster(&mut self, cluster: u32) {
        self.cluster_high = (cluster >> 16) as u16;
        self.cluster_low = (cluster & 0xFFFF) as u16;
    }
    
    /// Check if this is a directory
    pub fn is_directory(&self) -> bool {
        self.attributes & 0x10 != 0
    }
    
    /// Check if this is a regular file
    pub fn is_file(&self) -> bool {
        !self.is_directory() && !self.is_volume_label() && !self.is_long_filename()
    }
    
    /// Check if this is a volume label
    pub fn is_volume_label(&self) -> bool {
        self.attributes & 0x08 != 0
    }
    
    /// Get the filename as a string (8.3 format)
    pub fn filename(&self) -> alloc::string::String {
        use alloc::string::String;
        
        // Handle special cases
        if self.name[0] == 0x05 {
            // This represents a filename starting with 0xE5
            let mut name = self.name;
            name[0] = 0xE5;
            return Self::parse_filename(&name);
        }
        
        Self::parse_filename(&self.name)
    }
    
    /// Parse 8.3 filename format
    fn parse_filename(name: &[u8; 11]) -> alloc::string::String {
        use alloc::string::String;
        
        let mut result = String::new();
        
        // Extract the main filename (first 8 characters)
        let mut main_name_len = 8;
        for i in (0..8).rev() {
            if name[i] != b' ' {
                main_name_len = i + 1;
                break;
            }
        }
        
        if main_name_len == 0 {
            return result;
        }
        
        for i in 0..main_name_len {
            result.push(name[i] as char);
        }
        
        // Check for extension (last 3 characters)
        let mut ext_len = 3;
        for i in (8..11).rev() {
            if name[i] != b' ' {
                ext_len = i - 8 + 1;
                break;
            }
        }
        
        if ext_len > 0 && name[8] != b' ' {
            result.push('.');
            for i in 8..(8 + ext_len) {
                result.push(name[i] as char);
            }
        }
        
        result
    }
    
    /// Create a new directory entry
    pub fn new_file(name: &str, cluster: u32, size: u32) -> Self {
        let mut entry = Self {
            name: [b' '; 11],
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
        };
        
        entry.set_name(name);
        entry
    }
    
    /// Create a new directory entry for a directory
    pub fn new_directory(name: &str, cluster: u32) -> Self {
        let mut entry = Self {
            name: [b' '; 11],
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
            file_size: 0, // Directories have size 0
        };
        
        entry.set_name(name);
        entry
    }
    
    /// Set the filename (8.3 format)
    fn set_name(&mut self, name: &str) {
        // Initialize with spaces
        self.name = [b' '; 11];
        
        // Split into name and extension
        if let Some(dot_pos) = name.rfind('.') {
            let main_name = &name[..dot_pos];
            let extension = &name[dot_pos + 1..];
            
            // Copy main name (up to 8 characters)
            let main_len = core::cmp::min(main_name.len(), 8);
            for (i, byte) in main_name.bytes().take(main_len).enumerate() {
                self.name[i] = byte.to_ascii_uppercase();
            }
            
            // Copy extension (up to 3 characters)
            let ext_len = core::cmp::min(extension.len(), 3);
            for (i, byte) in extension.bytes().take(ext_len).enumerate() {
                self.name[8 + i] = byte.to_ascii_uppercase();
            }
        } else {
            // No extension, just copy the name
            let name_len = core::cmp::min(name.len(), 8);
            for (i, byte) in name.bytes().take(name_len).enumerate() {
                self.name[i] = byte.to_ascii_uppercase();
            }
        }
    }
}

/// FAT32 Filesystem Information Sector
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct Fat32FsInfo {
    /// Lead signature (0x41615252)
    pub lead_signature: u32,
    /// Reserved bytes
    pub reserved1: [u8; 480],
    /// Structure signature (0x61417272)
    pub structure_signature: u32,
    /// Free cluster count (0xFFFFFFFF if unknown)
    pub free_cluster_count: u32,
    /// Next free cluster hint
    pub next_free_cluster: u32,
    /// Reserved bytes
    pub reserved2: [u8; 12],
    /// Trail signature (0xAA550000)
    pub trail_signature: u32,
}

impl Fat32FsInfo {
    /// Check if this is a valid FSInfo sector
    pub fn is_valid(&self) -> bool {
        self.lead_signature == 0x41615252 &&
        self.structure_signature == 0x61417272 &&
        self.trail_signature == 0xAA550000
    }
}

/// FAT entry constants
pub const FAT32_EOC: u32 = 0x0FFFFFF8; // End of chain marker
pub const FAT32_BAD: u32 = 0x0FFFFFF7; // Bad cluster marker
pub const FAT32_FREE: u32 = 0x00000000; // Free cluster marker

/// Directory entry attribute constants
pub const ATTR_READ_ONLY: u8 = 0x01;
pub const ATTR_HIDDEN: u8 = 0x02;
pub const ATTR_SYSTEM: u8 = 0x04;
pub const ATTR_VOLUME_ID: u8 = 0x08;
pub const ATTR_DIRECTORY: u8 = 0x10;
pub const ATTR_ARCHIVE: u8 = 0x20;
pub const ATTR_LONG_NAME: u8 = 0x0F;

/// Directory entry size in bytes
pub const DIR_ENTRY_SIZE: usize = mem::size_of::<Fat32DirectoryEntry>();

// Ensure structures have correct sizes
const _: () = assert!(mem::size_of::<Fat32BootSector>() == 512);
const _: () = assert!(mem::size_of::<Fat32DirectoryEntry>() == 32);
const _: () = assert!(mem::size_of::<Fat32FsInfo>() == 512);