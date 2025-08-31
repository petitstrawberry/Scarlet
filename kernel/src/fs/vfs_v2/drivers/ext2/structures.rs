//! Ext2 Filesystem Data Structures
//!
//! This module defines the on-disk data structures used by the ext2 filesystem.
//! These structures are laid out according to the ext2 specification and are
//! used for reading and writing filesystem metadata.

use core::mem;

/// Ext2 Superblock structure
/// 
/// The superblock contains metadata about the entire filesystem.
/// It is located at offset 1024 bytes from the start of the partition.
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
    pub log_frag_size: i32,
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
    pub max_mnt_count: i16,
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
    /// OS that created the filesystem
    pub creator_os: u32,
    /// Revision level
    pub rev_level: u32,
    /// Default uid for reserved blocks
    pub def_resuid: u16,
    /// Default gid for reserved blocks
    pub def_resgid: u16,
    
    // Extended fields (rev_level >= 1)
    /// First non-reserved inode
    pub first_ino: u32,
    /// Size of inode structure
    pub inode_size: u16,
    /// Block group number of this superblock
    pub block_group_nr: u16,
    /// Compatible feature set
    pub feature_compat: u32,
    /// Incompatible feature set
    pub feature_incompat: u32,
    /// Read-only compatible feature set
    pub feature_ro_compat: u32,
    /// UUID of filesystem
    pub uuid: [u8; 16],
    /// Volume name
    pub volume_name: [u8; 16],
    /// Directory where last mounted
    pub last_mounted: [u8; 64],
    /// Compression algorithms used
    pub algorithm_usage_bitmap: u32,
    
    // Performance hints
    /// Number of blocks to preallocate for files
    pub prealloc_blocks: u8,
    /// Number of blocks to preallocate for directories
    pub prealloc_dir_blocks: u8,
    /// Padding
    pub _padding1: u16,
    
    // Journaling support
    /// UUID of journal superblock
    pub journal_uuid: [u8; 16],
    /// Inode number of journal file
    pub journal_inum: u32,
    /// Device number of journal file
    pub journal_dev: u32,
    /// Start of list of inodes to delete
    pub last_orphan: u32,
    
    // Directory indexing support
    /// HTREE hash seed
    pub hash_seed: [u32; 4],
    /// Default hash version to use
    pub def_hash_version: u8,
    /// Padding
    pub _padding2: [u8; 3],
    
    // Other options
    /// Default mount options
    pub default_mount_opts: u32,
    /// First metablock block group
    pub first_meta_bg: u32,
    /// Padding to end of 1024-byte block
    pub _padding3: [u8; 760],
}

/// Ext2 Block Group Descriptor
/// 
/// Each block group has a descriptor that contains pointers to
/// the block bitmap, inode bitmap, and inode table for that group.
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct Ext2BlockGroupDescriptor {
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
    pub _padding: u16,
    /// Reserved for future use
    pub _reserved: [u32; 3],
}

/// Ext2 Inode structure
/// 
/// Inodes contain metadata about files and directories.
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct Ext2Inode {
    /// File mode
    pub mode: u16,
    /// Owner uid
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
    /// Group id
    pub gid: u16,
    /// Links count
    pub links_count: u16,
    /// Blocks count (in 512-byte sectors)
    pub blocks: u32,
    /// File flags
    pub flags: u32,
    /// OS dependent 1
    pub osd1: u32,
    /// Pointers to blocks
    pub block: [u32; EXT2_N_BLOCKS],
    /// File version (for NFS)
    pub generation: u32,
    /// File ACL
    pub file_acl: u32,
    /// Directory ACL / High 32 bits of file size
    pub dir_acl: u32,
    /// Fragment address
    pub faddr: u32,
    /// OS dependent 2
    pub osd2: [u8; 12],
}

/// Ext2 Directory Entry
/// 
/// Directory entries have variable length and contain the inode number
/// and name of files/subdirectories.
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct Ext2DirectoryEntry {
    /// Inode number
    pub inode: u32,
    /// Directory entry length
    pub rec_len: u16,
    /// Name length
    pub name_len: u8,
    /// File type (ext2 rev >= 1 only)
    pub file_type: u8,
    // Name follows immediately after this structure
}

