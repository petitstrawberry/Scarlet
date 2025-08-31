//! Ext2 Filesystem Tests
//!
//! This module contains tests for the ext2 filesystem implementation.

use alloc::{sync::Arc, vec};
use crate::{
    device::block::tests::disk::TestDisk,
    fs::{FileSystemError, FileSystemErrorKind, FileType}
};

use super::*;

#[test_case]
fn test_ext2_driver_creation() {
    let driver = Ext2Driver;
    assert_eq!(driver.name(), "ext2");
    assert_eq!(driver.filesystem_type(), crate::fs::FileSystemType::Block);
}

#[test_case]
fn test_ext2_driver_requires_block_device() {
    let driver = Ext2Driver;
    
    // Should fail without block device
    let result = driver.create();
    assert!(result.is_err());
    
    if let Err(e) = result {
        assert_eq!(e.kind(), FileSystemErrorKind::NotSupported);
    }
}

#[test_case]
fn test_ext2_superblock_validation() {
    // Create a mock superblock with invalid magic
    let mut superblock = Ext2Superblock {
        inodes_count: 1000,
        blocks_count: 8192,
        r_blocks_count: 400,
        free_blocks_count: 7000,
        free_inodes_count: 900,
        first_data_block: 1,
        log_block_size: 0, // 1024 bytes
        log_frag_size: 0,
        blocks_per_group: 8192,
        frags_per_group: 8192,
        inodes_per_group: 1000,
        mtime: 0,
        wtime: 0,
        mnt_count: 0,
        max_mnt_count: 20,
        magic: 0x1234, // Invalid magic
        state: EXT2_VALID_FS,
        errors: 1,
        minor_rev_level: 0,
        lastcheck: 0,
        checkinterval: 0,
        creator_os: 0,
        rev_level: EXT2_GOOD_OLD_REV,
        def_resuid: 0,
        def_resgid: 0,
        first_ino: EXT2_FIRST_INO,
        inode_size: 128,
        block_group_nr: 0,
        feature_compat: 0,
        feature_incompat: 0,
        feature_ro_compat: 0,
        uuid: [0; 16],
        volume_name: [0; 16],
        last_mounted: [0; 64],
        algorithm_usage_bitmap: 0,
        prealloc_blocks: 0,
        prealloc_dir_blocks: 0,
        _padding1: 0,
        journal_uuid: [0; 16],
        journal_inum: 0,
        journal_dev: 0,
        last_orphan: 0,
        hash_seed: [0; 4],
        def_hash_version: 0,
        _padding2: [0; 3],
        default_mount_opts: 0,
        first_meta_bg: 0,
        _padding3: [0; 760],
    };
    
    // Should fail with invalid magic
    let result = Ext2FileSystem::validate_ext2(&superblock);
    assert!(result.is_err());
    
    // Fix magic number
    superblock.magic = EXT2_SUPER_MAGIC;
    
    // Should succeed with valid magic
    let result = Ext2FileSystem::validate_ext2(&superblock);
    assert!(result.is_ok());
    
    // Test invalid state
    superblock.state = 99; // Invalid state
    let result = Ext2FileSystem::validate_ext2(&superblock);
    assert!(result.is_err());
    
    // Fix state
    superblock.state = EXT2_VALID_FS;
    
    // Test unsupported revision
    superblock.rev_level = 99; // Unsupported revision
    let result = Ext2FileSystem::validate_ext2(&superblock);
    assert!(result.is_err());
}

#[test_case]
fn test_ext2_mode_conversion() {
    /// Test ext2 mode conversion
    let file_type = Ext2Node::ext2_mode_to_file_type(EXT2_S_IFREG | 0o644);
    assert!(matches!(file_type, FileType::RegularFile));
    
    // Test directory
    let file_type = Ext2Node::ext2_mode_to_file_type(EXT2_S_IFDIR | 0o755);
    assert!(matches!(file_type, FileType::Directory));
    
    // Test symbolic link
    let file_type = Ext2Node::ext2_mode_to_file_type(EXT2_S_IFLNK | 0o777);
    assert!(matches!(file_type, FileType::SymbolicLink(_)));
    
    // Test character device
    let file_type = Ext2Node::ext2_mode_to_file_type(EXT2_S_IFCHR | 0o666);
    assert!(matches!(file_type, FileType::CharDevice(_)));
    
    // Test block device
    let file_type = Ext2Node::ext2_mode_to_file_type(EXT2_S_IFBLK | 0o666);
    assert!(matches!(file_type, FileType::BlockDevice(_)));
    
    // Test FIFO
    let file_type = Ext2Node::ext2_mode_to_file_type(EXT2_S_IFIFO | 0o644);
    assert!(matches!(file_type, FileType::Pipe));
    
    // Test socket
    let file_type = Ext2Node::ext2_mode_to_file_type(EXT2_S_IFSOCK | 0o666);
    assert!(matches!(file_type, FileType::Socket));
}

