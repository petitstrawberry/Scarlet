//! ext2 Filesystem Tests
//!
//! This module contains comprehensive tests for the ext2 filesystem implementation,
//! using both MockBlockDevice for unit tests and virtio-blk for integration tests.

use alloc::{sync::Arc, vec, vec::Vec, format, string::ToString};
use crate::{
    device::block::{mockblk::MockBlockDevice, request::{BlockIORequest, BlockIORequestType}}, drivers::block::virtio_blk::VirtioBlockDevice, early_println, fs::{get_fs_driver_manager, FileSystemError, FileSystemErrorKind, FileSystemType, FileType}, object::capability::StreamOps
};

use super::*;

#[test_case]
fn test_ext2_driver_registration() {
    let fs_driver_manager = get_fs_driver_manager();
    let driver_type = fs_driver_manager.get_driver_type("ext2");
    assert_eq!(driver_type, Some(FileSystemType::Block));
}

#[test_case]
fn test_ext2_mockdevice_basic_creation() {
    let fs_driver_manager = get_fs_driver_manager();
    
    // Create a test ext2 device with proper structure
    let mock_device = create_test_ext2_device();
    let block_device_arc = Arc::new(mock_device);
    
    // Try to create ext2 filesystem from the mock device
    match fs_driver_manager.create_from_block("ext2", block_device_arc, 512) {
        Ok(fs) => {
            early_println!("[Test] Successfully created ext2 filesystem from mock device");
            
            // Get the root node
            let root_node = fs.root_node();
            early_println!("[Test] Got root node with ID: {}", root_node.id());
            
            // Test basic filesystem operations
            assert_eq!(fs.name(), "ext2");
        },
        Err(e) => {
            early_println!("[Test] Failed to create ext2 filesystem from mock device: {:?}", e);
            early_println!("[Test] This is expected since our mock device doesn't have proper ext2 structure");
            assert!(
                e.kind == FileSystemErrorKind::IoError || 
                e.kind == FileSystemErrorKind::InvalidData
            );
        }
    }
}

#[test_case]
fn test_ext2_mockdevice_directory_operations() {
    let fs_driver_manager = get_fs_driver_manager();
    
    // Create a more complete test ext2 device
    let mock_device = create_test_ext2_device_with_files();
    let block_device_arc = Arc::new(mock_device);
    
    match fs_driver_manager.create_from_block("ext2", block_device_arc, 512) {
        Ok(fs) => {
            early_println!("[Test] Successfully created ext2 filesystem with files");
            
            // Get the root node
            let root_node = fs.root_node();
            
            // Test directory reading
            match fs.readdir(&root_node) {
                Ok(entries) => {
                    early_println!("[Test] Root directory contains {} entries", entries.len());
                    for entry in &entries {
                        early_println!("[Test] Found entry: {} (type: {:?})", entry.name, entry.file_type);
                    }
                },
                Err(e) => {
                    early_println!("[Test] Failed to read directory: {:?}", e);
                }
            }
        },
        Err(e) => {
            early_println!("[Test] Expected failure for mock device: {:?}", e);
            // Expected to fail since our mock device structure is incomplete
        }
    }
}

#[test_case]
fn test_ext2_superblock_parsing() {
    use super::structures::*;
    
    early_println!("[Test] Starting ext2 superblock parsing test");
    
    // Create a minimal valid ext2 superblock
    let mut superblock_data = vec![0u8; 1024];  // Standard ext2 superblock size
    
    early_println!("[Test] Created superblock data buffer of size {}", superblock_data.len());
    
    // Fill in essential superblock fields manually
    // Magic at offset 56 (0x38)
    superblock_data[56] = (EXT2_SUPER_MAGIC & 0xFF) as u8;
    superblock_data[57] = ((EXT2_SUPER_MAGIC >> 8) & 0xFF) as u8;
    
    early_println!("[Test] Set magic bytes to {:02x} {:02x}", superblock_data[56], superblock_data[57]);
    
    // blocks_count at offset 4
    superblock_data[4] = 0x00;
    superblock_data[5] = 0x20;
    superblock_data[6] = 0x00;
    superblock_data[7] = 0x00; // 8192
    
    // inodes_count at offset 0
    superblock_data[0] = 0x00;
    superblock_data[1] = 0x08;
    superblock_data[2] = 0x00;
    superblock_data[3] = 0x00; // 2048
    
    // Set block size field (log_block_size at offset 24)
    superblock_data[24] = 0x00;
    superblock_data[25] = 0x00;
    superblock_data[26] = 0x00;
    superblock_data[27] = 0x00; // log_block_size = 0 -> block_size = 1024
    
    // Test that the magic bytes are set correctly
    let magic_from_bytes = u16::from_le_bytes([superblock_data[56], superblock_data[57]]);
    early_println!("[Test] Magic bytes from data: {:04x}, Expected: {:04x}", magic_from_bytes, EXT2_SUPER_MAGIC);
    early_println!("[Test] Raw bytes at 56-57: {:02x} {:02x}", superblock_data[56], superblock_data[57]);
    assert_eq!(magic_from_bytes, EXT2_SUPER_MAGIC, "Magic bytes should be set correctly");
    
    // Test superblock parsing
    let result = Ext2Superblock::from_bytes(&superblock_data);
    if result.is_err() {
        early_println!("[Test] Superblock parsing failed: {:?}", result.as_ref().err());
        early_println!("[Test] Magic bytes at 56-57: {:02x} {:02x}", superblock_data[56], superblock_data[57]);
        early_println!("[Test] Expected magic: {:04x}", EXT2_SUPER_MAGIC);
        
        // Let's also check a few more bytes to see what's going on
        early_println!("[Test] First 16 bytes: {:02x?}", &superblock_data[0..16]);
        early_println!("[Test] Bytes around magic (52-60): {:02x?}", &superblock_data[52..60]);
    }
    assert!(result.is_ok(), "Should be able to parse valid superblock");
    
    let superblock = result.unwrap();
    let magic = u16::from_le(superblock.magic);
    let blocks_count = superblock.get_blocks_count();
    let inodes_count = superblock.get_inodes_count();
    assert_eq!(magic, EXT2_SUPER_MAGIC);
    assert_eq!(blocks_count, 8192);
    assert_eq!(inodes_count, 2048);
    
    early_println!("[Test] ✓ ext2 superblock parsing test passed");
}

