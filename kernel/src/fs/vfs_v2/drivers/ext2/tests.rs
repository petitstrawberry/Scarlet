//! Tests for EXT2 filesystem implementation

use super::*;
use crate::device::block::mockblk::MockBlockDevice;
use crate::fs::get_fs_driver_manager;
use alloc::{boxed::Box, format, vec::Vec};

#[test_case]
fn test_ext2_driver_registration() {
    let fs_driver_manager = get_fs_driver_manager();
    let driver_type = fs_driver_manager.get_driver_type("ext2");
    assert_eq!(driver_type, Some(FileSystemType::Block));
}

#[test_case] 
fn test_ext2_superblock_validation() {
    let mut superblock = Ext2Superblock {
        inodes_count: 1000,
        blocks_count: 8192,
        r_blocks_count: 410,
        free_blocks_count: 7000,
        free_inodes_count: 989,
        first_data_block: 1,
        log_block_size: 0, // 1024 bytes
        log_frag_size: 0,
        blocks_per_group: 8192,
        frags_per_group: 8192,
        inodes_per_group: 1000,
        mtime: 0,
        wtime: 0,
        mnt_count: 1,
        max_mnt_count: 20,
        magic: EXT2_MAGIC,
        state: 1,
        errors: 1,
        minor_rev_level: 0,
        lastcheck: 0,
        checkinterval: 0,
        creator_os: 0,
        rev_level: EXT2_GOOD_OLD_REV,
        def_resuid: 0,
        def_resgid: 0,
    };
    
    // Test valid superblock
    assert!(superblock.is_valid());
    assert_eq!(superblock.block_size(), 1024);
    assert_eq!(superblock.group_count(), 1);
    
    // Test invalid magic
    superblock.magic = 0x1234;
    assert!(!superblock.is_valid());
    
    // Test validation
    let result = Ext2FileSystem::validate_ext2(&superblock);
    assert!(result.is_err());
}

#[test_case]
fn test_ext2_inode_operations() {
    let mut inode = Ext2Inode {
        mode: EXT2_S_IFREG | 0o644,
        uid: 1000,
        size: 2048,
        atime: 0,
        ctime: 0,
        mtime: 0,
        dtime: 0,
        gid: 1000,
        links_count: 1,
        blocks: 4,
        flags: 0,
        osd1: 0,
        block: [0; 15],
        generation: 0,
        file_acl: 0,
        dir_acl: 0,
        faddr: 0,
        osd2: [0; 12],
    };
    
    // Test file type detection
    assert!(inode.is_regular_file());
    assert!(!inode.is_directory());
    assert!(!inode.is_symbolic_link());
    
    // Test directory inode
    inode.mode = EXT2_S_IFDIR | 0o755;
    assert!(!inode.is_regular_file());
    assert!(inode.is_directory());
    assert!(!inode.is_symbolic_link());
    
    // Test symbolic link inode
    inode.mode = EXT2_S_IFLNK | 0o644;
    assert!(!inode.is_regular_file());
    assert!(!inode.is_directory());
    assert!(inode.is_symbolic_link());
    
    // Test block pointers
    inode.block[0] = 100;
    inode.block[12] = 200;
    inode.block[13] = 300;
    inode.block[14] = 400;
    
    assert_eq!(inode.direct_block(0), Some(100));
    assert_eq!(inode.direct_block(12), None);
    let indirect = inode.indirect_block();
    let double_indirect = inode.double_indirect_block();
    let triple_indirect = inode.triple_indirect_block();
    assert_eq!(indirect, 200);
    assert_eq!(double_indirect, 300);
    assert_eq!(triple_indirect, 400);
}

#[test_case]
fn test_ext2_create_from_mock_device() {
    // Create a mock block device
    let mock_device = MockBlockDevice::new("mock_ext2", 1024, 8192);
    
    // Create EXT2 filesystem
    let result = Ext2FileSystem::new(Arc::new(mock_device));
    assert!(result.is_ok());
    
    let ext2_fs = result.unwrap();
    assert_eq!(ext2_fs.name(), "ext2");
    assert!(ext2_fs.is_read_only());
    assert_eq!(ext2_fs.block_size, 1024);
}

