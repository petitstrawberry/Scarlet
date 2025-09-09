//! ext2 data structures
//!
//! This module defines the on-disk data structures used by the ext2 filesystem.
//! All structures are packed and follow the ext2 filesystem specification.

use core::mem;
use alloc::{boxed::Box, vec::Vec, string::String, format, string::ToString};
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
pub const EXT2_S_IFCHR: u16 = 0x2000; // Character device
pub const EXT2_S_IFBLK: u16 = 0x6000; // Block device
pub const EXT2_S_IFIFO: u16 = 0x1000; // FIFO (pipe)
pub const EXT2_S_IFSOCK: u16 = 0xC000; // Socket

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
    /// Parse superblock from raw bytes using unsafe type conversion for efficiency
    pub fn from_bytes(data: &[u8]) -> Result<Self, FileSystemError> {
        // Standard ext2 superblock is 1024 bytes, but our struct might be slightly larger due to alignment
        if data.len() < 1024 {
            return Err(FileSystemError::new(
                FileSystemErrorKind::InvalidData,
                format!("Insufficient data for ext2 superblock: got {} bytes, need at least 1024 bytes", data.len())
            ));
        }

        // Use unsafe cast for efficiency since ext2 structures are packed and have fixed layout
        let superblock = unsafe {
            // Ensure proper alignment by copying to stack
            let mut aligned_data = [0u8; 1024];
            aligned_data[..1024].copy_from_slice(&data[..1024]);
            *(aligned_data.as_ptr() as *const Self)
        };

        // Validate magic number to ensure we have a valid ext2 superblock
        if u16::from_le(superblock.magic) != EXT2_SUPER_MAGIC {
            return Err(FileSystemError::new(
                FileSystemErrorKind::InvalidData,
                "Invalid ext2 magic number"
            ));
        }

        Ok(superblock)
    }

    /// Parse superblock from raw bytes and return as Box to avoid stack overflow
    pub fn from_bytes_boxed(data: &[u8]) -> Result<Box<Self>, FileSystemError> {
        // Standard ext2 superblock is 1024 bytes, but our struct might be slightly larger due to alignment
        if data.len() < 1024 {
            return Err(FileSystemError::new(
                FileSystemErrorKind::InvalidData,
                format!("Insufficient data for ext2 superblock: got {} bytes, need at least 1024 bytes", data.len())
            ));
        }

        // Use heap allocation to avoid stack overflow
        // Create data on heap and transmute directly without going through stack
        let aligned_data = data[..1024].to_vec().into_boxed_slice();
        
        let superblock = unsafe {
            // Directly convert Box<[u8]> to Box<Self> without stack copy
            Box::from_raw(Box::into_raw(aligned_data) as *mut Self)
        };

        // Validate magic number to ensure we have a valid ext2 superblock
        if u16::from_le(superblock.magic) != EXT2_SUPER_MAGIC {
            return Err(FileSystemError::new(
                FileSystemErrorKind::InvalidData,
                "Invalid ext2 magic number"
            ));
        }

        Ok(superblock)
    }

    /// Get block size in bytes
    pub fn get_block_size(&self) -> u32 {
        1024 << u32::from_le(self.log_block_size)
    }

    /// Get total blocks count
    pub fn get_blocks_count(&self) -> u32 {
        u32::from_le(self.blocks_count)
    }

    /// Get total inodes count  
    pub fn get_inodes_count(&self) -> u32 {
        u32::from_le(self.inodes_count)
    }

    /// Get blocks per group
    pub fn get_blocks_per_group(&self) -> u32 {
        u32::from_le(self.blocks_per_group)
    }

    /// Get inodes per group
    pub fn get_inodes_per_group(&self) -> u32 {
        u32::from_le(self.inodes_per_group)
    }

    /// Get inode size
    pub fn get_inode_size(&self) -> u16 {
        u16::from_le(self.inode_size)
    }

    /// Get first data block
    pub fn get_first_data_block(&self) -> u32 {
        u32::from_le(self.first_data_block)
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
    /// Parse block group descriptor from raw bytes using unsafe type conversion
    pub fn from_bytes(data: &[u8]) -> Result<Self, FileSystemError> {
        if data.len() < mem::size_of::<Self>() {
            return Err(FileSystemError::new(
                FileSystemErrorKind::InvalidData,
                "Insufficient data for ext2 block group descriptor"
            ));
        }

        // Use unsafe cast for efficiency since the structure is packed and has fixed layout
        let descriptor = unsafe {
            *(data.as_ptr() as *const Self)
        };

        Ok(descriptor)
    }

    /// Get block bitmap address
    pub fn get_block_bitmap(&self) -> u32 {
        u32::from_le(self.block_bitmap)
    }

    /// Get inode bitmap address
    pub fn get_inode_bitmap(&self) -> u32 {
        u32::from_le(self.inode_bitmap)
    }

    /// Get inode table address
    pub fn get_inode_table(&self) -> u32 {
        u32::from_le(self.inode_table)
    }

    /// Get free blocks count
    pub fn get_free_blocks_count(&self) -> u16 {
        u16::from_le(self.free_blocks_count)
    }

    /// Set free blocks count
    pub fn set_free_blocks_count(&mut self, count: u16) {
        self.free_blocks_count = count.to_le();
    }

    /// Get free inodes count
    pub fn get_free_inodes_count(&self) -> u16 {
        u16::from_le(self.free_inodes_count)
    }

    /// Set free inodes count
    pub fn set_free_inodes_count(&mut self, count: u16) {
        self.free_inodes_count = count.to_le();
    }

    /// Get used directories count
    pub fn get_used_dirs_count(&self) -> u16 {
        u16::from_le(self.used_dirs_count)
    }

    /// Set used directories count
    pub fn set_used_dirs_count(&mut self, count: u16) {
        self.used_dirs_count = count.to_le();
    }

    /// Write the descriptor back to bytes
    pub fn write_to_bytes(&self, data: &mut [u8]) {
        if data.len() >= mem::size_of::<Self>() {
            unsafe {
                let ptr = data.as_mut_ptr() as *mut Self;
                *ptr = *self;
            }
        }
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
    pub fn empty() -> Self {
        Self {
            mode: 0,
            uid: 0,
            size: 0,
            atime: 0,
            ctime: 0,
            mtime: 0,
            dtime: 0,
            gid: 0,
            links_count: 0,
            blocks: 0,
            flags: 0,
            osd1: 0,
            block: [0; 15],
            generation: 0,
            file_acl: 0,
            dir_acl: 0,
            faddr: 0,
            osd2: [0; 12],
        }
    }
    /// Parse inode from raw bytes using unsafe type conversion for efficiency
    pub fn from_bytes(data: &[u8]) -> Result<Self, FileSystemError> {
        if data.len() < mem::size_of::<Self>() {
            return Err(FileSystemError::new(
                FileSystemErrorKind::InvalidData,
                "Insufficient data for ext2 inode"
            ));
        }

        // Use unsafe cast for efficiency since the inode structure is packed and has fixed layout
        let inode = unsafe {
            *(data.as_ptr() as *const Self)
        };

        Ok(inode)
    }

    /// Get file mode (permissions and type)
    pub fn get_mode(&self) -> u16 {
        u16::from_le(self.mode)
    }

    /// Get file size in bytes
    pub fn get_size(&self) -> u32 {
        u32::from_le(self.size)
    }

    /// Get modification time
    pub fn get_mtime(&self) -> u32 {
        u32::from_le(self.mtime)
    }

    /// Get access time
    pub fn get_atime(&self) -> u32 {
        u32::from_le(self.atime)
    }

    /// Get creation time
    pub fn get_ctime(&self) -> u32 {
        u32::from_le(self.ctime)
    }

    /// Get link count
    pub fn get_links_count(&self) -> u16 {
        u16::from_le(self.links_count)
    }

    /// Get blocks count (512-byte blocks)
    pub fn get_blocks(&self) -> u32 {
        u32::from_le(self.blocks)
    }

    /// Get block pointer at index
    pub fn get_block(&self, index: usize) -> Option<u32> {
        if index < 15 {
            Some(u32::from_le(self.block[index]))
        } else {
            None
        }
    }

    /// Check if this is a directory
    pub fn is_dir(&self) -> bool {
        (self.get_mode() & EXT2_S_IFMT) == EXT2_S_IFDIR
    }

    /// Check if this is a regular file
    pub fn is_file(&self) -> bool {
        (self.get_mode() & EXT2_S_IFMT) == EXT2_S_IFREG
    }

    /// Check if this is a character device
    pub fn is_char_device(&self) -> bool {
        (self.get_mode() & EXT2_S_IFMT) == EXT2_S_IFCHR
    }

    /// Check if this is a block device
    pub fn is_block_device(&self) -> bool {
        (self.get_mode() & EXT2_S_IFMT) == EXT2_S_IFBLK
    }

    /// Check if this is a symbolic link
    pub fn is_symlink(&self) -> bool {
        (self.get_mode() & EXT2_S_IFMT) == EXT2_S_IFLNK
    }

    /// Check if this is a FIFO (pipe)
    pub fn is_fifo(&self) -> bool {
        (self.get_mode() & EXT2_S_IFMT) == EXT2_S_IFIFO
    }

    /// Check if this is a socket
    pub fn is_socket(&self) -> bool {
        (self.get_mode() & EXT2_S_IFMT) == EXT2_S_IFSOCK
    }

    /// Get device information for device files
    /// Returns (major, minor) device numbers
    pub fn get_device_info(&self) -> Option<(u32, u32)> {
        if self.is_char_device() || self.is_block_device() {
            // In ext2, device info is stored in the first direct block pointer
            let device_id = u32::from_le(self.block[0]);
            let major = (device_id >> 8) & 0xFF;
            let minor = device_id & 0xFF;
            Some((major, minor))
        } else {
            None
        }
    }
}

/// ext2 Directory Entry
///
/// Directory entries are stored as variable-length records within directory data blocks.
#[derive(Debug, Clone, Copy)]
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
    /// Parse directory entry from raw bytes using unsafe type conversion
    pub fn from_bytes(data: &[u8]) -> Result<Self, FileSystemError> {
        if data.len() < mem::size_of::<Self>() {
            return Err(FileSystemError::new(
                FileSystemErrorKind::InvalidData,
                "Insufficient data for ext2 directory entry header"
            ));
        }

        // Use unsafe cast for efficiency since the directory entry header is packed and fixed-size
        let entry = unsafe {
            *(data.as_ptr() as *const Self)
        };

        Ok(entry)
    }

    /// Get inode number
    pub fn get_inode(&self) -> u32 {
        u32::from_le(self.inode)
    }

    /// Get record length
    pub fn get_rec_len(&self) -> u16 {
        u16::from_le(self.rec_len)
    }

    /// Get name length
    pub fn get_name_len(&self) -> u8 {
        self.name_len
    }

    /// Get file type
    pub fn get_file_type(&self) -> u8 {
        self.file_type
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