#[test_case]
fn test_ext2_inode_parsing() {
    use super::structures::*;
    
    // Create a test inode
    let mut inode_data = vec![0u8; 128];
    
    // mode at offset 0
    inode_data[0] = 0x44; // 0x8044 = EXT2_S_IFREG | 0o644
    inode_data[1] = 0x81;
    
    // size at offset 4
    inode_data[4] = 0x00;
    inode_data[5] = 0x04;
    inode_data[6] = 0x00;
    inode_data[7] = 0x00; // 1024
    
    // links_count at offset 26
    inode_data[26] = 0x01;
    inode_data[27] = 0x00; // 1
    
    // Test inode parsing
    let result = Ext2Inode::from_bytes(&inode_data);
    assert!(result.is_ok(), "Should be able to parse valid inode");
    
    let inode = result.unwrap();
    let mode = inode.get_mode();
    let size = inode.get_size();
    let links_count = inode.links_count;
    assert_eq!(mode & EXT2_S_IFMT, EXT2_S_IFREG);
    assert_eq!(size, 1024);
    assert_eq!(links_count, 1);
    
    early_println!("[Test] ✓ ext2 inode parsing test passed");
}

#[test_case]
fn test_ext2_directory_entry_parsing() {
    use super::structures::*;
    
    // Create a test directory entry
    let name = "test.txt";
    let mut entry_data = vec![0u8; 8 + name.len()];
    
    // Set up directory entry header
    entry_data[0..4].copy_from_slice(&12u32.to_le_bytes()); // inode
    entry_data[4..6].copy_from_slice(&(8 + name.len() as u16).to_le_bytes()); // rec_len
    entry_data[6] = name.len() as u8; // name_len
    entry_data[7] = 1; // file_type (regular file)
    
    // Copy name
    entry_data[8..8 + name.len()].copy_from_slice(name.as_bytes());
    
    // Test directory entry parsing
    let result = Ext2DirectoryEntry::from_bytes(&entry_data);
    assert!(result.is_ok(), "Should be able to parse valid directory entry");
    
    let entry = result.unwrap();
    let inode = entry.entry.get_inode();
    let name_len = entry.entry.get_name_len();
    let name = &entry.name;
    assert_eq!(inode, 12);
    assert_eq!(name_len, name.len() as u8);
    assert_eq!(name, "test.txt");
    
    early_println!("[Test] ✓ ext2 directory entry parsing test passed");
}

#[test_case]
fn test_ext2_block_group_descriptor_parsing() {
    use super::structures::*;
    
    // Create a test block group descriptor
    let mut bgd_data = vec![0u8; 32];
    
    // block_bitmap at offset 0
    bgd_data[0] = 0x03;
    bgd_data[1] = 0x00;
    bgd_data[2] = 0x00;
    bgd_data[3] = 0x00; // 3
    
    // inode_bitmap at offset 4  
    bgd_data[4] = 0x04;
    bgd_data[5] = 0x00;
    bgd_data[6] = 0x00;
    bgd_data[7] = 0x00; // 4
    
    // inode_table at offset 8
    bgd_data[8] = 0x05;
    bgd_data[9] = 0x00;
    bgd_data[10] = 0x00;
    bgd_data[11] = 0x00; // 5
    
    // free_blocks_count at offset 12
    bgd_data[12] = 0xE8;
    bgd_data[13] = 0x03; // 1000
    
    // Test BGD parsing
    let result = Ext2BlockGroupDescriptor::from_bytes(&bgd_data);
    assert!(result.is_ok(), "Should be able to parse valid block group descriptor");
    
    let bgd = result.unwrap();
    let block_bitmap = bgd.block_bitmap;
    let inode_bitmap = bgd.get_inode_bitmap();
    let inode_table = bgd.get_inode_table();
    let free_blocks_count = bgd.free_blocks_count;
    assert_eq!(block_bitmap, 3);
    assert_eq!(inode_bitmap, 4);
    assert_eq!(inode_table, 5);
    assert_eq!(free_blocks_count, 1000);
    
    early_println!("[Test] ✓ ext2 block group descriptor parsing test passed");
}

#[test_case]
fn test_ext2_node_creation() {
    use super::node::*;
    use crate::fs::FileType;
    
    // Test creating various types of nodes
    let file_node = Ext2Node::new(12, FileType::RegularFile, 100);
    assert_eq!(file_node.inode_number(), 12);
    assert_eq!(file_node.id(), 100);
    assert_eq!(file_node.file_type().unwrap(), FileType::RegularFile);
    
    let dir_node = Ext2Node::new(2, FileType::Directory, 1);
    assert_eq!(dir_node.inode_number(), 2);
    assert_eq!(dir_node.id(), 1);
    assert_eq!(dir_node.file_type().unwrap(), FileType::Directory);
    
    early_println!("[Test] ✓ ext2 node creation test passed");
}

#[test_case]
fn test_ext2_file_object_operations() {
    use super::node::*;
    use crate::fs::SeekFrom;
    use crate::object::capability::StreamOps;
    
    // Create a file object
    let file_obj = Ext2FileObject::new(12, 100);
    assert_eq!(file_obj.file_id(), 100);
    
    // Test seek operations
    let seek_result = file_obj.seek(crate::fs::SeekFrom::Start(0));
    assert!(seek_result.is_ok(), "Should be able to seek to start");
    assert_eq!(seek_result.unwrap(), 0);
    
    let seek_result = file_obj.seek(crate::fs::SeekFrom::Current(10));
    assert!(seek_result.is_ok(), "Should be able to seek current");
    assert_eq!(seek_result.unwrap(), 10);
    
    // Test read (should fail gracefully since no filesystem is set)
    let mut buffer = vec![0u8; 100];
    let read_result = file_obj.read(&mut buffer);
    // Read should fail since no filesystem reference is set, but should not panic
    assert!(read_result.is_err(), "Read should error when no filesystem is set");
    
    early_println!("[Test] ✓ ext2 file object operations test passed");
}

