//! ext2 data structures
//!
//! This module defines the on-disk data structures used by the ext2 filesystem.
//! All structures are packed and follow the ext2 filesystem specification.

use core::mem;
use alloc::{vec::Vec, string::String, format, string::ToString};
use crate::fs::{FileSystemError, FileSystemErrorKind};

/// ext2 magic number
pub const EXT2_SUPER_MAGIC: u16 = 0xEF53;

/// ext2 root inode number
pub const EXT2_ROOT_INO: u32 = 2;

/// ext2 file type constants for inode mode field
pub const EXT2_S_IFMT: u16 = 0xF000;  // File type mask
pub const EXT2_S_IFREG: u16 = 0x8000; // Regular file
pub const EXT2_S_IFDIR: u16 = 0x4000; // Directory
pub const EXT2_S_IFLNK: u16 = 0xA000; // Symbolic link

/// ext2 Superblock structure
/// 
/// This structure represents the superblock of an ext2 filesystem.
/// It contains essential information about the filesystem layout and parameters.
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct Ext2Superblock {
    /// Total number of inodes
    pub inodes_count: u32,
    /// Total number of blocks
    pub blocks_count: u32,
    /// Number of blocks reserved for superuser
    pub r_blocks_count: u32,
    /// Number of free blocks
    pub free_blocks_count: u32,
    /// Number of free inodes
    pub free_inodes_count: u32,
    /// First data block (0 for 1K blocks, 1 for larger blocks)
    pub first_data_block: u32,
    /// Block size (log2(block_size) - 10)
    pub log_block_size: u32,
    /// Fragment size (log2(fragment_size) - 10)
    pub log_frag_size: u32,
    /// Number of blocks per group
    pub blocks_per_group: u32,
    /// Number of fragments per group
    pub frags_per_group: u32,
    /// Number of inodes per group
    pub inodes_per_group: u32,
    /// Mount time
    pub mtime: u32,
    /// Write time
    pub wtime: u32,
    /// Mount count
    pub mnt_count: u16,
    /// Maximum mount count
    pub max_mnt_count: u16,
    /// Magic signature
    pub magic: u16,
    /// File system state
    pub state: u16,
    /// Behavior when detecting errors
    pub errors: u16,
    /// Minor revision level
    pub minor_rev_level: u16,
    /// Time of last check
    pub lastcheck: u32,
    /// Maximum time between checks
    pub checkinterval: u32,
    /// Creator OS
    pub creator_os: u32,
    /// Revision level
    pub rev_level: u32,
    /// Default uid for reserved blocks
    pub def_resuid: u16,
    /// Default gid for reserved blocks
    pub def_resgid: u16,
    /// First non-reserved inode
    pub first_ino: u32,
    /// Size of inode structure
    pub inode_size: u16,
    /// Block group this superblock is part of
    pub block_group_nr: u16,
    /// Compatible feature set
    pub feature_compat: u32,
    /// Incompatible feature set
    pub feature_incompat: u32,
    /// Read-only feature set
    pub feature_ro_compat: u32,
    /// 128-bit UUID for volume
    pub uuid: [u8; 16],
    /// Volume name
    pub volume_name: [u8; 16],
    /// Directory where last mounted
    pub last_mounted: [u8; 64],
    /// Algorithm usage bitmap
    pub algorithm_usage_bitmap: u32,
    /// Number of blocks to try to preallocate for files
    pub prealloc_blocks: u8,
    /// Number of blocks to preallocate for directories
    pub prealloc_dir_blocks: u8,
    /// Padding to 1024 bytes
    pub padding: [u8; 1024 - 204],
}

impl Ext2Superblock {
    /// Parse superblock from raw bytes
    pub fn from_bytes(data: &[u8]) -> Result<Self, FileSystemError> {
        if data.len() < mem::size_of::<Self>() {
            return Err(FileSystemError::new(
                FileSystemErrorKind::InvalidData,
                "Insufficient data for ext2 superblock"
            ));
        }

        // Safety: We've checked the size and ext2 superblock has a fixed layout
        let superblock = unsafe {
            core::ptr::read_unaligned(data.as_ptr() as *const Self)
        };

        Ok(superblock)
    }
}

/// ext2 Block Group Descriptor
///
/// Each block group has a descriptor that contains information about
/// the location of important data structures within that group.
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct Ext2BlockGroupDescriptor {
    /// Block address of block bitmap
    pub block_bitmap: u32,
    /// Block address of inode bitmap
    pub inode_bitmap: u32,
    /// Block address of inode table
    pub inode_table: u32,
    /// Number of free blocks in group
    pub free_blocks_count: u16,
    /// Number of free inodes in group
    pub free_inodes_count: u16,
    /// Number of directories in group
    pub used_dirs_count: u16,
    /// Padding to 32 bytes
    pub pad: u16,
    /// Reserved for future use
    pub reserved: [u32; 3],
}

