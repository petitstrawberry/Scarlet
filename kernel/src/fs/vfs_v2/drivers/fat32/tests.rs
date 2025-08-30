//! Tests for FAT32 filesystem implementation

use super::*;
use crate::device::block::mockblk::MockBlockDevice;
use crate::fs::get_fs_driver_manager;
use alloc::{boxed::Box, format, vec::Vec};

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

#[test_case]
fn test_fat32_mockdevice_cluster_io() {
    // Create a mock device with proper FAT32 structure
    let mock_device = create_test_fat32_device();
    let fat32_fs = Fat32FileSystem::new(Arc::new(mock_device)).expect("Failed to create FAT32 filesystem");
    
    // Test cluster write and read
    let test_data = b"Hello, FAT32 cluster test!";
    let test_cluster = 3; // Start from cluster 3 (2 is root)
    
    // Write test data to cluster
    fat32_fs.write_cluster(test_cluster, test_data).expect("Failed to write cluster");
    
    // Read back the data
    let read_data = fat32_fs.read_cluster(test_cluster).expect("Failed to read cluster");
    
    // Verify data matches (check first part since cluster may be larger)
    assert_eq!(&read_data[..test_data.len()], test_data);
    
    // Verify rest is padded with zeros
    let cluster_size = (fat32_fs.sectors_per_cluster * fat32_fs.bytes_per_sector) as usize;
    for i in test_data.len()..cluster_size {
        assert_eq!(read_data[i], 0);
    }
}

#[test_case]
fn test_fat32_mockdevice_fat_operations() {
    // Create a mock device with proper FAT32 structure
    let mock_device = create_test_fat32_device();
    let fat32_fs = Fat32FileSystem::new(Arc::new(mock_device)).expect("Failed to create FAT32 filesystem");
    
    // Test FAT entry read/write
    let test_cluster = 100;
    let test_value = 0x0FFFFFF8; // End of chain marker
    
    // Write FAT entry
    fat32_fs.write_fat_entry(test_cluster, test_value).expect("Failed to write FAT entry");
    
    // Read back FAT entry
    let read_value = fat32_fs.read_fat_entry(test_cluster).expect("Failed to read FAT entry");
    
    // Verify values match
    assert_eq!(read_value, test_value);
    
    // Test chain of clusters
    let cluster1 = 50;
    let cluster2 = 51;
    let cluster3 = 52;
    
    // Set up cluster chain: 50 -> 51 -> 52 -> EOF
    fat32_fs.write_fat_entry(cluster1, cluster2).expect("Failed to write FAT entry");
    fat32_fs.write_fat_entry(cluster2, cluster3).expect("Failed to write FAT entry");
    fat32_fs.write_fat_entry(cluster3, 0x0FFFFFF8).expect("Failed to write FAT entry");
    
    // Verify chain
    assert_eq!(fat32_fs.read_fat_entry(cluster1).expect("Failed to read FAT entry"), cluster2);
    assert_eq!(fat32_fs.read_fat_entry(cluster2).expect("Failed to read FAT entry"), cluster3);
    assert_eq!(fat32_fs.read_fat_entry(cluster3).expect("Failed to read FAT entry"), 0x0FFFFFF8);
}

#[test_case]
fn test_fat32_mockdevice_file_content_io() {
    // Create a mock device with proper FAT32 structure
    let mock_device = create_test_fat32_device();
    let fat32_fs = Fat32FileSystem::new(Arc::new(mock_device)).expect("Failed to create FAT32 filesystem");
    
    // Test small file (single cluster)
    let small_data = b"This is a small test file content.";
    let start_cluster = 10;
    
    // Write file content
    fat32_fs.write_file_content(start_cluster, small_data).expect("Failed to write file content");
    
    // Read back file content
    let read_content = fat32_fs.read_file_content(start_cluster, small_data.len()).expect("Failed to read file content");
    
    // Verify content matches
    assert_eq!(read_content, small_data);
    
    // Test larger file (multiple clusters)
    let mut large_data = Vec::new();
    for i in 0..10000 {
        large_data.extend_from_slice(format!("Line {}: This is test data for multi-cluster file.\n", i).as_bytes());
    }
    
    let large_start_cluster = 20;
    
    // Write large file content
    fat32_fs.write_file_content(large_start_cluster, &large_data).expect("Failed to write large file content");
    
    // Read back large file content
    let read_large_content = fat32_fs.read_file_content(large_start_cluster, large_data.len()).expect("Failed to read large file content");
    
    // Verify content matches
    assert_eq!(read_large_content, large_data);
}