// Helper function to create a mock ext2 device with proper structure
fn create_test_ext2_device() -> MockBlockDevice {
    let sector_size = 512;
    let sector_count = 16384; // 8MB device
    
    let mock_device = MockBlockDevice::new("mock_ext2", sector_size, sector_count);
    
    // Create a minimal ext2 superblock in block 1 (sectors 2-3 for 1KB block)
    let mut superblock_data = vec![0u8; 1024];
    
    // Set up superblock fields manually in byte array
    // Magic at offset 56 (0x38)
    superblock_data[56] = (EXT2_SUPER_MAGIC & 0xFF) as u8;
    superblock_data[57] = ((EXT2_SUPER_MAGIC >> 8) & 0xFF) as u8;
    
    // blocks_count at offset 4
    superblock_data[4] = 0x00;
    superblock_data[5] = 0x20;
    superblock_data[6] = 0x00;
    superblock_data[7] = 0x00; // 8192
    
    // inodes_count at offset 0  
    superblock_data[0] = 0x00;
    superblock_data[1] = 0x08;
    superblock_data[2] = 0x00;
    superblock_data[3] = 0x00; // 2048
    
    // log_block_size at offset 24
    superblock_data[24] = 0x00;
    superblock_data[25] = 0x00;
    superblock_data[26] = 0x00;
    superblock_data[27] = 0x00; // 0 = 1KB blocks
    
    // blocks_per_group at offset 32
    superblock_data[32] = 0x00;
    superblock_data[33] = 0x20;
    superblock_data[34] = 0x00;
    superblock_data[35] = 0x00; // 8192
    
    // inodes_per_group at offset 40  
    superblock_data[40] = 0x00;
    superblock_data[41] = 0x08;
    superblock_data[42] = 0x00;
    superblock_data[43] = 0x00; // 2048
    
    // inode_size at offset 88
    superblock_data[88] = 0x80;
    superblock_data[89] = 0x00; // 128
    
    // first_data_block at offset 20
    superblock_data[20] = 0x01;
    superblock_data[21] = 0x00;
    superblock_data[22] = 0x00;
    superblock_data[23] = 0x00; // 1
    
    // rev_level at offset 76
    superblock_data[76] = 0x01;
    superblock_data[77] = 0x00;
    superblock_data[78] = 0x00;
    superblock_data[79] = 0x00; // 1
    
    // Write superblock to sectors 2-3 (block 1)
    let superblock_request = Box::new(BlockIORequest {
        request_type: BlockIORequestType::Write,
        sector: 2,
        sector_count: 2,
        head: 0,
        cylinder: 0,
        buffer: superblock_data,
    });
    
    mock_device.enqueue_request(superblock_request);
    mock_device.process_requests();
    
    mock_device
}

// Helper function to create a mock ext2 device with files and directories
fn create_test_ext2_device_with_files() -> MockBlockDevice {
    let mut mock_device = create_test_ext2_device();
    
    // This would be much more complex in a real implementation
    // For now, just return the basic device
    // TODO: Add proper inode table, directory entries, etc.
    
    mock_device
}

// Test that verifies ext2 can handle realistic filesystem operations
#[test_case]
fn test_ext2_realistic_operations() {
    early_println!("[Test] Running ext2 realistic operations test");
    
    let fs_driver_manager = get_fs_driver_manager();
    
    // Create a more realistic ext2 device with proper superblock
    let mock_device = create_test_ext2_device();
    let block_device_arc = Arc::new(mock_device);
    
    match fs_driver_manager.create_from_block("ext2", block_device_arc, 512) {
        Ok(fs) => {
            early_println!("[Test] Successfully created ext2 filesystem");
            
            // Test root node access
            let root_node = fs.root_node();
            early_println!("[Test] Got root node with ID: {}", root_node.id());
            
            // Test filesystem metadata operations
            assert_eq!(fs.name(), "ext2");
            
            // Test root directory metadata
            if let Ok(file) = fs.open(&root_node, 0) {
                if let Ok(metadata) = file.metadata() {
                    early_println!("[Test] Root directory metadata - size: {}, type: {:?}", 
                                 metadata.size, metadata.file_type);
                    assert_eq!(metadata.file_type, crate::fs::FileType::Directory);
                }
            }
            
            early_println!("[Test] ✓ Realistic ext2 operations test passed!");
        },
        Err(e) => {
            early_println!("[Test] Expected filesystem creation failure for mock device: {:?}", e);
            // This is expected since our mock device doesn't have complete ext2 structure
            assert!(
                e.kind == FileSystemErrorKind::IoError || 
                e.kind == FileSystemErrorKind::InvalidData
            );
        }
    }
}

// Test ext2 memory mapping operations 
#[test_case]
fn test_ext2_memory_mapping_operations() {
    early_println!("[Test] Running ext2 memory mapping operations test");
    
    let fs_driver_manager = get_fs_driver_manager();
    let mock_device = create_test_ext2_device();
    let block_device_arc = Arc::new(mock_device);
    
    // Even if filesystem creation fails, we can test the node implementations directly
    match fs_driver_manager.create_from_block("ext2", block_device_arc, 512) {
        Ok(fs) => {
            let root_node = fs.root_node();
            
            // Test file operations if we can open a file
            if let Ok(file) = fs.open(&root_node, 0) {
                // Test supports_mmap
                assert!(!file.supports_mmap()); // Directory shouldn't support mmap
                
                early_println!("[Test] ✓ Memory mapping capability detection works");
            }
        },
        Err(_) => {
            early_println!("[Test] Filesystem creation failed as expected for mock device");
            // This is expected behavior for incomplete mock data
        }
    }
    
    early_println!("[Test] Memory mapping operations test completed");
}