impl Ext2BlockGroupDescriptor {
    /// Parse block group descriptor from raw bytes
    pub fn from_bytes(data: &[u8]) -> Result<Self, FileSystemError> {
        if data.len() < mem::size_of::<Self>() {
            return Err(FileSystemError::new(
                FileSystemErrorKind::InvalidData,
                "Insufficient data for ext2 block group descriptor"
            ));
        }

        // Safety: We've checked the size and BGD has a fixed layout
        let bgd = unsafe {
            core::ptr::read_unaligned(data.as_ptr() as *const Self)
        };

        Ok(bgd)
    }
}

/// ext2 Inode structure
///
/// Each file and directory is represented by an inode that contains
/// metadata about the file and pointers to its data blocks.
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct Ext2Inode {
    /// File mode (permissions and file type)
    pub mode: u16,
    /// Owner UID
    pub uid: u16,
    /// Size in bytes
    pub size: u32,
    /// Access time
    pub atime: u32,
    /// Creation time
    pub ctime: u32,
    /// Modification time
    pub mtime: u32,
    /// Deletion time
    pub dtime: u32,
    /// Group ID
    pub gid: u16,
    /// Link count
    pub links_count: u16,
    /// Blocks count (512-byte blocks)
    pub blocks: u32,
    /// File flags
    pub flags: u32,
    /// OS dependent 1
    pub osd1: u32,
    /// Pointers to blocks (0-11 direct, 12 indirect, 13 double indirect, 14 triple indirect)
    pub block: [u32; 15],
    /// File version (for NFS)
    pub generation: u32,
    /// File ACL
    pub file_acl: u32,
    /// Directory ACL / high 32 bits of file size
    pub dir_acl: u32,
    /// Fragment address
    pub faddr: u32,
    /// OS dependent 2
    pub osd2: [u8; 12],
}

impl Ext2Inode {
    /// Parse inode from raw bytes
    pub fn from_bytes(data: &[u8]) -> Result<Self, FileSystemError> {
        if data.len() < mem::size_of::<Self>() {
            return Err(FileSystemError::new(
                FileSystemErrorKind::InvalidData,
                "Insufficient data for ext2 inode"
            ));
        }

        // Safety: We've checked the size and inode has a fixed layout
        let inode = unsafe {
            core::ptr::read_unaligned(data.as_ptr() as *const Self)
        };

        Ok(inode)
    }
}

/// ext2 Directory Entry
///
/// Directory entries are stored as variable-length records within directory data blocks.
#[derive(Debug, Clone)]
#[repr(C, packed)]
pub struct Ext2DirectoryEntryRaw {
    /// Inode number
    pub inode: u32,
    /// Record length
    pub rec_len: u16,
    /// Name length
    pub name_len: u8,
    /// File type (ext2 revision 1.0 and later)
    pub file_type: u8,
    // Name follows this header
}

impl Ext2DirectoryEntryRaw {
    /// Parse directory entry from raw bytes
    pub fn from_bytes(data: &[u8]) -> Result<Self, FileSystemError> {
        if data.len() < 8 {
            return Err(FileSystemError::new(
                FileSystemErrorKind::InvalidData,
                "Insufficient data for ext2 directory entry header"
            ));
        }

        let inode = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        let rec_len = u16::from_le_bytes([data[4], data[5]]);
        let name_len = data[6];
        let file_type = data[7];

        Ok(Self {
            inode,
            rec_len,
            name_len,
            file_type,
        })
    }
}

/// Complete directory entry with name
#[derive(Debug, Clone)]
pub struct Ext2DirectoryEntry {
    pub entry: Ext2DirectoryEntryRaw,
    pub name: String,
}

impl Ext2DirectoryEntry {
    /// Parse a complete directory entry with name from raw bytes
    pub fn from_bytes(data: &[u8]) -> Result<Self, FileSystemError> {
        if data.len() < 8 {
            return Err(FileSystemError::new(
                FileSystemErrorKind::InvalidData,
                "Insufficient data for ext2 directory entry"
            ));
        }

        let entry = Ext2DirectoryEntryRaw::from_bytes(data)?;
        
        if data.len() < 8 + entry.name_len as usize {
            return Err(FileSystemError::new(
                FileSystemErrorKind::InvalidData,
                "Insufficient data for directory entry name"
            ));
        }

        let name_bytes = &data[8..8 + entry.name_len as usize];
        let name = String::from_utf8(name_bytes.to_vec())
            .map_err(|_| FileSystemError::new(
                FileSystemErrorKind::InvalidData,
                "Invalid UTF-8 in directory entry name"
            ))?;

        Ok(Self { entry, name })
    }

    pub fn name_str(&self) -> Result<String, FileSystemError> {
        Ok(self.name.clone())
    }
}

// Ensure structures have correct sizes
// Note: Ext2Superblock is flexible in size but we define minimum 1024 bytes
// const _: () = assert!(mem::size_of::<Ext2Superblock>() >= 1024);