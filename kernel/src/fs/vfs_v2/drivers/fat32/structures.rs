//! FAT32 data structures
//!
//! This module defines the on-disk data structures used by the FAT32 filesystem.
//! All structures are packed and follow the Microsoft FAT32 specification.

use core::mem;
use alloc::{vec::Vec, string::String, format, string::ToString};

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
    
    /// Update only the cluster and file size fields (preserves SFN and other metadata)
    pub fn update_cluster_and_size(&mut self, cluster: u32, size: u32) {
        self.set_cluster(cluster);
        self.file_size = size;
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
            // Convert to lowercase for case-insensitive comparison
            result.push((name[i] as char).to_ascii_lowercase());
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
                // Convert to lowercase for case-insensitive comparison
                result.push((name[i] as char).to_ascii_lowercase());
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
        
        let sfn = Self::generate_sfn(name, 1);
        entry.set_name(sfn);
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
        
        let sfn = Self::generate_sfn(name, 1);
        entry.set_name(sfn);
        entry
    }
    
    /// Set the filename (8.3 format) - expects properly formatted SFN bytes
    fn set_name(&mut self, name: [u8; 11]) {
        self.name = name;
    }
    
    /// Generate proper 8.3 Short File Name (SFN) from a long filename
    pub fn generate_sfn(name: &str, numeric_tail: u32) -> [u8; 11] {
        let mut sfn = [b' '; 11];
        
        // Split into name and extension
        if let Some(dot_pos) = name.rfind('.') {
            let main_name = &name[..dot_pos];
            let extension = &name[dot_pos + 1..];
            
            // Process main name
            let main_name_upper = main_name.to_ascii_uppercase();
            let main_bytes: Vec<u8> = main_name_upper.bytes()
                .filter(|&b| Self::is_valid_sfn_char(b))
                .collect();
            
            // Process extension
            let extension_upper = extension.to_ascii_uppercase();
            let ext_bytes: Vec<u8> = extension_upper.bytes()
                .filter(|&b| Self::is_valid_sfn_char(b))
                .collect();
            
            // Check if we need numeric tail (long filename or case conversion)
            let needs_numeric_tail = main_bytes.len() > 8 || ext_bytes.len() > 3 || 
                                     main_name != main_name_upper || extension != extension_upper;
            
            if needs_numeric_tail {
                // Generate numeric tail format: BASENAME~N.EXT
                let tail_str = format!("~{}", numeric_tail);
                let available_chars = 8 - tail_str.len();
                
                // Copy base name (truncated to fit tail)
                let copy_len = core::cmp::min(main_bytes.len(), available_chars);
                for i in 0..copy_len {
                    sfn[i] = main_bytes[i];
                }
                
                // Add numeric tail
                for (i, byte) in tail_str.bytes().enumerate() {
                    if copy_len + i < 8 {
                        sfn[copy_len + i] = byte;
                    }
                }
            } else {
                // Copy main name as-is (fits in 8.3)
                let copy_len = core::cmp::min(main_bytes.len(), 8);
                for i in 0..copy_len {
                    sfn[i] = main_bytes[i];
                }
            }
            
            // Copy extension (up to 3 characters)
            let ext_len = core::cmp::min(ext_bytes.len(), 3);
            for i in 0..ext_len {
                sfn[8 + i] = ext_bytes[i];
            }
        } else {
            // No extension
            let main_name_upper = name.to_ascii_uppercase();
            let main_bytes: Vec<u8> = main_name_upper.bytes()
                .filter(|&b| Self::is_valid_sfn_char(b))
                .collect();
            
            // Check if we need numeric tail (long filename or case conversion)
            let needs_numeric_tail = main_bytes.len() > 8 || name != main_name_upper;
            
            if needs_numeric_tail {
                // Generate numeric tail format: BASENAME~N
                let tail_str = format!("~{}", numeric_tail);
                let available_chars = 8 - tail_str.len();
                
                // Copy base name (truncated to fit tail)
                let copy_len = core::cmp::min(main_bytes.len(), available_chars);
                for i in 0..copy_len {
                    sfn[i] = main_bytes[i];
                }
                
                // Add numeric tail
                for (i, byte) in tail_str.bytes().enumerate() {
                    if copy_len + i < 8 {
                        sfn[copy_len + i] = byte;
                    }
                }
            } else {
                // Copy name as-is (fits in 8 chars)
                let copy_len = core::cmp::min(main_bytes.len(), 8);
                for i in 0..copy_len {
                    sfn[i] = main_bytes[i];
                }
            }
        }
        
        sfn
    }
    
    /// Check if a character is valid for SFN
    fn is_valid_sfn_char(c: u8) -> bool {
        match c {
            // Invalid characters for SFN
            b'"' | b'*' | b'+' | b',' | b'/' | b':' | b';' | b'<' | b'=' | 
            b'>' | b'?' | b'[' | b'\\' | b']' | b'|' | b' ' => false,
            // Control characters (0x00-0x1F)
            0x00..=0x1F => false,
            // Valid characters
            _ => true,
        }
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



/// Internal data structures for Scarlet's FAT32 implementation
///
/// This module provides high-level, easy-to-use representations of FAT32 data
/// for internal use within the filesystem implementation.

/// Internal representation of a FAT32 directory entry for Scarlet's use
///
/// This structure combines SFN and LFN information into a single, easy-to-use format.
/// It abstracts away the complexities of the on-disk format while providing all
/// necessary information for filesystem operations.
#[derive(Debug, Clone)]
pub struct Fat32DirectoryEntryInternal {
    /// The primary filename (preferring LFN if available, otherwise SFN)
    pub filename: String,
    /// Short filename (8.3 format) for compatibility
    pub short_filename: String,
    /// File attributes
    pub attributes: u8,
    /// Starting cluster number
    pub start_cluster: u32,
    /// File size in bytes
    pub file_size: u32,
    /// Creation time and date information
    pub creation_time: FileTime,
    /// Last modification time and date information
    pub modification_time: FileTime,
    /// Last access date
    pub last_access_date: u16,
}

/// Time and date information for files
#[derive(Debug, Clone, Copy)]
pub struct FileTime {
    /// Time (packed format)
    pub time: u16,
    /// Date (packed format)  
    pub date: u16,
    /// Tenths of a second for creation time
    pub tenths: u8,
}

impl Fat32DirectoryEntryInternal {
    /// Create from a raw FAT32 directory entry
    pub fn from_raw_entry(raw_entry: &Fat32DirectoryEntry) -> Self {
        let short_filename = Self::parse_sfn(&raw_entry.name);
        
        Self {
            filename: short_filename.clone(), // Will be updated with LFN if available
            short_filename,
            attributes: raw_entry.attributes,
            start_cluster: raw_entry.cluster(),
            file_size: raw_entry.file_size,
            creation_time: FileTime {
                time: raw_entry.creation_time,
                date: raw_entry.creation_date,
                tenths: raw_entry.creation_time_tenths,
            },
            modification_time: FileTime {
                time: raw_entry.modification_time,
                date: raw_entry.modification_date,
                tenths: 0,
            },
            last_access_date: raw_entry.last_access_date,
        }
    }
    
    /// Create from a raw FAT32 directory entry (legacy compatibility)
    pub fn from_raw(raw_entry: Fat32DirectoryEntry) -> Self {
        Self::from_raw_entry(&raw_entry)
    }
    
    /// Get the primary filename (preferring LFN if different from SFN)
    pub fn name(&self) -> String {
        self.filename.clone()
    }
    
    /// Get the cluster number  
    pub fn cluster(&self) -> u32 {
        self.start_cluster
    }
    
    /// Get the file size
    pub fn size(&self) -> u32 {
        self.file_size
    }
    
    /// Set the long filename (used when LFN entries are available)
    pub fn set_long_filename(&mut self, lfn: String) {
        self.filename = lfn;
    }
    
    /// Check if this entry represents a directory
    pub fn is_directory(&self) -> bool {
        (self.attributes & 0x10) != 0
    }
    
    /// Check if this entry represents a regular file
    pub fn is_file(&self) -> bool {
        !self.is_directory() && (self.attributes & 0x08) == 0
    }
    
    /// Check if this entry is hidden
    pub fn is_hidden(&self) -> bool {
        (self.attributes & 0x02) != 0
    }
    
    /// Check if this entry is read-only
    pub fn is_read_only(&self) -> bool {
        (self.attributes & 0x01) != 0
    }
    
    /// Parse SFN (8.3 format) into a readable filename
    fn parse_sfn(name: &[u8; 11]) -> String {
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
            result.push((name[i] as char).to_ascii_lowercase());
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
                result.push((name[i] as char).to_ascii_lowercase());
            }
        }
        
        result
    }
    
    /// Convert to a raw FAT32 directory entry for writing to disk
    pub fn to_raw_entry(&self) -> Fat32DirectoryEntry {
        let mut raw_entry = Fat32DirectoryEntry {
            name: [b' '; 11],
            attributes: self.attributes,
            nt_reserved: 0,
            creation_time_tenths: self.creation_time.tenths,
            creation_time: self.creation_time.time,
            creation_date: self.creation_time.date,
            last_access_date: self.last_access_date,
            cluster_high: (self.start_cluster >> 16) as u16,
            modification_time: self.modification_time.time,
            modification_date: self.modification_time.date,
            cluster_low: (self.start_cluster & 0xFFFF) as u16,
            file_size: self.file_size,
        };
        
        // Generate and set the short filename from the original filename
        let sfn = Fat32DirectoryEntry::generate_sfn(&self.filename, 1);
        raw_entry.set_name(sfn);
        raw_entry
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

/// Long File Name (LFN) directory entry
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct Fat32LFNEntry {
    /// Sequence number (1-based, with 0x40 bit for last entry)
    pub sequence: u8,
    /// First 5 characters (10 bytes, UTF-16LE)
    pub name1: [u16; 5],
    /// Attributes (always 0x0F for LFN)
    pub attributes: u8,
    /// Type (always 0 for LFN)
    pub entry_type: u8,
    /// Checksum of corresponding SFN
    pub checksum: u8,
    /// Characters 6-11 (12 bytes, UTF-16LE)
    pub name2: [u16; 6],
    /// First cluster (always 0 for LFN)
    pub cluster: u16,
    /// Characters 12-13 (4 bytes, UTF-16LE)
    pub name3: [u16; 2],
}

impl Fat32LFNEntry {
    /// Check if this is an LFN entry
    pub fn is_lfn(&self) -> bool {
        self.attributes == 0x0F
    }
    
    /// Check if this is the last LFN entry in sequence
    pub fn is_last_lfn(&self) -> bool {
        (self.sequence & 0x40) != 0
    }
    
    /// Get sequence number (without the last entry flag)
    pub fn sequence_number(&self) -> u8 {
        self.sequence & 0x3F
    }
    
    /// Extract characters from this LFN entry
    pub fn extract_chars(&self) -> Vec<u16> {
        let mut chars = Vec::new();
        
        // Add name1 (5 chars) - use read_unaligned for packed struct
        let name1_copy = self.name1;
        for ch in name1_copy {
            if ch != 0 && ch != 0xFFFF {
                chars.push(ch);
            }
        }
        
        // Add name2 (6 chars) - use read_unaligned for packed struct
        let name2_copy = self.name2;
        for ch in name2_copy {
            if ch != 0 && ch != 0xFFFF {
                chars.push(ch);
            }
        }
        
        // Add name3 (2 chars) - use read_unaligned for packed struct
        let name3_copy = self.name3;
        for ch in name3_copy {
            if ch != 0 && ch != 0xFFFF {
                chars.push(ch);
            }
        }
        
        chars
    }
}

/// Builder for constructing Fat32DirectoryEntryInternal entries
pub struct Fat32DirectoryEntryBuilder {
    entry: Fat32DirectoryEntryInternal,
}

impl Fat32DirectoryEntryBuilder {
    /// Create a new builder for a file entry
    pub fn new_file(filename: &str, cluster: u32, size: u32) -> Self {
        let short_filename = Self::generate_short_filename(filename);
        
        Self {
            entry: Fat32DirectoryEntryInternal {
                filename: filename.to_string(),
                short_filename,
                attributes: 0, // Regular file
                start_cluster: cluster,
                file_size: size,
                creation_time: FileTime { time: 0, date: 0, tenths: 0 },
                modification_time: FileTime { time: 0, date: 0, tenths: 0 },
                last_access_date: 0,
            },
        }
    }
    
    /// Create a new builder for a directory entry
    pub fn new_directory(dirname: &str, cluster: u32) -> Self {
        let short_filename = Self::generate_short_filename(dirname);
        
        Self {
            entry: Fat32DirectoryEntryInternal {
                filename: dirname.to_string(),
                short_filename,
                attributes: 0x10, // Directory
                start_cluster: cluster,
                file_size: 0, // Directories have size 0
                creation_time: FileTime { time: 0, date: 0, tenths: 0 },
                modification_time: FileTime { time: 0, date: 0, tenths: 0 },
                last_access_date: 0,
            },
        }
    }
    
    /// Set attributes
    pub fn attributes(mut self, attrs: u8) -> Self {
        self.entry.attributes |= attrs;
        self
    }
    
    /// Build the final entry
    pub fn build(self) -> Fat32DirectoryEntryInternal {
        self.entry
    }
    
    /// Generate a short filename from a long filename
    fn generate_short_filename(filename: &str) -> String {
        // Simple implementation - truncate to 8.3 format
        if let Some(dot_pos) = filename.rfind('.') {
            let name_part = &filename[..dot_pos];
            let ext_part = &filename[dot_pos + 1..];
            
            let short_name = if name_part.len() <= 8 {
                name_part.to_string()
            } else {
                format!("{:.6}~1", name_part)
            };
            
            let short_ext = if ext_part.len() <= 3 {
                ext_part.to_string()
            } else {
                ext_part[..3].to_string()
            };
            
            format!("{}.{}", short_name, short_ext).to_uppercase()
        } else {
            // No extension
            if filename.len() <= 8 {
                filename.to_uppercase()
            } else {
                format!("{:.6}~1", filename).to_uppercase()
            }
        }
    }
}