#[test_case]
fn test_ext2_filesystem_type() {
    let driver = driver::Ext2Driver;
    assert_eq!(driver.name(), "ext2");
    assert_eq!(driver.filesystem_type(), FileSystemType::Block);
    
    // EXT2 should not support creation without block device
    assert!(driver.create().is_err());
    assert!(driver.create_from_option_string("").is_err());
}

#[test_case]
fn test_ext2_node_creation() {
    // Test file node creation
    let file_node = Ext2Node::new_file("test.txt".to_string(), 1, 10, 1024);
    assert_eq!(file_node.name(), "test.txt");
    assert_eq!(file_node.file_type().unwrap(), FileType::RegularFile);
    assert_eq!(file_node.metadata().unwrap().size, 1024);
    assert_eq!(file_node.inode_number, 10);
    
    // Test directory node creation
    let dir_node = Ext2Node::new_directory("testdir".to_string(), 2, 5);
    assert_eq!(dir_node.name(), "testdir");
    assert_eq!(dir_node.file_type().unwrap(), FileType::Directory);
    assert_eq!(dir_node.metadata().unwrap().size, 0);
    assert_eq!(dir_node.inode_number, 5);
    
    // Test children loading state
    assert!(!dir_node.are_children_loaded());
    dir_node.mark_children_loaded();
    assert!(dir_node.are_children_loaded());
}

#[test_case]
fn test_ext2_mockdevice_filesystem_operations() {
    // Create filesystem with mock device
    let mock_device = MockBlockDevice::new("mock_ext2", 1024, 8192);
    let ext2_fs = Ext2FileSystem::new(Arc::new(mock_device)).expect("Failed to create EXT2 filesystem");
    
    // Test root node
    let root = ext2_fs.root_node();
    assert_eq!(root.file_type().unwrap(), FileType::Directory);
    
    // Test readdir on root
    let entries = ext2_fs.readdir(&root).expect("Failed to read root directory");
    assert!(entries.len() >= 4); // ., .., test.txt, subdir
    
    let entry_names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert!(entry_names.contains(&"."));
    assert!(entry_names.contains(&".."));
    assert!(entry_names.contains(&"test.txt"));
    assert!(entry_names.contains(&"subdir"));
    
    // Test lookup operations
    let test_file = ext2_fs.lookup(&root, &"test.txt".to_string()).expect("Failed to lookup test.txt");
    assert_eq!(test_file.file_type().unwrap(), FileType::RegularFile);
    
    let subdir = ext2_fs.lookup(&root, &"subdir".to_string()).expect("Failed to lookup subdir");
    assert_eq!(subdir.file_type().unwrap(), FileType::Directory);
    
    // Test lookup of non-existent file
    let not_found = ext2_fs.lookup(&root, &"nonexistent.txt".to_string());
    assert!(not_found.is_err());
}

#[test_case]
fn test_ext2_file_operations() {
    // Create filesystem with mock device
    let mock_device = MockBlockDevice::new("mock_ext2", 1024, 8192);
    let ext2_fs = Ext2FileSystem::new(Arc::new(mock_device)).expect("Failed to create EXT2 filesystem");
    
    // Get root and lookup a file
    let root = ext2_fs.root_node();
    let test_file = ext2_fs.lookup(&root, &"test.txt".to_string()).expect("Failed to lookup test.txt");
    
    // Test opening the file
    let file_obj = ext2_fs.open(&test_file, 0).expect("Failed to open file");
    
    // Test file operations
    assert_eq!(file_obj.metadata().unwrap().file_type, FileType::RegularFile);
    
    let sync_result = file_obj.sync();
    assert!(sync_result.is_ok());
    
    // Test truncate (should fail for read-only implementation)
    let truncate_result = file_obj.truncate(500);
    assert!(truncate_result.is_err());
}

