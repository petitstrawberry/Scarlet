//! EXT2 data structures
//!
//! This module defines the on-disk data structures used by the EXT2 filesystem.
//! All structures are packed and follow the EXT2 filesystem specification.

use core::mem;

/// EXT2 Superblock structure
/// 
/// This structure represents the superblock of an EXT2 filesystem.
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
    /// Block number of first data block
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
    /// Magic signature (0xEF53)
    pub magic: u16,
    /// File system state
    pub state: u16,
    /// Behaviour when detecting errors
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
}

/// EXT2 Group Descriptor structure
/// 
/// Each block group has a group descriptor that contains metadata about the group.
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct Ext2GroupDescriptor {
    /// Block number of block bitmap
    pub block_bitmap: u32,
    /// Block number of inode bitmap
    pub inode_bitmap: u32,
    /// Block number of inode table
    pub inode_table: u32,
    /// Number of free blocks in group
    pub free_blocks_count: u16,
    /// Number of free inodes in group
    pub free_inodes_count: u16,
    /// Number of directories in group
    pub used_dirs_count: u16,
    /// Padding
    pub pad: u16,
    /// Reserved
    pub reserved: [u32; 3],
}

/// EXT2 Inode structure
/// 
/// This structure represents an inode in the EXT2 filesystem.
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct Ext2Inode {
    /// File mode
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
    /// Links count
    pub links_count: u16,
    /// Blocks count
    pub blocks: u32,
    /// File flags
    pub flags: u32,
    /// OS specific 1
    pub osd1: u32,
    /// Block pointers (12 direct + 1 indirect + 1 double indirect + 1 triple indirect)
    pub block: [u32; 15],
    /// File version (for NFS)
    pub generation: u32,
    /// File ACL
    pub file_acl: u32,
    /// Directory ACL / High 32 bits of file size
    pub dir_acl: u32,
    /// Fragment address
    pub faddr: u32,
    /// OS specific 2
    pub osd2: [u8; 12],
}

/// EXT2 Directory Entry structure
/// 
/// This structure represents a directory entry in an EXT2 directory.
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct Ext2DirectoryEntry {
    /// Inode number
    pub inode: u32,
    /// Record length
    pub rec_len: u16,
    /// Name length
    pub name_len: u8,
    /// File type
    pub file_type: u8,
    // Variable-length name follows
}

// EXT2 constants
pub const EXT2_MAGIC: u16 = 0xEF53;
pub const EXT2_GOOD_OLD_REV: u32 = 0;
pub const EXT2_DYNAMIC_REV: u32 = 1;

// Block size calculations
pub const EXT2_MIN_BLOCK_SIZE: u32 = 1024;
pub const EXT2_MAX_BLOCK_SIZE: u32 = 4096;

// File type constants for directory entries
pub const EXT2_FT_UNKNOWN: u8 = 0;
pub const EXT2_FT_REG_FILE: u8 = 1;
pub const EXT2_FT_DIR: u8 = 2;
pub const EXT2_FT_CHRDEV: u8 = 3;
pub const EXT2_FT_BLKDEV: u8 = 4;
pub const EXT2_FT_FIFO: u8 = 5;
pub const EXT2_FT_SOCK: u8 = 6;
pub const EXT2_FT_SYMLINK: u8 = 7;

// Inode mode constants
pub const EXT2_S_IFMT: u16 = 0xF000;   // File type mask
pub const EXT2_S_IFSOCK: u16 = 0xC000; // Socket
pub const EXT2_S_IFLNK: u16 = 0xA000;  // Symbolic link
pub const EXT2_S_IFREG: u16 = 0x8000;  // Regular file
pub const EXT2_S_IFBLK: u16 = 0x6000;  // Block device
pub const EXT2_S_IFDIR: u16 = 0x4000;  // Directory
pub const EXT2_S_IFCHR: u16 = 0x2000;  // Character device
pub const EXT2_S_IFIFO: u16 = 0x1000;  // FIFO

impl Ext2Superblock {
    /// Get the block size in bytes
    pub fn block_size(&self) -> u32 {
        1024 << self.log_block_size
    }
    
    /// Check if this is a valid EXT2 superblock
    pub fn is_valid(&self) -> bool {
        self.magic == EXT2_MAGIC
    }
    
    /// Get the number of block groups
    pub fn group_count(&self) -> u32 {
        (self.blocks_count + self.blocks_per_group - 1) / self.blocks_per_group
    }
}

impl Ext2Inode {
    /// Check if this inode represents a directory
    pub fn is_directory(&self) -> bool {
        (self.mode & EXT2_S_IFMT) == EXT2_S_IFDIR
    }
    
    /// Check if this inode represents a regular file
    pub fn is_regular_file(&self) -> bool {
        (self.mode & EXT2_S_IFMT) == EXT2_S_IFREG
    }
    
    /// Check if this inode represents a symbolic link
    pub fn is_symbolic_link(&self) -> bool {
        (self.mode & EXT2_S_IFMT) == EXT2_S_IFLNK
    }
    
    /// Get the direct block pointer at the given index
    pub fn direct_block(&self, index: usize) -> Option<u32> {
        if index < 12 {
            Some(self.block[index])
        } else {
            None
        }
    }
    
    /// Get the indirect block pointer
    pub fn indirect_block(&self) -> u32 {
        self.block[12]
    }
    
    /// Get the double indirect block pointer
    pub fn double_indirect_block(&self) -> u32 {
        self.block[13]
    }
    
    /// Get the triple indirect block pointer
    pub fn triple_indirect_block(&self) -> u32 {
        self.block[14]
    }
}

impl Ext2DirectoryEntry {
    /// Get the name of this directory entry
    /// 
    /// # Safety
    /// This function assumes that the name follows immediately after the fixed part
    /// and that name_len is correct.
    pub unsafe fn name(&self) -> &[u8] {
        let name_ptr = (self as *const Self as *const u8).offset(8);
        core::slice::from_raw_parts(name_ptr, self.name_len as usize)
    }
    
    /// Get the total size of this directory entry including the name
    pub fn total_size(&self) -> usize {
        8 + self.name_len as usize
    }
    
    /// Get the actual record length (aligned)
    pub fn record_length(&self) -> u16 {
        self.rec_len
    }
}