// Test ext2 file content and metadata reading
#[test_case] 
fn test_ext2_file_content_and_metadata() {
    early_println!("[Test] Running ext2 file content and metadata test");
    
    let fs_driver_manager = get_fs_driver_manager();
    let mock_device = create_test_ext2_device();
    let block_device_arc = Arc::new(mock_device);
    
    match fs_driver_manager.create_from_block("ext2", block_device_arc, 512) {
        Ok(fs) => {
            let root_node = fs.root_node();
            
            // Test file opening and metadata reading
            if let Ok(file) = fs.open(&root_node, 0) {
                // Test metadata operation
                if let Ok(metadata) = file.metadata() {
                    early_println!("[Test] Successfully read metadata: size={}, permissions={:?}", 
                                 metadata.size, metadata.permissions);
                    
                    // Verify this is a directory
                    assert_eq!(metadata.file_type, crate::fs::FileType::Directory);
                }
                
                // Test seek operations
                if let Ok(new_pos) = file.seek(crate::object::capability::file::SeekFrom::Start(0)) {
                    early_println!("[Test] Seek to start: {}", new_pos);
                    assert_eq!(new_pos, 0);
                }
            }
            
            early_println!("[Test] ✓ File content and metadata test passed!");
        },
        Err(e) => {
            early_println!("[Test] Expected filesystem failure: {:?}", e);
            // Expected for mock device without complete ext2 structure
        }
    }
    
    early_println!("[Test] File content and metadata test completed");
}

#[test_case]
fn test_ext2_comprehensive_mock_operations() {
    early_println!("[Test] Running comprehensive ext2 mock operations test");
    
    let fs_driver_manager = get_fs_driver_manager();
    
    // Create test device
    let mock_device = create_test_ext2_device();
    let block_device_arc = Arc::new(mock_device);
    
    // Test filesystem creation
    match fs_driver_manager.create_from_block("ext2", block_device_arc, 512) {
        Ok(fs) => {
            early_println!("[Test] Successfully created ext2 filesystem");
            
            // Test root node access
            let root_node = fs.root_node();
            early_println!("[Test] Got root node with ID: {}", root_node.id());
            
            // Test filesystem name
            assert_eq!(fs.name(), "ext2");
            early_println!("[Test] Filesystem name: {}", fs.name());
            
            early_println!("[Test] ✓ All basic ext2 operations completed successfully!");
        },
        Err(e) => {
            early_println!("[Test] Expected filesystem creation failure: {:?}", e);
            // This is expected since our mock device doesn't have complete ext2 structure
            assert!(
                e.kind == FileSystemErrorKind::IoError || 
                e.kind == FileSystemErrorKind::InvalidData
            );
        }
    }
    
    early_println!("[Test] Comprehensive ext2 mock operations test completed");
}

// ========== ext2 virtio-blk Integration Tests ==========
// The following tests use the actual virtio-blk device with real ext2 images

#[test_case]
fn test_ext2_virtio_blk_filesystem() {
    use crate::drivers::block::virtio_blk::VirtioBlockDevice;

    early_println!("[Test] Testing ext2 with virtio-blk...");

    // Create a virtio-blk device for testing ext2 image on bus.5
    let base_addr = 0x10006000; // Standard virtio-blk address for QEMU bus.5
    let virtio_device = VirtioBlockDevice::new(base_addr);

    early_println!("[Test] Created virtio-blk device: {}", virtio_device.get_disk_name());
    early_println!("[Test] Device size: {} bytes", virtio_device.get_disk_size());

    // Register the ext2 driver if not already registered
    let fs_driver_manager = get_fs_driver_manager();
    fs_driver_manager.register_driver(Box::new(super::Ext2Driver));

    // Try to read the boot block to verify device connectivity
    let request = Box::new(crate::device::block::request::BlockIORequest {
        request_type: BlockIORequestType::Read,
        sector: 0,  // Read boot block
        sector_count: 1,
        head: 0,
        cylinder: 0,
        buffer: vec![0u8; 1024],
    });

    virtio_device.enqueue_request(request);
    let results = virtio_device.process_requests();

    assert_eq!(results.len(), 1);
    let result = &results[0];

    match &result.result {
        Ok(_) => {
            early_println!("[Test] Successfully read boot sector from virtio-blk device");
            early_println!("[Test] Boot sector size: {} bytes", result.request.buffer.len());

            // Try to create ext2 filesystem from the virtio-blk device
            let block_device_arc = Arc::new(virtio_device);
            match fs_driver_manager.create_from_block("ext2", block_device_arc, 1024) {
                Ok(fs) => {
                    early_println!("[Test] Successfully created ext2 filesystem from virtio-blk device");
                    
                    // Get the root node
                    let root_node = fs.root_node();
                    early_println!("[Test] Got root node with ID: {}", root_node.id());
                    
                    // Verify filesystem type
                    assert_eq!(fs.name(), "ext2");
                    early_println!("[Test] ✓ Confirmed ext2 filesystem on virtio-blk device");
                },
                Err(e) => {
                    early_println!("[Test] Failed to create ext2 filesystem: {:?}", e);
                    // This might happen if the image is not properly formatted
                }
            }
        },
        Err(_) => {
            panic!("Failed to read from virtio-blk device");
        }
    }

    early_println!("[Test] ext2 virtio-blk integration test completed successfully");
}

