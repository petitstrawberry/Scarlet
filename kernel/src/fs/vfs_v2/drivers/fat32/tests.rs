//! Tests for FAT32 filesystem implementation

use super::*;
use crate::device::block::mockblk::MockBlockDevice;
use crate::fs::get_fs_driver_manager;

#[test_case]
fn test_fat32_driver_registration() {
    let fs_driver_manager = get_fs_driver_manager();
    let driver_type = fs_driver_manager.get_driver_type("fat32");
    assert_eq!(driver_type, Some(FileSystemType::Block));
}

#[test_case] 
fn test_fat32_boot_sector_validation() {
    let mut boot_sector = Fat32BootSector {
        jump_instruction: [0xEB, 0x58, 0x90],
        oem_name: *b"MSWIN4.1",
        bytes_per_sector: 512,
        sectors_per_cluster: 8,
        reserved_sectors: 32,
        fat_count: 2,
        max_root_entries: 0,
        total_sectors_16: 0,
        media_descriptor: 0xF8,
        sectors_per_fat_16: 0,
        sectors_per_track: 63,
        heads: 255,
        hidden_sectors: 0,
        total_sectors_32: 65536,
        sectors_per_fat: 512,
        extended_flags: 0,
        fs_version: 0,
        root_cluster: 2,
        fs_info_sector: 1,
        backup_boot_sector: 6,
        reserved: [0; 12],
        drive_number: 0x80,
        reserved1: 0,
        boot_signature: 0x29,
        volume_serial: 0x12345678,
        volume_label: *b"NO NAME    ",
        fs_type: *b"FAT32   ",
        boot_code: [0; 420],
        signature: 0xAA55,
    };
    
    // Valid boot sector should pass validation
    assert!(boot_sector.is_valid());
    
    // Invalid signature should fail
    boot_sector.signature = 0xAA56;
    assert!(!boot_sector.is_valid());
    
    // Reset signature
    boot_sector.signature = 0xAA55;
    
    // Invalid bytes per sector should fail
    boot_sector.bytes_per_sector = 511;
    assert!(!boot_sector.is_valid());
    
    // Reset bytes per sector
    boot_sector.bytes_per_sector = 512;
    
    // Invalid sectors per cluster should fail (not power of 2)
    boot_sector.sectors_per_cluster = 7;
    assert!(!boot_sector.is_valid());
    
    // Reset to valid value
    boot_sector.sectors_per_cluster = 8;
    assert!(boot_sector.is_valid());
}

#[test_case]
fn test_fat32_directory_entry() {
    let entry = Fat32DirectoryEntry::new_file("TEST.TXT", 100, 1024);
    
    // Test basic properties
    assert!(!entry.is_free());
    assert!(!entry.is_last());
    assert!(!entry.is_long_filename());
    assert!(!entry.is_directory());
    assert!(entry.is_file());
    assert!(!entry.is_volume_label());
    
    // Test cluster number
    assert_eq!(entry.cluster(), 100);
    
    // Test file size (copy to avoid unaligned reference)
    let file_size = entry.file_size;
    assert_eq!(file_size, 1024);
    
    // Test filename
    let filename = entry.filename();
    assert_eq!(filename, "TEST.TXT");
    
    // Test directory entry
    let dir_entry = Fat32DirectoryEntry::new_directory("TESTDIR", 200);
    assert!(dir_entry.is_directory());
    assert!(!dir_entry.is_file());
    assert_eq!(dir_entry.cluster(), 200);
    // Test directory file size (copy to avoid unaligned reference)
    let dir_file_size = dir_entry.file_size;
    assert_eq!(dir_file_size, 0);
}

#[test_case]
fn test_fat32_create_from_mock_device() {
    let driver = Fat32Driver;
    
    // Create a mock block device with minimal valid boot sector
    let mock_device = MockBlockDevice::new("test_fat32", 512, 1000);
    
    // For this test, we expect it to fail because the mock device 
    // doesn't have a proper FAT32 boot sector
    let result = driver.create_from_block(Arc::new(mock_device), 512);
    
    // Should fail due to invalid boot sector
    assert!(result.is_err());
    
    // Verify it's the right type of error
    match result {
        Err(e) => {
            assert!(
                e.kind == FileSystemErrorKind::IoError || 
                e.kind == FileSystemErrorKind::InvalidData
            );
        },
        Ok(_) => panic!("Expected error but got success"),
    }
}

#[test_case]
fn test_fat32_filesystem_type() {
    let driver = Fat32Driver;
    assert_eq!(driver.name(), "fat32");
    assert_eq!(driver.filesystem_type(), FileSystemType::Block);
    
    // FAT32 should not support creation without block device
    assert!(driver.create().is_err());
    assert!(driver.create_from_option_string("").is_err());
}

#[test_case]
fn test_fat32_node_creation() {
    let file_node = Fat32Node::new_file("test.txt".to_string(), 1, 100);
    let dir_node = Fat32Node::new_directory("testdir".to_string(), 2, 200);
    
    // Test file node
    assert_eq!(file_node.id(), 1);
    assert_eq!(file_node.cluster(), 100);
    match file_node.file_type() {
        Ok(FileType::RegularFile) => {},
        _ => panic!("Expected regular file type"),
    }
    
    // Test directory node
    assert_eq!(dir_node.id(), 2);
    assert_eq!(dir_node.cluster(), 200);
    match dir_node.file_type() {
        Ok(FileType::Directory) => {},
        _ => panic!("Expected directory type"),
    }
}