impl Ext2DirectoryEntry {
    /// Get the name from a directory entry buffer
    pub fn name<'a>(&self, buffer: &'a [u8], offset: usize) -> Option<&'a [u8]> {
        let name_start = offset + mem::size_of::<Ext2DirectoryEntry>();
        let name_end = name_start + self.name_len as usize;
        
        if name_end <= buffer.len() {
            Some(&buffer[name_start..name_end])
        } else {
            None
        }
    }
}

// Constants

/// Ext2 magic number
pub const EXT2_SUPER_MAGIC: u16 = 0xEF53;

/// Filesystem states
pub const EXT2_VALID_FS: u16 = 1;   // Unmounted cleanly
pub const EXT2_ERROR_FS: u16 = 2;   // Errors detected

/// Revision levels
pub const EXT2_GOOD_OLD_REV: u32 = 0;   // Original ext2 revision
pub const EXT2_DYNAMIC_REV: u32 = 1;    // First revision with dynamic features

/// Number of block pointers in inode
pub const EXT2_N_BLOCKS: usize = 15;
pub const EXT2_NDIR_BLOCKS: usize = 12;
pub const EXT2_IND_BLOCK: usize = 12;
pub const EXT2_DIND_BLOCK: usize = 13;
pub const EXT2_TIND_BLOCK: usize = 14;

/// File types (for directory entries)
pub const EXT2_FT_UNKNOWN: u8 = 0;
pub const EXT2_FT_REG_FILE: u8 = 1;
pub const EXT2_FT_DIR: u8 = 2;
pub const EXT2_FT_CHRDEV: u8 = 3;
pub const EXT2_FT_BLKDEV: u8 = 4;
pub const EXT2_FT_FIFO: u8 = 5;
pub const EXT2_FT_SOCK: u8 = 6;
pub const EXT2_FT_SYMLINK: u8 = 7;

/// File mode constants
pub const EXT2_S_IFMT: u16 = 0xF000;   // File type mask
pub const EXT2_S_IFSOCK: u16 = 0xC000; // Socket
pub const EXT2_S_IFLNK: u16 = 0xA000;  // Symbolic link
pub const EXT2_S_IFREG: u16 = 0x8000;  // Regular file
pub const EXT2_S_IFBLK: u16 = 0x6000;  // Block device
pub const EXT2_S_IFDIR: u16 = 0x4000;  // Directory
pub const EXT2_S_IFCHR: u16 = 0x2000;  // Character device
pub const EXT2_S_IFIFO: u16 = 0x1000;  // FIFO

// Permission bits
pub const EXT2_S_ISUID: u16 = 0x0800;  // Set UID
pub const EXT2_S_ISGID: u16 = 0x0400;  // Set GID
pub const EXT2_S_ISVTX: u16 = 0x0200;  // Sticky bit

pub const EXT2_S_IRUSR: u16 = 0x0100;  // User read
pub const EXT2_S_IWUSR: u16 = 0x0080;  // User write
pub const EXT2_S_IXUSR: u16 = 0x0040;  // User execute

pub const EXT2_S_IRGRP: u16 = 0x0020;  // Group read
pub const EXT2_S_IWGRP: u16 = 0x0010;  // Group write
pub const EXT2_S_IXGRP: u16 = 0x0008;  // Group execute

pub const EXT2_S_IROTH: u16 = 0x0004;  // Other read
pub const EXT2_S_IWOTH: u16 = 0x0002;  // Other write
pub const EXT2_S_IXOTH: u16 = 0x0001;  // Other execute

/// Special inode numbers
pub const EXT2_BAD_INO: u32 = 1;       // Bad blocks inode
pub const EXT2_ROOT_INO: u32 = 2;      // Root directory inode
pub const EXT2_ACL_IDX_INO: u32 = 3;   // ACL index inode
pub const EXT2_ACL_DATA_INO: u32 = 4;  // ACL data inode
pub const EXT2_BOOT_LOADER_INO: u32 = 5; // Boot loader inode
pub const EXT2_UNDEL_DIR_INO: u32 = 6; // Undelete directory inode
pub const EXT2_FIRST_INO: u32 = 11;    // First non-reserved inode

// Ensure structures have correct sizes
const _: () = assert!(mem::size_of::<Ext2Superblock>() == 1024);
const _: () = assert!(mem::size_of::<Ext2BlockGroupDescriptor>() == 32);
const _: () = assert!(mem::size_of::<Ext2Inode>() == 128);
const _: () = assert!(mem::size_of::<Ext2DirectoryEntry>() == 8);