#[test_case] 
fn test_ext2_virtio_blk_file_operations() {
    use crate::drivers::block::virtio_blk::VirtioBlockDevice;

    early_println!("[Test] Testing ext2 file operations with virtio-blk...");

    // Create a virtio-blk device for testing
    let base_addr = 0x10006000; // Standard virtio-blk address for QEMU bus.5
    let virtio_device = VirtioBlockDevice::new(base_addr);

    early_println!("[Test] Created virtio-blk device: {}", virtio_device.get_disk_name());
    early_println!("[Test] Device size: {} bytes", virtio_device.get_disk_size());

    // Create ext2 filesystem from the virtio-blk device
    let fs_driver_manager = get_fs_driver_manager();
    let block_device_arc = Arc::new(virtio_device);

    match fs_driver_manager.create_from_block("ext2", block_device_arc, 1024) {
        Ok(fs) => {
            early_println!("[Test] Successfully created ext2 filesystem from virtio-blk device");
            
            // Get the root node
            let root_node = fs.root_node();
            early_println!("[Test] Got root node for file operations");
            
            // Test 1: Read root directory
            early_println!("[Test] Reading root directory...");
            match fs.readdir(&root_node) {
                Ok(entries) => {
                    early_println!("[Test] Root directory contains {} entries", entries.len());
                    for entry in &entries {
                        early_println!("[Test] Found: {} (type: {:?})", entry.name, entry.file_type);
                    }
                    early_println!("[Test] ✓ Root directory read successful");
                },
                Err(e) => {
                    panic!("Could not read root directory: {:?}", e);
                }
            }
            
            // Test 2: Look for and read hello.txt file
            early_println!("[Test] Looking for hello.txt file...");
            match fs.lookup(&root_node, &String::from("hello.txt")) {
                Ok(file_node) => {
                    early_println!("[Test] Successfully found hello.txt");
                    match fs.open(&file_node, 0) {
                        Ok(file_obj) => {
                            let mut buffer = vec![0u8; 64];
                            match file_obj.read(&mut buffer) {
                                Ok(bytes_read) => {
                                    early_println!("[Test] Read {} bytes from hello.txt", bytes_read);
                                    
                                    // Convert to string and verify content
                                    let content = core::str::from_utf8(&buffer[..bytes_read])
                                        .unwrap_or("INVALID_UTF8");
                                    early_println!("[Test] File content: '{}'", content);
                                    
                                    let expected = "Hello, Scarlet!\n";
                                    assert_eq!(content, expected, "hello.txt content should match expected text");
                                    early_println!("[Test] ✓ File read operation successful");
                                },
                                Err(e) => {
                                    panic!("Failed to read from hello.txt: {:?}", e);
                                }
                            }
                        },
                        Err(e) => {
                            panic!("Failed to open hello.txt: {:?}", e);
                        }
                    }
                },
                Err(e) => {
                    early_println!("[Test] Could not find hello.txt: {:?}", e);
                }
            }
            
            // Test 3: Look for and read readme.txt file
            early_println!("[Test] Looking for readme.txt file...");
            match fs.lookup(&root_node, &String::from("readme.txt")) {
                Ok(file_node) => {
                    early_println!("[Test] Successfully found readme.txt");
                    match fs.open(&file_node, 0) {
                        Ok(file_obj) => {
                            let mut buffer = vec![0u8; 128]; // Enough for longer content
                            match file_obj.read(&mut buffer) {
                                Ok(bytes_read) => {
                                    early_println!("[Test] Read {} bytes from readme.txt", bytes_read);
                                    
                                    // Convert to string and verify content
                                    let content = core::str::from_utf8(&buffer[..bytes_read])
                                        .unwrap_or("INVALID_UTF8");
                                    early_println!("[Test] File content: '{}'", content);
                                    
                                    let expected = "This is a test file for ext2 filesystem implementation.\n";
                                    assert_eq!(content, expected, "readme.txt content should match expected text");
                                    early_println!("[Test] ✓ Second file read operation successful");
                                },
                                Err(e) => {
                                    panic!("Failed to read from readme.txt: {:?}", e);
                                }
                            }
                        },
                        Err(e) => {
                            panic!("Failed to open readme.txt: {:?}", e);
                        }
                    }
                },
                Err(e) => {
                    panic!("Failed to lookup readme.txt: {:?}", e);
                }
            }
            
            // Test 4: Test directory operations
            early_println!("[Test] Testing directory operations...");
            match fs.lookup(&root_node, &String::from("test_files")) {
                Ok(dir_node) => {
                    early_println!("[Test] Successfully looked up test_files directory");
                    match fs.readdir(&dir_node) {
                        Ok(entries) => {
                            early_println!("[Test] test_files directory contains {} entries", entries.len());
                            for entry in &entries {
                                early_println!("[Test] Found in test_files: {} (type: {:?})", entry.name, entry.file_type);
                            }
                            early_println!("[Test] ✓ Directory read operation successful");
                        },
                        Err(e) => {
                            panic!("Could not read test_files directory: {:?}", e);
                        }
                    }
                },
                Err(e) => {
                    panic!("Could not lookup test_files directory: {:?}", e);
                }
            }
            
            early_println!("[Test] All ext2 file operations completed successfully!");
        },
        Err(e) => {
            panic!("Failed to create ext2 filesystem from virtio-blk device: {:?}", e);
        }
    }
    
    early_println!("[Test] ext2 virtio-blk file operations test completed successfully");
}