#[test_case]
fn test_fat32_mockdevice_partial_read() {
    // Create a mock device with proper FAT32 structure
    let mock_device = create_test_fat32_device();
    let fat32_fs = Fat32FileSystem::new(Arc::new(mock_device)).expect("Failed to create FAT32 filesystem");
    
    // Write some test data with a specific pattern to detect read boundaries
    let test_data = b"This is a longer test file that we will read partially to test partial read functionality.";
    let start_cluster = 15;
    
    fat32_fs.write_file_content(start_cluster, test_data).expect("Failed to write file content");
    
    // Test partial reads of different sizes
    let partial_size = 20;
    let partial_content = fat32_fs.read_file_content(start_cluster, partial_size).expect("Failed to read partial content");
    
    assert_eq!(partial_content.len(), partial_size);
    assert_eq!(partial_content, &test_data[..partial_size]);
    
    // Test reading exactly the file size
    let exact_content = fat32_fs.read_file_content(start_cluster, test_data.len()).expect("Failed to read exact content");
    assert_eq!(exact_content.len(), test_data.len());
    assert_eq!(exact_content, test_data);
    
    // Test reading more than available - the issue is that read_file_content
    // should use the file system's knowledge of file size, but we're calling
    // it directly without that context. In a real scenario, the file size
    // would be tracked in directory entries.
    
    // For now, let's test that we can read a full cluster and verify the content
    let cluster_size = (fat32_fs.sectors_per_cluster * fat32_fs.bytes_per_sector) as usize;
    let full_cluster = fat32_fs.read_cluster(start_cluster).expect("Failed to read full cluster");
    
    // The first part should match our test data
    assert_eq!(&full_cluster[..test_data.len()], test_data);
    
    // The rest should be zeros (since write_cluster pads with zeros)
    for i in test_data.len()..cluster_size {
        assert_eq!(full_cluster[i], 0, "Expected zero at position {} but found {}", i, full_cluster[i]);
    }
}

// Helper function to create a mock FAT32 device with proper structure
fn create_test_fat32_device() -> MockBlockDevice {
    let sector_size = 512;
    let sector_count = 65536; // 32MB device
    let mut mock_device = MockBlockDevice::new("test_fat32", sector_size, sector_count);
    
    // Create and write boot sector
    let boot_sector = Fat32BootSector {
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
        total_sectors_32: sector_count as u32,
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
    
    // Convert boot sector to bytes
    let boot_sector_bytes = unsafe {
        core::slice::from_raw_parts(
            &boot_sector as *const _ as *const u8,
            core::mem::size_of::<Fat32BootSector>()
        ).to_vec()
    };
    
    // Write boot sector to device
    let boot_request = Box::new(crate::device::block::request::BlockIORequest {
        request_type: crate::device::block::request::BlockIORequestType::Write,
        sector: 0,
        sector_count: 1,
        head: 0,
        cylinder: 0,
        buffer: boot_sector_bytes,
    });
    
    mock_device.enqueue_request(boot_request);
    mock_device.process_requests();
    
    // Initialize FAT tables with proper values
    let fat_start_sector = boot_sector.reserved_sectors as usize;
    
    // Initialize all FAT sectors to zero (free clusters)
    for fat_copy in 0..boot_sector.fat_count {
        for sector_offset in 0..boot_sector.sectors_per_fat {
            let fat_sector_addr = fat_start_sector + (fat_copy as usize * boot_sector.sectors_per_fat as usize) + sector_offset as usize;
            
            let mut fat_sector = vec![0u8; sector_size];
            
            // Only set special entries in the first sector
            if sector_offset == 0 {
                // Cluster 0: 0x0FFFFFF8 (media descriptor)
                fat_sector[0] = 0xF8;
                fat_sector[1] = 0xFF;
                fat_sector[2] = 0xFF;
                fat_sector[3] = 0x0F;
                
                // Cluster 1: 0x0FFFFFFF (EOF)
                fat_sector[4] = 0xFF;
                fat_sector[5] = 0xFF;
                fat_sector[6] = 0xFF;
                fat_sector[7] = 0x0F;
                
                // Cluster 2 (root): 0x0FFFFFFF (EOF)
                fat_sector[8] = 0xFF;
                fat_sector[9] = 0xFF;
                fat_sector[10] = 0xFF;
                fat_sector[11] = 0x0F;
                
                // All other clusters in this sector remain 0 (free)
            }
            // All other sectors remain all zeros (free clusters)
            
            let fat_request = Box::new(crate::device::block::request::BlockIORequest {
                request_type: crate::device::block::request::BlockIORequestType::Write,
                sector: fat_sector_addr,
                sector_count: 1,
                head: 0,
                cylinder: 0,
                buffer: fat_sector,
            });
            
            mock_device.enqueue_request(fat_request);
            mock_device.process_requests();
        }
    }
    
    mock_device
}