#[test_case]
fn test_ext2_node_creation() {
    // Create a test inode
    let inode = Ext2Inode {
        mode: EXT2_S_IFREG | 0o644,
        uid: 1000,
        size: 1024,
        atime: 1000000,
        ctime: 1000000,
        mtime: 1000000,
        dtime: 0,
        gid: 1000,
        links_count: 1,
        blocks: 2, // 2 * 512 = 1024 bytes
        flags: 0,
        osd1: 0,
        block: [0; EXT2_N_BLOCKS],
        generation: 0,
        file_acl: 0,
        dir_acl: 0,
        faddr: 0,
        osd2: [0; 12],
    };
    
    let node = Ext2Node::new_from_inode("test.txt".to_string(), 12, &inode);
    
    assert_eq!(node.name(), "test.txt");
    assert_eq!(node.inode_num, 12);
    
    let metadata = node.metadata().unwrap();
    assert_eq!(metadata.file_id, 12);
    assert_eq!(metadata.size, 1024);
    assert_eq!(metadata.link_count, 1);
    
    let file_type = node.file_type().unwrap();
    assert!(matches!(file_type, FileType::RegularFile));
}

#[test_case]
fn test_ext2_node_children() {
    // Create a directory inode
    let dir_inode = Ext2Inode {
        mode: EXT2_S_IFDIR | 0o755,
        uid: 0,
        size: 4096,
        atime: 1000000,
        ctime: 1000000,
        mtime: 1000000,
        dtime: 0,
        gid: 0,
        links_count: 2,
        blocks: 8, // 8 * 512 = 4096 bytes
        flags: 0,
        osd1: 0,
        block: [0; EXT2_N_BLOCKS],
        generation: 0,
        file_acl: 0,
        dir_acl: 0,
        faddr: 0,
        osd2: [0; 12],
    };
    
    let dir_node = Arc::new(Ext2Node::new_from_inode("test_dir".to_string(), 2, &dir_inode));
    
    // Create a file inode
    let file_inode = Ext2Inode {
        mode: EXT2_S_IFREG | 0o644,
        uid: 1000,
        size: 100,
        atime: 1000000,
        ctime: 1000000,
        mtime: 1000000,
        dtime: 0,
        gid: 1000,
        links_count: 1,
        blocks: 1,
        flags: 0,
        osd1: 0,
        block: [0; EXT2_N_BLOCKS],
        generation: 0,
        file_acl: 0,
        dir_acl: 0,
        faddr: 0,
        osd2: [0; 12],
    };
    
    let file_node = Arc::new(Ext2Node::new_from_inode("test.txt".to_string(), 12, &file_inode));
    
    // Add file to directory
    let result = dir_node.add_child("test.txt".to_string(), file_node.clone());
    assert!(result.is_ok());
    
    // Check that child was added
    let child = dir_node.get_child("test.txt");
    assert!(child.is_some());
    
    let child_node = child.unwrap();
    assert_eq!(child_node.name(), "test.txt");
    
    // Test adding child to non-directory should fail
    let result = file_node.add_child("invalid".to_string(), dir_node.clone());
    assert!(result.is_err());
}

#[test_case]
fn test_ext2_file_object_creation() {
    // Create a file inode
    let file_inode = Ext2Inode {
        mode: EXT2_S_IFREG | 0o644,
        uid: 1000,
        size: 100,
        atime: 1000000,
        ctime: 1000000,
        mtime: 1000000,
        dtime: 0,
        gid: 1000,
        links_count: 1,
        blocks: 1,
        flags: 0,
        osd1: 0,
        block: [0; EXT2_N_BLOCKS],
        generation: 0,
        file_acl: 0,
        dir_acl: 0,
        faddr: 0,
        osd2: [0; 12],
    };
    
    let node = Arc::new(Ext2Node::new_from_inode("test.txt".to_string(), 12, &file_inode));
    let file_obj = Ext2FileObject::new(node.clone());
    
    assert_eq!(*file_obj.position.read(), 0);
    assert!(file_obj.cached_content.read().is_none());
    assert!(!*file_obj.is_dirty.read());
}

#[test_case]
fn test_ext2_directory_object_creation() {
    // Create a directory inode
    let dir_inode = Ext2Inode {
        mode: EXT2_S_IFDIR | 0o755,
        uid: 0,
        size: 4096,
        atime: 1000000,
        ctime: 1000000,
        mtime: 1000000,
        dtime: 0,
        gid: 0,
        links_count: 2,
        blocks: 8,
        flags: 0,
        osd1: 0,
        block: [0; EXT2_N_BLOCKS],
        generation: 0,
        file_acl: 0,
        dir_acl: 0,
        faddr: 0,
        osd2: [0; 12],
    };
    
    let node = Arc::new(Ext2Node::new_from_inode("test_dir".to_string(), 2, &dir_inode));
    let dir_obj = Ext2DirectoryObject::new(node.clone());
    
    assert_eq!(*dir_obj.position.read(), 0);
}

#[test_case]
fn test_ext2_constants() {
    // Test magic number
    assert_eq!(EXT2_SUPER_MAGIC, 0xEF53);
    
    // Test filesystem states
    assert_eq!(EXT2_VALID_FS, 1);
    assert_eq!(EXT2_ERROR_FS, 2);
    
    // Test revision levels
    assert_eq!(EXT2_GOOD_OLD_REV, 0);
    assert_eq!(EXT2_DYNAMIC_REV, 1);
    
    // Test special inode numbers
    assert_eq!(EXT2_ROOT_INO, 2);
    assert_eq!(EXT2_FIRST_INO, 11);
    
    // Test file types
    assert_eq!(EXT2_FT_REG_FILE, 1);
    assert_eq!(EXT2_FT_DIR, 2);
    assert_eq!(EXT2_FT_SYMLINK, 7);
}