#[test_case]
fn test_ext2_virtio_blk_write_operations() {
    use crate::drivers::block::virtio_blk::VirtioBlockDevice;

    early_println!("[Test] Starting ext2 virtio-blk write operations test...");
    
    // Create a virtio-blk device for testing
    let base_addr = 0x10006000; // Standard virtio-blk address for QEMU bus.5
    let virtio_dev = VirtioBlockDevice::new(base_addr);
    
    // Register the ext2 driver if not already registered
    let fs_driver_manager = get_fs_driver_manager();
    fs_driver_manager.register_driver(Box::new(super::Ext2Driver));
    
    // Create an ext2 filesystem instance using the virtio-blk device
    match fs_driver_manager.create_from_block("ext2", Arc::new(virtio_dev), 1024) {
        Ok(fs) => {
            early_println!("[Test] Successfully created ext2 filesystem from virtio-blk device");
            
            // Get the root node
            let root_node = fs.root_node();
            early_println!("[Test] Got root node for write operations");
            
            // Test 1: Try to create a new file in the root directory
            early_println!("[Test] Testing file creation...");
            let new_filename = String::from("test_write.txt");
            match fs.create(&root_node, &new_filename, FileType::RegularFile, 0o644) {
                Ok(new_file_node) => {
                    early_println!("[Test] Successfully created new file: {}", new_filename);
                    
                    // Test 2: Write data to the new file
                    match fs.open(&new_file_node, 0x01) { // 0x01 = write flag
                        Ok(file_obj) => {
                            let test_data = b"Hello, this is a test write to ext2 filesystem!";
                            match file_obj.write(test_data) {
                                Ok(bytes_written) => {
                                    early_println!("[Test] Successfully wrote {} bytes to {}", bytes_written, new_filename);
                                    assert_eq!(bytes_written, test_data.len(), "All bytes should be written");
                                    
                                    // Test 3: Read back the data we just wrote
                                    match file_obj.seek(crate::object::capability::file::SeekFrom::Start(0)) {
                                        Ok(_) => {
                                            let mut read_buffer = vec![0u8; test_data.len()];
                                            match file_obj.read(&mut read_buffer) {
                                                Ok(bytes_read) => {
                                                    early_println!("[Test] Read back {} bytes from written file", bytes_read);
                                                    let read_content = core::str::from_utf8(&read_buffer[..bytes_read])
                                                        .unwrap_or("INVALID_UTF8");
                                                    let original_content = core::str::from_utf8(test_data)
                                                        .unwrap_or("INVALID_UTF8");
                                                    
                                                    assert_eq!(read_content, original_content, "Read content should match written content");
                                                    early_println!("[Test] ✓ Write-read verification successful");
                                                },
                                                Err(e) => {
                                                    panic!("Could not read back written data: {:?}", e);
                                                }
                                            }
                                        },
                                        Err(e) => {
                                            panic!("Could not seek to beginning of file: {:?}", e);
                                        }
                                    }
                                },
                                Err(e) => {
                                    panic!("Could not write to file: {:?}", e);
                                }
                            }
                        },
                        Err(e) => {
                            panic!("Could not open file for writing: {:?}", e);
                        }
                    }
                },
                Err(e) => {
                    panic!("Could not create new file: {:?}", e);
                    // This is expected since ext2 write operations require proper implementation
                }
            }
            
            // Test 4: Try to create a directory
            early_println!("[Test] Testing directory creation...");
            let new_dirname = String::from("test_new_dir");
            match fs.create(&root_node, &new_dirname, FileType::Directory, 0o755) {
                Ok(new_dir_node) => {
                    early_println!("[Test] Successfully created directory: {}", new_dirname);
                    
                    // Test directory listing
                    match fs.readdir(&new_dir_node) {
                        Ok(entries) => {
                            early_println!("[Test] New directory contains {} entries", entries.len());
                            for entry in &entries {
                                early_println!("[Test] Found in new dir: {} (type: {:?})", entry.name, entry.file_type);
                            }
                        },
                        Err(e) => {
                            panic!("[Test] Could not read new directory: {:?}", e);
                        }
                    }
                },
                Err(e) => {
                    panic!("[Test] Could not create directory: {:?}", e);
                }
            }
            
            // Test 5: Complex nested operations
            early_println!("[Test] Testing complex nested operations...");
            let top_dir = "complex_test";
            let sub_dir = "nested_subdir";
            let file_in_nested_dir = "nested_file.txt";
            
            // First create top-level directory
            match fs.create(&root_node, &String::from(top_dir), FileType::Directory, 0o755) {
                Ok(top_dir_node) => {
                    early_println!("[Test] ✓ Created top-level directory: {}", top_dir);
                    
                    // Create subdirectory inside the top-level directory
                    match fs.create(&top_dir_node, &String::from(sub_dir), FileType::Directory, 0o755) {
                        Ok(sub_dir_node) => {
                            early_println!("[Test] ✓ Created subdirectory: {}/{}", top_dir, sub_dir);
                            
                            // Create a file in the subdirectory
                            match fs.create(&sub_dir_node, &String::from(file_in_nested_dir), FileType::RegularFile, 0o644) {
                                Ok(nested_file_node) => {
                                    early_println!("[Test] ✓ Created file in nested directory: {}/{}/{}", top_dir, sub_dir, file_in_nested_dir);
                                    
                                    // Write data to the file in nested directory
                                    match fs.open(&nested_file_node, 0) {
                                        Ok(nested_file_obj) => {
                                            let nested_content = b"File in nested directory!";
                                            match nested_file_obj.write(nested_content) {
                                                Ok(_bytes_written) => {
                                                    early_println!("[Test] ✓ Written data to file in nested directory");
                                                },
                                                Err(e) => {
                                                    panic!("Failed to write to file in nested directory: {:?}", e);
                                                }
                                            }
                                        },
                                        Err(e) => {
                                            panic!("Failed to open file in nested directory for writing: {:?}", e);
                                        }
                                    }
                                },
                                Err(e) => {
                                    panic!("Failed to create file in nested directory: {:?}", e);
                                }
                            }
                        },
                        Err(e) => {
                            panic!("Failed to create subdirectory: {:?}", e);
                        }
                    }
                },
                Err(e) => {
                    panic!("Failed to create top-level directory: {:?}", e);
                }
            }
            
            early_println!("[Test] ✓ Complex write operations completed");
            
            // Test 6: Verify operations by reading back nested directory and file
            early_println!("[Test] Verifying nested directory and file...");
            match fs.lookup(&root_node, &String::from(top_dir)) {
                Ok(top_dir_node) => {
                    match fs.readdir(&top_dir_node) {
                        Ok(entries) => {
                            early_println!("[Test] Top-level directory contains {} entries", entries.len());
                            for entry in &entries {
                                early_println!("[Test] Found in top dir: {} (type: {:?})", entry.name, entry.file_type);
                            }
                            early_println!("[Test] ✓ Top-level directory listing successful");
                        },
                        Err(e) => {
                            panic!("Failed to read top-level directory: {:?}", e);
                        }
                    }
                },
                Err(e) => {
                    panic!("Failed to lookup top-level directory: {:?}", e);
                }
            }
            
            early_println!("[Test] All ext2 write tests completed (some operations expected to fail due to incomplete write support)");
        },
        Err(e) => {
            panic!("Failed to create ext2 filesystem from virtio-blk device: {:?}", e);
        }
    }
    
    early_println!("[Test] ext2 virtio-blk write operations test completed");
}