#[test_case]
fn test_ext2_directory_entry_parsing() {
    // Test directory entry structure
    let entry = Ext2DirectoryEntry {
        inode: 10,
        rec_len: 20,
        name_len: 8,
        file_type: EXT2_FT_REG_FILE,
    };
    
    // Copy fields to avoid packed struct reference issues
    let entry_inode = entry.inode;
    let entry_rec_len = entry.rec_len;
    let entry_name_len = entry.name_len;
    let entry_file_type = entry.file_type;
    
    assert_eq!(entry_inode, 10);
    assert_eq!(entry_rec_len, 20);
    assert_eq!(entry_name_len, 8);
    assert_eq!(entry_file_type, EXT2_FT_REG_FILE);
    assert_eq!(entry.total_size(), 16); // 8 bytes fixed + 8 bytes name
    assert_eq!(entry.record_length(), 20);
}

#[test_case]
fn test_ext2_inode_cache() {
    // Create filesystem with mock device
    let mock_device = MockBlockDevice::new("mock_ext2", 1024, 8192);
    let ext2_fs = Ext2FileSystem::new(Arc::new(mock_device)).expect("Failed to create EXT2 filesystem");
    
    // Read root inode (should be cached)
    let inode1 = ext2_fs.read_inode(2).expect("Failed to read root inode");
    assert!(inode1.is_directory());
    
    // Read same inode again (should come from cache)
    let inode2 = ext2_fs.read_inode(2).expect("Failed to read root inode from cache");
    let mode1 = inode1.mode;
    let mode2 = inode2.mode;
    let size1 = inode1.size;
    let size2 = inode2.size;
    assert_eq!(mode1, mode2);
    assert_eq!(size1, size2);
    
    // Try to read invalid inode
    let invalid_inode = ext2_fs.read_inode(u32::MAX);
    assert!(invalid_inode.is_err());
}

#[test_case]
fn test_ext2_unsupported_operations() {
    // Create filesystem with mock device
    let mock_device = MockBlockDevice::new("mock_ext2", 1024, 8192);
    let ext2_fs = Ext2FileSystem::new(Arc::new(mock_device)).expect("Failed to create EXT2 filesystem");
    
    let root = ext2_fs.root_node();
    
    // Test unsupported create operation
    let create_result = ext2_fs.create(&root, &"newfile.txt".to_string(), FileType::RegularFile, 0o644);
    assert!(create_result.is_err());
    
    // Test unsupported remove operation
    let remove_result = ext2_fs.remove(&root, &"test.txt".to_string());
    assert!(remove_result.is_err());
}

#[test_case]
fn test_ext2_node_metadata_update() {
    let node = Ext2Node::new_file("test.txt".to_string(), 1, 10, 1024);
    
    // Create a test inode
    let inode = Ext2Inode {
        mode: EXT2_S_IFREG | 0o644,
        uid: 1000,
        size: 2048,
        atime: 12345,
        ctime: 12346,
        mtime: 12347,
        dtime: 0,
        gid: 1000,
        links_count: 1,
        blocks: 4,
        flags: 0,
        osd1: 0,
        block: [0; 15],
        generation: 0,
        file_acl: 0,
        dir_acl: 0,
        faddr: 0,
        osd2: [0; 12],
    };
    
    // Set inode data (should update metadata)
    node.set_inode_data(inode);
    
    let metadata = node.metadata().unwrap();
    assert_eq!(metadata.size, 2048);
    assert_eq!(metadata.access_time, 12345);
    assert_eq!(metadata.modify_time, 12347);
    assert_eq!(metadata.change_time, 12346);
    assert_eq!(metadata.owner_id, 1000);
    assert_eq!(metadata.group_id, 1000);
    assert_eq!(metadata.file_type, FileType::RegularFile);
    
    // Test getting cached inode
    let cached_inode = node.get_inode_data();
    assert!(cached_inode.is_some());
    let cached = cached_inode.unwrap();
    let cached_size = cached.size;
    let cached_uid = cached.uid;
    assert_eq!(cached_size, 2048);
    assert_eq!(cached_uid, 1000);
}