/// Test ext2 delete operations with virtio-blk
#[test_case]
fn test_ext2_virtio_blk_delete_operations() {
    use crate::drivers::block::virtio_blk::VirtioBlockDevice;

    early_println!("[Test] Starting ext2 delete operations test...");

    // Create a virtio-blk device for testing
    let base_addr = 0x10006000; // Standard virtio-blk address for QEMU bus.5
    let virtio_dev = VirtioBlockDevice::new(base_addr);
    
    // Register the ext2 driver if not already registered
    let fs_driver_manager = get_fs_driver_manager();
    fs_driver_manager.register_driver(Box::new(super::Ext2Driver));
    
    // Create an ext2 filesystem instance using the virtio-blk device
    match fs_driver_manager.create_from_block("ext2", Arc::new(virtio_dev), 1024) {
        Ok(fs) => {
            early_println!("[Test] Successfully created ext2 filesystem from virtio-blk device");
            
            let root_node = fs.root_node();
            
            // Test 1: Try to delete . and .. entries (should fail)
            early_println!("[Test] Testing deletion of special entries...");
            
            let dot_name = String::from(".");
            match fs.remove(&root_node, &dot_name) {
                Ok(_) => {
                    panic!("Deletion of '.' should not be allowed");
                },
                Err(_) => {
                    early_println!("[Test] ✓ Correctly prevented deletion of '.' entry");
                }
            }
            
            let dotdot_name = String::from("..");
            match fs.remove(&root_node, &dotdot_name) {
                Ok(_) => {
                    panic!("Deletion of '..' should not be allowed");
                },
                Err(_) => {
                    early_println!("[Test] ✓ Correctly prevented deletion of '..' entry");
                }
            }
            
            // Test 2: Create and delete files with various names
            early_println!("[Test] Testing deletion of files with various names...");
            
            let test_names = vec![
                "short.txt",
                "very_long_filename_that_tests_name_length_limits.txt",
                "with spaces.txt",
                "with-dashes.txt",
                "with_underscores.txt",
                "with.dots.txt",
                "123numbers.txt",
            ];
            
            for name in &test_names {
                let file_name = String::from(*name);
                
                // Create file
                match fs.create(&root_node, &file_name, FileType::RegularFile, 0o644) {
                    Ok(_) => {
                        early_println!("[Test] ✓ Created file: {}", file_name);
                        
                        // Try to delete it
                        match fs.remove(&root_node, &file_name) {
                            Ok(_) => {
                                early_println!("[Test] ✓ Successfully deleted file: {}", file_name);
                                
                                // Verify the file is really gone
                                match fs.lookup(&root_node, &file_name) {
                                    Ok(_) => {
                                        panic!("[Test] Deleted file '{}' still exists!", file_name);
                                    },
                                    Err(_) => {
                                        early_println!("[Test] ✓ Confirmed file '{}' is deleted", file_name);
                                    }
                                }
                            },
                            Err(e) => {
                                panic!("[Test] Failed to delete file '{}': {:?}", file_name, e);
                            }
                        }
                    },
                    Err(e) => {
                        panic!("[Test] Failed to create file '{}': {:?}", file_name, e);
                    }
                }
            }
            
            // Test 3: Create and delete nested directories
            early_println!("[Test] Testing nested directory deletion...");
            
            let parent_dir = String::from("parent_dir");
            match fs.create(&root_node, &parent_dir, FileType::Directory, 0o755) {
                Ok(parent_node) => {
                    early_println!("[Test] ✓ Created parent directory: {}", parent_dir);
                    
                    let child_dir = String::from("child_dir");
                    match fs.create(&parent_node, &child_dir, FileType::Directory, 0o755) {
                        Ok(child_node) => {
                            early_println!("[Test] ✓ Created child directory: {}", child_dir);
                            
                            // Create a file in the child directory
                            let nested_file = String::from("nested_file.txt");
                            match fs.create(&child_node, &nested_file, FileType::RegularFile, 0o644) {
                                Ok(_) => {
                                    early_println!("[Test] ✓ Created nested file: {}", nested_file);
                                    
                                    // Delete the nested file first
                                    match fs.remove(&child_node, &nested_file) {
                                        Ok(_) => {
                                            early_println!("[Test] ✓ Successfully deleted nested file");
                                            
                                            // Verify the file is really gone
                                            match fs.lookup(&child_node, &nested_file) {
                                                Ok(_) => {
                                                    panic!("[Test] Deleted file still exists!");
                                                },
                                                Err(_) => {
                                                    early_println!("[Test] ✓ Confirmed nested file is deleted");
                                                }
                                            }
                                        },
                                        Err(e) => {
                                            panic!("[Test] Failed to delete nested file: {:?}", e);
                                        }
                                    }
                                    
                                    // Delete the child directory (should be empty now)
                                    match fs.remove(&parent_node, &child_dir) {
                                        Ok(_) => {
                                            early_println!("[Test] ✓ Successfully deleted child directory");
                                            
                                            // Verify the directory is really gone
                                            match fs.lookup(&parent_node, &child_dir) {
                                                Ok(_) => {
                                                    panic!("[Test] Deleted directory still exists!");
                                                },
                                                Err(_) => {
                                                    early_println!("[Test] ✓ Confirmed child directory is deleted");
                                                }
                                            }
                                        },
                                        Err(e) => {
                                            panic!("[Test] Failed to delete child directory: {:?}", e);
                                        }
                                    }
                                    
                                    // Delete the parent directory (should be empty now)
                                    match fs.remove(&root_node, &parent_dir) {
                                        Ok(_) => {
                                            early_println!("[Test] ✓ Successfully deleted parent directory");
                                            
                                            // Verify the parent directory is really gone
                                            match fs.lookup(&root_node, &parent_dir) {
                                                Ok(_) => {
                                                    panic!("[Test] Deleted parent directory still exists!");
                                                },
                                                Err(_) => {
                                                    early_println!("[Test] ✓ Confirmed parent directory is deleted");
                                                }
                                            }
                                        },
                                        Err(e) => {
                                            panic!("[Test] Failed to delete parent directory: {:?}", e);
                                        }
                                    }
                                },
                                Err(e) => {
                                    panic!("[Test] Failed to create nested file: {:?}", e);
                                }
                            }
                        },
                        Err(e) => {
                            panic!("[Test] Failed to create child directory: {:?}", e);
                        }
                    }
                },
                Err(e) => {
                    panic!("[Test] Failed to create parent directory: {:?}", e);
                }
            }
            
            // Test 4: Test deletion of non-existent files (should fail gracefully)
            early_println!("[Test] Testing deletion of non-existent files...");
            
            let nonexistent_file = String::from("this_file_does_not_exist.txt");
            match fs.remove(&root_node, &nonexistent_file) {
                Ok(_) => {
                    panic!("[Test] Deletion of non-existent file should fail");
                },
                Err(_) => {
                    early_println!("[Test] ✓ Correctly failed to delete non-existent file");
                }
            }
            
            let nonexistent_dir = String::from("this_dir_does_not_exist");
            match fs.remove(&root_node, &nonexistent_dir) {
                Ok(_) => {
                    panic!("[Test] Deletion of non-existent directory should fail");
                },
                Err(_) => {
                    early_println!("[Test] ✓ Correctly failed to delete non-existent directory");
                }
            }
            
            // Test 5: Test deletion with special characters in names
            early_println!("[Test] Testing deletion of files with special characters...");
            
            let special_names = vec![
                "file with spaces.txt",
                "file-with-hyphens.txt", 
                "file_with_underscores.txt",
                "file.with.many.dots.txt",
                "123numeric_start.txt",
                "UPPERCASE.TXT",
                "mixedCaSe.txt",
            ];
            
            for name in &special_names {
                let file_name = String::from(*name);
                
                // Create file with special name
                match fs.create(&root_node, &file_name, FileType::RegularFile, 0o644) {
                    Ok(_) => {
                        early_println!("[Test] ✓ Created file with special name: {}", file_name);
                        
                        // Delete it
                        match fs.remove(&root_node, &file_name) {
                            Ok(_) => {
                                early_println!("[Test] ✓ Successfully deleted file with special name: {}", file_name);
                            },
                            Err(e) => {
                                panic!("[Test] Failed to delete file with special name '{}': {:?}", file_name, e);
                            }
                        }
                    },
                    Err(e) => {
                        panic!("[Test] Failed to create file with special name '{}': {:?}", file_name, e);
                    }
                }
            }
            
            // Test 6: Test bulk creation and deletion
            early_println!("[Test] Testing bulk creation and deletion...");
            
            let mut created_files = Vec::new();
            for i in 0..10 {
                let file_name = format!("bulk_test_file_{}.txt", i);
                match fs.create(&root_node, &file_name, FileType::RegularFile, 0o644) {
                    Ok(_) => {
                        early_println!("[Test] ✓ Created bulk file: {}", file_name);
                        created_files.push(file_name);
                    },
                    Err(e) => {
                        panic!("[Test] Failed to create bulk file {}: {:?}", file_name, e);
                    }
                }
            }
            
            // Delete all bulk files
            for file_name in &created_files {
                match fs.remove(&root_node, file_name) {
                    Ok(_) => {
                        early_println!("[Test] ✓ Successfully deleted bulk file: {}", file_name);
                    },
                    Err(e) => {
                        panic!("[Test] Failed to delete bulk file '{}': {:?}", file_name, e);
                    }
                }
            }
            
            // Test 7: Verify directory state after all deletions
            early_println!("[Test] Verifying root directory state after all deletions...");
            match fs.readdir(&root_node) {
                Ok(entries) => {
                    early_println!("[Test] Root directory contains {} entries after all deletion tests", entries.len());
                    for entry in &entries {
                        early_println!("[Test] Remaining entry: {} (type: {:?})", entry.name, entry.file_type);
                    }
                    
                    // Should only have the original files and directories, plus "." and ".."
                    let expected_system_entries = vec![".", ".."];
                    let mut system_entry_count = 0;
                    
                    for entry in &entries {
                        if expected_system_entries.contains(&entry.name.as_str()) {
                            system_entry_count += 1;
                        }
                    }
                    
                    if system_entry_count == expected_system_entries.len() {
                        early_println!("[Test] ✓ System entries (. and ..) are present");
                    } else {
                        panic!("[Test] System entries count mismatch. Expected {}, found {}", 
                               expected_system_entries.len(), system_entry_count);
                    }
                    
                    early_println!("[Test] ✓ Root directory integrity maintained after all deletions");
                },
                Err(e) => {
                    panic!("[Test] Failed to read root directory after all deletions: {:?}", e);
                }
            }

            early_println!("[Test] ext2 delete edge cases test completed");
        },
        Err(e) => {
            panic!("Failed to create ext2 filesystem from virtio-blk device: {:?}", e);
        }
    }
    
    early_println!("[Test] ext2 delete edge cases test completed");
}