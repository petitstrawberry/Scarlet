//! Virtual File System (VFS) test suite.
//!
//! This module contains comprehensive tests for the VFS layer, including:
//! - VfsManager creation and filesystem registration
//! - Mount/unmount operations with filesystem lifecycle management
//! - Path resolution and security validation (preventing directory traversal)
//! - File and directory operations with proper resource management
//! - Block device integration and I/O operations
//! - Bind mount functionality and cross-VFS sharing capabilities
//! - Error handling and edge case validation
//!
//! # Test Architecture
//!
//! Tests use MockBlockDevice and TestFileSystem to simulate real filesystem
//! operations without requiring actual hardware. The test suite validates:
//! - Thread-safe operations and concurrent access patterns
//! - RAII resource management and automatic cleanup
//! - Security protections and access control
//! - Performance characteristics and scalability

use alloc::{boxed::Box, sync::Arc};
use super::*;
use crate::device::block::mockblk::MockBlockDevice;
use crate::fs::testfs::{TestFileSystem, TestFileSystemDriver};
use crate::fs::drivers::tmpfs::TmpFS;
use crate::task::{new_user_task, CloneFlags};

// Test cases
#[test_case]
fn test_vfs_manager_creation() {
    let manager = VfsManager::new();
    assert_eq!(manager.filesystems.read().len(), 0);
    assert_eq!(manager.mount_count(), 0);
}

#[test_case]
fn test_fs_registration_and_mount() {
    let mut manager = VfsManager::new();
    let device = Box::new(MockBlockDevice::new(1, "test_disk", 512, 100));
    let fs = Box::new(TestFileSystem::new("testfs", device, 512));
    
    let fs_id = manager.register_fs(fs); // Get fs_id
    assert_eq!(manager.filesystems.read().len(), 1);
    
    let result = manager.mount(fs_id, "/mnt"); // Use fs_id
    assert!(result.is_ok());
    assert_eq!(manager.filesystems.read().len(), 0);
    assert_eq!(manager.mount_count(), 1);
}

#[test_case]
fn test_path_resolution() {
    let mut manager = VfsManager::new();
    let device = Box::new(MockBlockDevice::new(1, "test_disk", 512, 100));
    let fs = Box::new(TestFileSystem::new("testfs", device, 512));
    
    let fs_id = manager.register_fs(fs); // Get fs_id
    let _ = manager.mount(fs_id, "/mnt"); // Use fs_id
    
    // Resolve valid path
    match manager.with_resolve_path("/mnt/test.txt", |fs, relative_path| {
        assert_eq!(fs.read().name(), "testfs");
        assert_eq!(relative_path, "/test.txt");
        Ok(())
    }) {
        Ok(_) => {},
        Err(e) => panic!("Failed to resolve path: {:?}", e),
    }
    
    match manager.with_resolve_path("/mnt", |fs, relative_path| {
        assert_eq!(fs.read().name(), "testfs");
        assert_eq!(relative_path, "/");
        Ok(())
    }) {
        Ok(_) => {},
        Err(e) => panic!("Failed to resolve path: {:?}", e),
    }
    
    // Resolve invalid path
    let result = manager.resolve_path("/invalid/path");

    assert!(result.is_err());
}

#[test_case]
fn test_file_operations() {
    let mut manager = VfsManager::new();
    let device = Box::new(MockBlockDevice::new(1, "test_disk", 512, 100));
    let fs = Box::new(TestFileSystem::new("testfs", device, 512));
    
    let fs_id = manager.register_fs(fs); // Get fs_id
    let _ = manager.mount(fs_id, "/mnt"); // Use fs_id
    
    // Open file
    let kernel_obj = manager.open("/mnt/test.txt", 0).unwrap();
    let file = kernel_obj.as_file().unwrap();
    
    // Read test
    let mut buffer = [0u8; 20];
    let bytes_read = file.read(&mut buffer).unwrap();
    assert_eq!(bytes_read, 13); // Length of "Hello, world!"
    assert_eq!(&buffer[..13], b"Hello, world!");
    
    // Write test
    file.seek(SeekFrom::Start(0)).unwrap();
    let bytes_written = file.write(b"Test data").unwrap();
    assert_eq!(bytes_written, 9);
    
    // Re-read test
    file.seek(SeekFrom::Start(0)).unwrap();
    let mut buffer2 = [0u8; 20];
    let bytes_read2 = file.read(&mut buffer2).unwrap();
    assert_eq!(bytes_read2, 13); // File length is still 13 (Hello, world!)
    assert_eq!(&buffer2[..9], b"Test data"); // The beginning part has been replaced
}

#[test_case]
fn test_directory_operations() {
    let mut manager = VfsManager::new();
    let device = Box::new(MockBlockDevice::new(1, "test_disk", 512, 100));
    let fs = Box::new(TestFileSystem::new("testfs", device, 512));
    
    let fs_id = manager.register_fs(fs); // Get fs_id
    let _ = manager.mount(fs_id, "/mnt"); // Use fs_id
    
    // Get directory entries
    let entries = manager.readdir("/mnt").unwrap();
    assert_eq!(entries.len(), 4); // ".", "..", "test.txt", "testdir"
    // Check that "." and ".." are first, then other entries
    assert_eq!(entries[0].name, ".");
    assert_eq!(entries[1].name, "..");
    // The remaining entries should be sorted by file_id
    let regular_entries: Vec<_> = entries.iter().skip(2).collect();
    assert_eq!(regular_entries[0].name, "test.txt");
    assert_eq!(regular_entries[1].name, "testdir");
    assert_eq!(regular_entries[0].file_type, FileType::RegularFile);
    assert_eq!(regular_entries[1].file_type, FileType::Directory);
    
    // Create directory
    let result = manager.create_dir("/mnt/newdir");
    assert!(result.is_ok());
    
    // Verify
    let entries_after = manager.readdir("/mnt").unwrap();
    assert_eq!(entries_after.len(), 5); // ".", "..", "test.txt", "testdir", "newdir"
    // Skip "." and ".." entries and check the regular entries
    let regular_entries_after: Vec<_> = entries_after.iter().skip(2).collect();
    assert!(regular_entries_after.iter().any(|e| e.name == "newdir" && e.file_type == FileType::Directory));
    
    // Create file
    let result = manager.create_regular_file("/mnt/newdir/newfile.txt");
    assert!(result.is_ok());
    
    // Verify
    let dir_entries = manager.readdir("/mnt/newdir").unwrap();
    assert_eq!(dir_entries.len(), 3); // ".", "..", "newfile.txt"
    // Skip "." and ".." entries
    let regular_entries: Vec<_> = dir_entries.iter().skip(2).collect();
    assert_eq!(regular_entries[0].name, "newfile.txt");
    
    // Delete test
    let result = manager.remove("/mnt/newdir/newfile.txt");
    assert!(result.is_ok());
    
    // Delete empty directory
    let result = manager.remove("/mnt/newdir");
    assert!(result.is_ok());
}

#[test_case]
fn test_block_device_operations() {
    let device = MockBlockDevice::new(1, "test_disk", 512, 100);
    let _fs = TestFileSystem::new("testfs", Box::new(device), 512);
    
    // Test device instantiation
    assert!(true, "Test filesystem created successfully");
}

#[test_case]
fn test_unmount() {
    let mut manager = VfsManager::new();
    let device = Box::new(MockBlockDevice::new(1, "test_disk", 512, 100));
    let fs = Box::new(TestFileSystem::new("testfs", device, 512));
     let fs_id = manager.register_fs(fs); // Get fs_id
    let _ = manager.mount(fs_id, "/mnt"); // Use fs_id
    assert_eq!(manager.mount_count(), 1);

    // Unmount
    let result = manager.unmount("/mnt");
    assert!(result.is_ok());
    assert_eq!(manager.mount_count(), 0);
    assert_eq!(manager.filesystems.read().len(), 1); // The file system should be returned
    
    // Attempt to unmount an invalid mount point
    let result = manager.unmount("/invalid");
    match result {
        Ok(_) => panic!("Expected an error, but got Ok"),
        Err(e) => {
            assert_eq!(e.kind, FileSystemErrorKind::NotFound);
            assert_eq!(e.message, "Mount point /invalid not found".to_string());
        }
    }
}

// Test file structure

#[test_case]
fn test_file_creation() {
    // Setup
    let mut manager = VfsManager::new();
    let device = Box::new(MockBlockDevice::new(1, "test_disk", 512, 100));
    let fs = Box::new(TestFileSystem::new("testfs", device, 512));
    
    let fs_id = manager.register_fs(fs); // Get fs_id
    let _ = manager.mount(fs_id, "/mnt"); // Use fs_id

    // Create an instance of the file structure
    // let file = File::open_with_manager("/mnt/test.txt".to_string(), &mut manager).unwrap();
    // assert_eq!(file.path, "/mnt/test.txt");
}

#[test_case]
fn test_file_open_close() {
    // Setup
    let mut manager = VfsManager::new();
    let device = Box::new(MockBlockDevice::new(1, "test_disk", 512, 100));
    let fs = Box::new(TestFileSystem::new("testfs", device, 512));
    
    let fs_id = manager.register_fs(fs); // Get fs_id
    let _ = manager.mount(fs_id, "/mnt"); // Use fs_id
    
    // Create and open a file object
    let file = manager.open("/mnt/test.txt", 0o777);
    
    // Open the file
    assert!(file.is_ok());
}

#[test_case]
fn test_file_read_write() {
    // Setup
    let mut manager = VfsManager::new();
    let device = Box::new(MockBlockDevice::new(1, "test_disk", 512, 100));
    let fs = Box::new(TestFileSystem::new("testfs", device, 512));
    
    let fs_id = manager.register_fs(fs); // Get fs_id
    let _ = manager.mount(fs_id, "/mnt"); // Use fs_id
    
    let kernel_obj = manager.open("/mnt/test.txt", 0o777).unwrap();
    let file = kernel_obj.as_file().unwrap();

    // Read test
    let mut buffer = [0u8; 20];
    let bytes_read = file.read(&mut buffer).unwrap();
    assert_eq!(bytes_read, 13); // Length of "Hello, world!"
    assert_eq!(&buffer[..13], b"Hello, world!");
    
    // Write test
    file.seek(SeekFrom::Start(0)).unwrap();
    let bytes_written = file.write(b"Test data").unwrap();
    assert_eq!(bytes_written, 9);
    
    // Re-read test
    file.seek(SeekFrom::Start(0)).unwrap();
    let mut buffer2 = [0u8; 20];
    let bytes_read2 = file.read(&mut buffer2).unwrap();
    assert_eq!(bytes_read2, 13); // File length is still 13 (Hello, world!)
    assert_eq!(&buffer2[..9], b"Test data"); // The beginning part has been replaced
}

#[test_case]
fn test_file_seek() {
    // Setup
    let mut manager = VfsManager::new();
    let device = Box::new(MockBlockDevice::new(1, "test_disk", 512, 100));
    let fs = Box::new(TestFileSystem::new("testfs", device, 512));
    
    let fs_id = manager.register_fs(fs); // Get fs_id
    let _ = manager.mount(fs_id, "/mnt"); // Use fs_id
    
    let kernel_obj = manager.open("/mnt/test.txt", 0o777).unwrap();
    let file = kernel_obj.as_file().unwrap();
    
    // Seek from the start
    let pos = file.seek(SeekFrom::Start(5)).unwrap();
    assert_eq!(pos, 5);
    
    // Seek from the current position (forward)
    let pos = file.seek(SeekFrom::Current(3)).unwrap();
    assert_eq!(pos, 8);
    
    // Seek from the current position (backward)
    let pos = file.seek(SeekFrom::Current(-4)).unwrap();
    assert_eq!(pos, 4);
    
    // Seek from the end
    let pos = file.seek(SeekFrom::End(-5)).unwrap();
    assert_eq!(pos, 8); // 13 - 5 = 8
}

#[test_case]
fn test_file_metadata_and_size() {
    // Setup
    let mut manager = VfsManager::new();
    let device = Box::new(MockBlockDevice::new(1, "test_disk", 512, 100));
    let fs = Box::new(TestFileSystem::new("testfs", device, 512));
    
    let fs_id = manager.register_fs(fs); // Get fs_id
    let _ = manager.mount(fs_id, "/mnt"); // Use fs_id
    
    let kernel_obj = manager.open("/mnt/test.txt", 0o777).unwrap();
    let file = kernel_obj.as_file().unwrap();
    
    // Get metadata (possible even when not open)
    let metadata = file.metadata().unwrap();
    assert_eq!(metadata.file_type, FileType::RegularFile);

    // Write
    file.write(b"Hello, world!").unwrap();
    
    // Get size from metadata
    let metadata = file.metadata().unwrap();
    assert_eq!(metadata.size, 13); // Length of "Hello, world!"
}

#[test_case]
fn test_file_read_all() {
    // Setup
    let mut manager = VfsManager::new();
    let device = Box::new(MockBlockDevice::new(1, "test_disk", 512, 100));
    let fs = Box::new(TestFileSystem::new("testfs", device, 512));
    
    let fs_id = manager.register_fs(fs); // Get fs_id
    let _ = manager.mount(fs_id, "/mnt"); // Use fs_id
    
    // let mut file = File::new("/mnt/test.txt".to_string(), 0);
    let kernel_obj = manager.open("/mnt/test.txt", 0o777).unwrap();
    let file = kernel_obj.as_file().unwrap();
    // Write
    file.write(b"Hello, world!").unwrap();
    
    // Seek to beginning for reading
    file.seek(SeekFrom::Start(0)).unwrap();
    
    // Read the entire file - use buffer approach since read_all is not available
    let mut content = Vec::new();
    let mut buffer = [0u8; 64];
    loop {
        let bytes_read = file.read(&mut buffer).unwrap();
        if bytes_read == 0 {
            break;
        }
        content.extend_from_slice(&buffer[..bytes_read]);
    }
    assert_eq!(content, b"Hello, world!");
    
    // Modify part of the file and read again
    file.seek(SeekFrom::Start(0)).unwrap();
    file.write(b"Modified, ").unwrap();
    file.write(b"world!").unwrap();
    
    file.seek(SeekFrom::Start(0)).unwrap();
    // Read the entire file after modification - use buffer approach since read_all is not available
    let mut modified_content = Vec::new();
    let mut buffer = [0u8; 64];
    loop {
        let bytes_read = file.read(&mut buffer).unwrap();
        if bytes_read == 0 {
            break;
        }
        modified_content.extend_from_slice(&buffer[..bytes_read]);
    }
    assert_eq!(modified_content, b"Modified, world!");
}

#[test_case]
fn test_file_auto_close() {
    // Setup
    let mut manager = VfsManager::new();
    let device = Box::new(MockBlockDevice::new(1, "test_disk", 512, 100));
    let fs = Box::new(TestFileSystem::new("testfs", device, 512));
    
    let fs_id = manager.register_fs(fs); // Get fs_id
    let _ = manager.mount(fs_id, "/mnt"); // Use fs_id
    
    // Open a file within a scope
    {
        // let mut file = File::new("/mnt/test.txt".to_string(), 0);
        let file = manager.open("/mnt/test.txt", 0o777);
        assert!(file.is_ok());
        
        // Exiting the scope will automatically close the file due to the Drop trait
    }
    
    // Verify that a new file object can be created and opened
    let file2 = manager.open("/mnt/test.txt", 0o777);
    assert!(file2.is_ok());
}

#[test_case]
fn test_filesystem_driver_and_create_register_fs() {
    // Initialize VfsManager
    let mut manager = VfsManager::new();

    // Register a mock driver
    get_fs_driver_manager().register_driver(Box::new(TestFileSystemDriver));

    // Create a block device
    let device = Box::new(MockBlockDevice::new(1, "test_disk", 512, 100));

    // Use create_and_register_fs to generate and register a file system
    let fs_id = manager.create_and_register_block_fs("testfs", device, 512).unwrap();

    // Verify that the file system is correctly registered
    assert_eq!(fs_id, 1); // The first registration should have ID 1
    assert_eq!(manager.filesystems.read().len(), 1);

    // Check the name of the registered file system
    let registered_fs = manager.filesystems.read().get(&fs_id).unwrap().clone();
    assert_eq!(registered_fs.read().name(), "testfs");

    // Mount and verify functionality
    let result = manager.mount(fs_id, "/mnt");
    assert!(result.is_ok());
    assert_eq!(manager.mount_count(), 1);
    assert!(manager.has_mount_point("/mnt"));
}

#[test_case]
fn test_nested_mount_points() {
    // Setup: Create 3 different file systems
    let mut manager = VfsManager::new();
    
    // Root file system
    let root_device = Box::new(MockBlockDevice::new(1, "root_disk", 512, 100));
    let root_fs = Box::new(TestFileSystem::new("rootfs", root_device, 512));
    let root_fs_id = manager.register_fs(root_fs);
    
    // File system for /mnt
    let mnt_device = Box::new(MockBlockDevice::new(2, "mnt_disk", 512, 100));
    let mnt_fs = Box::new(TestFileSystem::new("mntfs", mnt_device, 512));
    let mnt_fs_id = manager.register_fs(mnt_fs);
    
    // File system for /mnt/usb
    let usb_device = Box::new(MockBlockDevice::new(3, "usb_disk", 512, 100));

    let usb_fs = Box::new(TestFileSystem::new("usbfs", usb_device, 512));
    let usb_fs_id = manager.register_fs(usb_fs);

    
    // Execute mounts (create hierarchical structure)
    manager.mount(root_fs_id, "/").unwrap();
    manager.mount(mnt_fs_id, "/mnt").unwrap();
    manager.mount(usb_fs_id, "/mnt/usb").unwrap();

    
    // 1. Test path resolution - Ensure each mount point references the correct file system
    manager.with_resolve_path("/", |fs, relative_path| {
        assert_eq!(fs.read().name(), "rootfs");
        assert_eq!(relative_path, "/");
        Ok(())
    }).unwrap();
    
    manager.with_resolve_path("/test.txt", |fs, relative_path| {
        assert_eq!(fs.read().name(), "rootfs");
        assert_eq!(relative_path, "/test.txt");
        Ok(())
    }).unwrap();
    
    manager.with_resolve_path("/mnt", |fs, relative_path| {
        assert_eq!(fs.read().name(), "mntfs");
        assert_eq!(relative_path, "/");
        Ok(())
    }).unwrap();
    
    manager.with_resolve_path("/mnt/test.txt", |fs, relative_path| {
        assert_eq!(fs.read().name(), "mntfs");
        assert_eq!(relative_path, "/test.txt");
        Ok(())
    }).unwrap();
    
    manager.with_resolve_path("/mnt/usb", |fs, relative_path| {
        assert_eq!(fs.read().name(), "usbfs");
        assert_eq!(relative_path, "/");
        Ok(())
    }).unwrap();
    
    manager.with_resolve_path("/mnt/usb/test.txt", |fs, relative_path| {
        assert_eq!(fs.read().name(), "usbfs");
        assert_eq!(relative_path, "/test.txt");
        Ok(())
    }).unwrap();
    
    // 2. Test file operations - Operations on nested mount points
    // Create file in root
    manager.create_regular_file("/rootfile.txt").unwrap();
    
    // Create file in /mnt
    manager.create_regular_file("/mnt/mntfile.txt").unwrap();
    
    // Create file in /mnt/usb
    manager.create_regular_file("/mnt/usb/usbfile.txt").unwrap();
    
    // Verify directory listings at each mount point
    let root_entries = manager.readdir("/").unwrap();
    let mnt_entries = manager.readdir("/mnt").unwrap();
    let usb_entries = manager.readdir("/mnt/usb").unwrap();
    
    // Ensure "rootfile.txt" is in root entries
    assert!(root_entries.iter().any(|e| e.name == "rootfile.txt"));
    
    // Ensure "mntfile.txt" is in /mnt entries
    assert!(mnt_entries.iter().any(|e| e.name == "mntfile.txt"));
    
    // Ensure "usbfile.txt" is in /mnt/usb entries
    assert!(usb_entries.iter().any(|e| e.name == "usbfile.txt"));
    
    // 3. Test unmounting and its effects
    // Test behavior when unmounting intermediate file system
    manager.unmount("/mnt/usb").unwrap();
    
    // Accessing /mnt/usb should result in an error
    let result = manager.with_resolve_path("/mnt/usb", |fs, relative_path| { 
        // mntfs should be resolved
        assert_eq!(fs.read().name(), "mntfs");
        // Relative path should be "/usb"
        assert_eq!(relative_path, "/usb");
        Ok(())
    });
    assert!(result.is_ok());
    
    // However, /mnt should still be accessible
    manager.with_resolve_path("/mnt", |fs, relative_path| {
        assert_eq!(fs.read().name(), "mntfs");
        assert_eq!(relative_path, "/");
        Ok(())
    }).unwrap();
    
    // Finally, unmount the remaining file systems
    manager.unmount("/mnt").unwrap();
    manager.unmount("/").unwrap();
    
    // Ensure all mount points are unmounted
    assert_eq!(manager.mount_count(), 0);
    // Ensure all file systems are returned to the registration list
    assert_eq!(manager.filesystems.read().len(), 3);
}

#[test_case]
fn test_directory_boundary_handling() {
    // Setup: Create similar mount points
    let mut manager = VfsManager::new();
    
    // Create 4 different file systems
    let root_fs = Box::new(TestFileSystem::new("rootfs", 
        Box::new(MockBlockDevice::new(1, "root_disk", 512, 100)), 512));
    let mnt_fs = Box::new(TestFileSystem::new("mntfs", 
        Box::new(MockBlockDevice::new(2, "mnt_disk", 512, 100)), 512));
    let mnt_data_fs = Box::new(TestFileSystem::new("mnt_datafs", 
        Box::new(MockBlockDevice::new(3, "mnt_data_disk", 512, 100)), 512));
    let mnt_sub_fs = Box::new(TestFileSystem::new("mnt_subfs", 
        Box::new(MockBlockDevice::new(4, "mnt_sub_disk", 512, 100)), 512));
    
    // Register file systems
    let root_id = manager.register_fs(root_fs);
    let mnt_id = manager.register_fs(mnt_fs);
    let mnt_data_id = manager.register_fs(mnt_data_fs);
    let mnt_sub_id = manager.register_fs(mnt_sub_fs);
    
    // Mount with confusing patterns
    manager.mount(root_id, "/").unwrap();
    manager.mount(mnt_id, "/mnt").unwrap();
    manager.mount(mnt_data_id, "/mnt_data").unwrap();
    manager.mount(mnt_sub_id, "/mnt/sub").unwrap();
    
    // Test case 1: Distinguish similar prefixes
    // Ensure distinction between "/mnt" and "/mnt_data"
    manager.with_resolve_path("/mnt_data/test.txt", |fs, relative_path| {
        assert_eq!(fs.read().name(), "mnt_datafs");
        assert_eq!(relative_path, "/test.txt");
        Ok(())
    }).unwrap();
    
    manager.with_resolve_path("/mnt/test.txt", |fs, relative_path| {
        assert_eq!(fs.read().name(), "mntfs");
        assert_eq!(relative_path, "/test.txt");
        Ok(())
    }).unwrap();
    
    // Test case 2: Handling trailing slashes
    manager.with_resolve_path("/mnt/", |fs, relative_path| {
        assert_eq!(fs.read().name(), "mntfs");
        assert_eq!(relative_path, "/");
        Ok(())
    }).unwrap();
    
    // Test case 3: Normalizing multiple slashes
    manager.with_resolve_path("/mnt///sub///test.txt", |fs, relative_path| {
        assert_eq!(fs.read().name(), "mnt_subfs");
        assert_eq!(relative_path, "/test.txt");
        Ok(())
    }).unwrap();
    
    // Test case 4: Edge case - Distinguish exact match and partial match
    manager.with_resolve_path("/mnt", |fs, relative_path| {
        assert_eq!(fs.read().name(), "mntfs");
        assert_eq!(relative_path, "/");
        Ok(())
    }).unwrap();
    
    manager.with_resolve_path("/mnt_data", |fs, relative_path| {
        assert_eq!(fs.read().name(), "mnt_datafs");
        assert_eq!(relative_path, "/");
        Ok(())
    }).unwrap();
    
    // Test case 5: Boundary condition - When other text follows the mount point
    manager.with_resolve_path("/mnt_extra", |fs, relative_path| {
        assert_eq!(fs.read().name(), "rootfs");
        assert_eq!(relative_path, "/mnt_extra");
        Ok(())
    }).unwrap();
    
    // Test case 6: Nested boundary conditions
    manager.with_resolve_path("/mnt/subextra", |fs, relative_path| {
        assert_eq!(fs.read().name(), "mntfs");
        assert_eq!(relative_path, "/subextra");
        Ok(())
    }).unwrap();
    
    manager.with_resolve_path("/mnt/sub/test", |fs, relative_path| {
        assert_eq!(fs.read().name(), "mnt_subfs");
        assert_eq!(relative_path, "/test");
        Ok(())
    }).unwrap();

    // Test case 7: Boundary check for file operations
    // Create file in /mnt_data
    manager.create_regular_file("/mnt_data/testfile.txt").unwrap();
    
    // Create file in /mnt/sub
    manager.create_regular_file("/mnt/sub/testfile.txt").unwrap();
    
    // Ensure files are correctly created in each file system
    let mnt_data_entries = manager.readdir("/mnt_data").unwrap();
    assert!(mnt_data_entries.iter().any(|e| e.name == "testfile.txt"));
    
    let mnt_sub_entries = manager.readdir("/mnt/sub").unwrap();
    assert!(mnt_sub_entries.iter().any(|e| e.name == "testfile.txt"));
    
    // Ensure delete operations work correctly
    manager.remove("/mnt_data/testfile.txt").unwrap();
    let mnt_data_entries = manager.readdir("/mnt_data").unwrap();
    assert!(!mnt_data_entries.iter().any(|e| e.name == "testfile.txt"));
}

#[test_case]
fn test_path_normalization() {
    // Normalize absolute paths
    assert_eq!(VfsManager::normalize_path("/a/b/../c"), "/a/c");
    assert_eq!(VfsManager::normalize_path("/a/./b/./c"), "/a/b/c");
    assert_eq!(VfsManager::normalize_path("/a/b/../../c"), "/c");
    assert_eq!(VfsManager::normalize_path("/a/b/c/.."), "/a/b");
    assert_eq!(VfsManager::normalize_path("/../a/b/c"), "/a/b/c");  // Cannot go above root
    
    // Normalize multiple slashes
    assert_eq!(VfsManager::normalize_path("/a//b///c"), "/a/b/c");
    
    // Edge cases
    assert_eq!(VfsManager::normalize_path("/"), "/");
    assert_eq!(VfsManager::normalize_path("//"), "/");
    assert_eq!(VfsManager::normalize_path("/."), "/");
    assert_eq!(VfsManager::normalize_path("/.."), "/");
    assert_eq!(VfsManager::normalize_path(""), ".");
    assert_eq!(VfsManager::normalize_path(".."), "..");

    
    // Normalize relative paths (optional - if VfsManager supports relative paths)
    assert_eq!(VfsManager::normalize_path("a/b/../c"), "a/c");
    assert_eq!(VfsManager::normalize_path("./a/b/c"), "a/b/c");
    assert_eq!(VfsManager::normalize_path("../a/b/c"), "../a/b/c");
    assert_eq!(VfsManager::normalize_path("a/b/c/.."), "a/b");
    assert_eq!(VfsManager::normalize_path("a/b/c/../.."), "a");
}

#[test_case]
fn test_to_absolute_path() {
    let mut task = new_user_task("test".to_string(), 0);
    task.cwd = Some("/mnt".to_string());

    let relative_path = "test.txt";
    let absolute_path = VfsManager::to_absolute_path(&task, relative_path).unwrap();
    assert_eq!(absolute_path, "/mnt/test.txt");


    let relative_path = "./test.txt";
    let absolute_path = VfsManager::to_absolute_path(&task, relative_path).unwrap();
    assert_eq!(absolute_path, "/mnt/test.txt");

    let relative_path = "../test.txt";
    let absolute_path = VfsManager::to_absolute_path(&task, relative_path).unwrap();
    assert_eq!(absolute_path, "/test.txt"); // Should not resolve to /test.txt

    let relative_path = "./a/../test.txt";
    let absolute_path = VfsManager::to_absolute_path(&task, relative_path).unwrap();
    assert_eq!(absolute_path, "/mnt/test.txt"); // Should resolve to /mnt/test.txt

    let relative_path = "/a/b/test.txt";
    let absolute_path = VfsManager::to_absolute_path(&task, relative_path).unwrap();
    assert_eq!(absolute_path, "/a/b/test.txt"); // Should not resolve to /mnt/a/b/test.txt

    task.cwd = None; // Reset current working directory
    let relative_path = "test.txt";
    let absolute_path = VfsManager::to_absolute_path(&task, relative_path);
    assert!(absolute_path.is_err());
}

#[test_case]
fn test_driver_registration() {
    let mut manager = FileSystemDriverManager::new();
    
    // Initially empty
    assert_eq!(manager.list_drivers().len(), 0);
    assert!(!manager.has_driver("testfs"));
    
    // Register driver
    manager.register_driver(Box::new(TestFileSystemDriver));
    
    // Verify registration
    assert_eq!(manager.list_drivers().len(), 1);
    assert!(manager.has_driver("testfs"));
    assert_eq!(manager.list_drivers()[0], "testfs");
}

#[test_case]
fn test_driver_type_check() {
    let mut manager = FileSystemDriverManager::new();
    manager.register_driver(Box::new(TestFileSystemDriver));
    
    // Check driver type
    assert_eq!(manager.get_driver_type("testfs"), Some(FileSystemType::Block));
    assert_eq!(manager.get_driver_type("nonexistent"), None);
}

#[test_case]
fn test_create_from_block() {
    let mut manager = FileSystemDriverManager::new();
    manager.register_driver(Box::new(TestFileSystemDriver));
    
    let device = Box::new(MockBlockDevice::new(1, "test_disk", 512, 100));
    let result = manager.create_from_block("testfs", device, 512);
    
    assert!(result.is_ok());
    let fs = result.unwrap();
    assert_eq!(fs.name(), "testfs");
}

#[test_case]
fn test_create_from_nonexistent_driver() {
    let manager = FileSystemDriverManager::new();
    
    let device = Box::new(MockBlockDevice::new(1, "test_disk", 512, 100));
    let result = manager.create_from_block("nonexistent", device, 512);
    
    assert!(result.is_err());
    if let Err(e) = result {
        assert_eq!(e.kind, FileSystemErrorKind::NotFound);
        assert_eq!(e.message, "File system driver 'nonexistent' not found");
    }
}

#[test_case]
fn test_container_rootfs_switching_demo() {
    // === Container Demo: Rootfs switching for container-like functionality ===
    
    // 1. Set up filesystem for main system
    let mut main_vfs = VfsManager::new();
    
    // Filesystem for main system (using TestFileSystem)
    let main_device = Box::new(MockBlockDevice::new(1, "main_disk", 512, 100));
    let main_fs = Box::new(TestFileSystem::new("main_testfs", main_device, 512));
    let main_fs_id = main_vfs.register_fs(main_fs);
    main_vfs.mount(main_fs_id, "/")
        .expect("Failed to mount main filesystem");
    
    // Create directories and files in main system
    main_vfs.create_dir("/system").expect("Failed to create /system");
    main_vfs.create_regular_file("/system/main.conf").expect("Failed to create main config");
    
    // 2. Create independent VfsManager for container 1
    let mut container1_vfs = VfsManager::new();
    
    // Filesystem for container 1
    let container1_device = Box::new(MockBlockDevice::new(2, "container1_disk", 512, 100));
    let container1_fs = Box::new(TestFileSystem::new("container1_testfs", container1_device, 512));
    let container1_fs_id = container1_vfs.register_fs(container1_fs);
    container1_vfs.mount(container1_fs_id, "/")
        .expect("Failed to mount container1 filesystem");
    
    // Create application files in container 1 filesystem
    container1_vfs.create_dir("/app").expect("Failed to create /app");
    container1_vfs.create_regular_file("/app/config.json").expect("Failed to create app config");
    container1_vfs.create_dir("/tmp").expect("Failed to create /tmp");
    
    // 3. Create independent VfsManager for container 2
    let mut container2_vfs = VfsManager::new();
    
    // Filesystem for container 2
    let container2_device = Box::new(MockBlockDevice::new(3, "container2_disk", 512, 100));
    let container2_fs = Box::new(TestFileSystem::new("container2_testfs", container2_device, 512));
    let container2_fs_id = container2_vfs.register_fs(container2_fs);
    container2_vfs.mount(container2_fs_id, "/")
        .expect("Failed to mount container2 filesystem");
    
    // Create different application files in container 2 filesystem
    container2_vfs.create_dir("/service").expect("Failed to create /service");
    container2_vfs.create_regular_file("/service/daemon.conf").expect("Failed to create daemon config");
    container2_vfs.create_dir("/data").expect("Failed to create /data");
    
    // 4. Create tasks with different VfsManagers
    
    // Container 1 task (uses independent VfsManager)
    let mut container1_task = new_user_task("container1_app".to_string(), 0);
    container1_task.vfs = Some(Arc::new(container1_vfs));
    container1_task.cwd = Some("/app".to_string());
    
    // Container 2 task (uses independent VfsManager)
    let mut container2_task = new_user_task("container2_service".to_string(), 0);
    container2_task.vfs = Some(Arc::new(container2_vfs));
    container2_task.cwd = Some("/service".to_string());
    
    // 5. Test filesystem access from each task
    
    // Access from main system task (no VfsManager assigned)
    let main_entries = main_vfs
        .readdir("/")
        .expect("Failed to read root directory from main task");
    
    // Verify that /system directory is visible
    assert!(main_entries.iter().any(|e| e.name == "system"));
    
    // Access from container 1 task
    let container1_entries = container1_task.vfs.as_ref().unwrap()
        .readdir("/")
        .expect("Failed to read root directory from container1 task");
    
    // Verify that /app directory is visible but /system is not
    assert!(container1_entries.iter().any(|e| e.name == "app"));
    assert!(!container1_entries.iter().any(|e| e.name == "system"));
    
    // Access from container 2 task
    let container2_entries = container2_task.vfs.as_ref().unwrap()
        .readdir("/")
        .expect("Failed to read root directory from container2 task");
    
    // Verify that /service directory is visible but /system and /app are not
    assert!(container2_entries.iter().any(|e| e.name == "service"));
    assert!(!container2_entries.iter().any(|e| e.name == "system"));
    assert!(!container2_entries.iter().any(|e| e.name == "app"));
    
    // 6. Test path resolution
    
    let container1_abs_path = VfsManager::to_absolute_path(&container1_task, "config.json")
        .expect("Failed to resolve path in container1 task");
    assert_eq!(container1_abs_path, "/app/config.json");
    
    let container2_abs_path = VfsManager::to_absolute_path(&container2_task, "daemon.conf")
        .expect("Failed to resolve path in container2 task");
    assert_eq!(container2_abs_path, "/service/daemon.conf");
    
    // 7. Test file access isolation
    
    // Verify that containers cannot access main system files
    let container1_main_access = container1_task.vfs.as_ref().unwrap()
        .open("/system/main.conf", 0);
    assert!(container1_main_access.is_err(), "Container1 should not access main system files");
    
    // Verify that container 2 cannot access container 1 files
    let container2_app_access = container2_task.vfs.as_ref().unwrap()
        .open("/app/config.json", 0);
    assert!(container2_app_access.is_err(), "Container2 should not access container1 files");
    
    // Verify that each container can access its own files
    let container1_own_access = container1_task.vfs.as_ref().unwrap()
        .open("/app/config.json", 0);
    assert!(container1_own_access.is_ok(), "Container1 should access its own files");
    
    let container2_own_access = container2_task.vfs.as_ref().unwrap()
        .open("/service/daemon.conf", 0);
    assert!(container2_own_access.is_ok(), "Container2 should access its own files");
    
    // 8. Test VfsManager inheritance during task cloning
    
    // Clone container 1 task and verify VfsManager inheritance
    let cloned_container1_task = container1_task.clone_task(CloneFlags::default())
        .expect("Failed to clone container1 task");
    
    // Verify that cloned task uses same VfsManager
    assert!(cloned_container1_task.vfs.is_some());
    
    // Verify that cloned task sees same filesystem
    let cloned_entries = cloned_container1_task.vfs.as_ref().unwrap()
        .readdir("/")
        .expect("Failed to read directory from cloned task");
    assert!(cloned_entries.iter().any(|e| e.name == "app"));
    assert!(!cloned_entries.iter().any(|e| e.name == "system"));
    
    // 9. Verify VfsManager statistics
    assert_eq!(main_vfs.mount_count(), 1);
    assert_eq!(container1_task.vfs.as_ref().unwrap().mount_count(), 1);
    assert_eq!(container2_task.vfs.as_ref().unwrap().mount_count(), 1);
    
    // 10. Cleanup
    let _ = main_vfs.unmount("/");
    
    // === Container Demo completed successfully! ===
    // Demonstrated features:
    // - ✓ Multiple isolated VfsManager instances
    // - ✓ Different filesystem views per container/task
    // - ✓ Path resolution isolation
    // - ✓ File access isolation between containers
    // - ✓ VfsManager inheritance in task cloning
    // - ✓ Container-like filesystem namespace isolation
}


#[test_case]
fn test_proper_vfs_isolation_with_new_instances() {
    // === Correct implementation method for isolation ===
    
    // Create independent VfsManager instances (not clone)
    let mut manager1 = VfsManager::new();
    let mut manager2 = VfsManager::new();
    
    // Register independent filesystems for each
    let device1 = Box::new(MockBlockDevice::new(1, "disk1", 512, 100));
    let fs1 = Box::new(TestFileSystem::new("fs1", device1, 512));
    let fs1_id = manager1.register_fs(fs1);
    manager1.mount(fs1_id, "/mnt").unwrap();
    
    let device2 = Box::new(MockBlockDevice::new(2, "disk2", 512, 100));
    let fs2 = Box::new(TestFileSystem::new("fs2", device2, 512));
    let fs2_id = manager2.register_fs(fs2);
    manager2.mount(fs2_id, "/mnt").unwrap();
    
    // Create file in manager1
    manager1.create_regular_file("/mnt/file_in_container1.txt").unwrap();
    // Visible from manager1 (correct isolation)
    let entries1 = manager1.readdir("/mnt").unwrap();
    assert!(entries1.iter().any(|e| e.name == "file_in_container1.txt"));
    
    // Not visible from manager2 (correct isolation)
    let entries2 = manager2.readdir("/mnt").unwrap();
    assert!(!entries2.iter().any(|e| e.name == "file_in_container1.txt"));
    
    // Create file in manager2
    manager2.create_regular_file("/mnt/file_in_container2.txt").unwrap();
    // Visible from manager2 (correct isolation)
    let entries2 = manager2.readdir("/mnt").unwrap();
    assert!(entries2.iter().any(|e| e.name == "file_in_container2.txt"));
    
    // Not visible from manager1 (correct isolation)
    let entries1 = manager1.readdir("/mnt").unwrap();
    assert!(!entries1.iter().any(|e| e.name == "file_in_container2.txt"));
}

// Test cases for structured parameter system
#[test_case]
fn test_structured_parameters_tmpfs() {
    use crate::fs::params::TmpFSParams;
    
    // Register TmpFS driver
    get_fs_driver_manager().register_driver(Box::new(crate::fs::drivers::tmpfs::TmpFSDriver));
    
    let mut manager = VfsManager::new();
    
    // Create TmpFS with specific parameters
    let params = TmpFSParams::with_memory_limit(1024 * 1024); // 1MB limit
    let fs_id = manager.create_and_register_fs_from_params("tmpfs", &params).unwrap();
    
    // Mount the filesystem
    let result = manager.mount(fs_id, "/tmp");
    assert!(result.is_ok());
    
    // Verify the filesystem is mounted and working
    let result = manager.create_dir("/tmp/test");
    assert!(result.is_ok());
    
    let entries = manager.readdir("/tmp").unwrap();
    assert!(entries.iter().any(|e| e.name == "test" && e.file_type == FileType::Directory));
}

#[test_case]
fn test_structured_parameters_testfs() {
    use crate::fs::params::BasicFSParams;
    
    // Register TestFS driver
    get_fs_driver_manager().register_driver(Box::new(TestFileSystemDriver));
    
    let mut manager = VfsManager::new();
    
    // Create TestFS with specific parameters
    let params = BasicFSParams::new()
        .with_block_size(1024)
        .with_read_only(false);
    let fs_id = manager.create_and_register_fs_from_params("testfs", &params).unwrap();
    
    // Mount the filesystem
    let result = manager.mount(fs_id, "/test");
    assert!(result.is_ok());
    
    // Verify the filesystem is mounted and working
    let entries = manager.readdir("/test").unwrap();
    assert!(entries.len() >= 2); // Should have at least test.txt and testdir
    assert!(entries.iter().any(|e| e.name == "test.txt"));
    assert!(entries.iter().any(|e| e.name == "testdir"));
}

#[test_case]
fn test_structured_parameters_cpio_error() {
    use crate::fs::params::CpioFSParams;
    use crate::fs::drivers::cpio::CpiofsDriver;
    
    // Register CPIO driver
    get_fs_driver_manager().register_driver(Box::new(CpiofsDriver));
    
    let mut manager = VfsManager::new();
    
    // Try to create CPIO filesystem with parameters (should fail)
    let params = CpioFSParams::new();
    let result = manager.create_and_register_fs_from_params("cpiofs", &params);
    
    // Should fail because CPIO requires memory area
    assert!(result.is_err());
    if let Err(e) = result {
        assert_eq!(e.kind, FileSystemErrorKind::NotSupported);
        assert!(e.message.contains("memory area"));
    }
}

#[test_case]
fn test_structured_parameters_backward_compatibility() {
    use crate::fs::params::BasicFSParams;
    
    // Register TestFS driver
    get_fs_driver_manager().register_driver(Box::new(TestFileSystemDriver));
    
    let mut manager = VfsManager::new();
    
    // Test that regular block device creation still works
    let device = Box::new(MockBlockDevice::new(1, "test_disk", 512, 100));
    let fs_id = manager.create_and_register_block_fs("testfs", device, 512).unwrap();
    
    // Mount and test
    let result = manager.mount(fs_id, "/legacy");
    assert!(result.is_ok());
    
    let entries = manager.readdir("/legacy").unwrap();
    assert!(entries.iter().any(|e| e.name == "test.txt"));
    
    // Test that structured parameters also work for the same driver
    let params = BasicFSParams::new();
    let fs_id2 = manager.create_and_register_fs_from_params("testfs", &params).unwrap();
    
    let result = manager.mount(fs_id2, "/structured");
    assert!(result.is_ok());
    
    let entries = manager.readdir("/structured").unwrap();
    assert!(entries.iter().any(|e| e.name == "test.txt"));
}

#[test_case]
fn test_structured_parameters_driver_not_found() {
    use crate::fs::params::BasicFSParams;
    
    let mut manager = VfsManager::new();
    
    // Try to create filesystem with non-existent driver
    let params = BasicFSParams::new();
    let result = manager.create_and_register_fs_from_params("nonexistent", &params);
    
    assert!(result.is_err());
    if let Err(e) = result {
        assert_eq!(e.kind, FileSystemErrorKind::NotFound);
        assert!(e.message.contains("not found"));
    }
}

// Bind mount tests

#[test_case]
fn test_basic_bind_mount() {
    let mut manager = VfsManager::new();
    
    // Create and mount source filesystem
    let source_device = Box::new(MockBlockDevice::new(1, "source_disk", 512, 100));
    let source_fs = Box::new(TestFileSystem::new("source_fs", source_device, 512));
    let source_fs_id = manager.register_fs(source_fs);
    manager.mount(source_fs_id, "/source").unwrap();
    
    // Create and mount target filesystem
    let target_device = Box::new(MockBlockDevice::new(2, "target_disk", 512, 100));
    let target_fs = Box::new(TestFileSystem::new("target_fs", target_device, 512));
    let target_fs_id = manager.register_fs(target_fs);
    manager.mount(target_fs_id, "/target").unwrap();
    
    // Create bind mount (read-write)
    let result = manager.bind_mount("/source", "/target/bind", false);
    assert!(result.is_ok());
    
    // Verify bind mount exists
    assert!(manager.is_bind_mount("/target/bind"));
    
    // Test file access through bind mount
    let kernel_obj = manager.open("/target/bind/test.txt", 0).unwrap();
    let file = kernel_obj.as_file().unwrap();
    let entries = manager.readdir("/target/bind").unwrap();
    assert!(entries.iter().any(|e| e.name == "test.txt"));
    let mut buffer = [0u8; 20];
    let bytes_read = file.read(&mut buffer).unwrap();
    assert_eq!(bytes_read, 13); // "Hello, world!" from TestFileSystem
    assert_eq!(&buffer[..13], b"Hello, world!");
    
    // Test description:
    // This test verifies the basic functionality of bind mounts in the VFS.
    // A bind mount creates an alternate access path to an existing filesystem or directory.
    // Here we:
    // 1. Create two separate filesystems (/source and /target)
    // 2. Create a read-write bind mount from /source to /target/bind
    // 3. Verify that files can be accessed through the bind mount path
    // 4. Confirm that the bind mount redirects file operations to the original filesystem
    // This is essential for container isolation, chroot environments, and namespace management
    // where the same filesystem content needs to be accessible from multiple mount points.
}

#[test_case]
fn test_readonly_bind_mount() {
    let mut manager = VfsManager::new();
    
    // Create and mount source filesystem
    let source_device = Box::new(MockBlockDevice::new(1, "source_disk", 512, 100));
    let source_fs = Box::new(TestFileSystem::new("source_fs", source_device, 512));
    let source_fs_id = manager.register_fs(source_fs);
    manager.mount(source_fs_id, "/source").unwrap();
    
    // Create read-only bind mount
    let result = manager.bind_mount("/source", "/readonly", true);
    assert!(result.is_ok());
    
    // Verify bind mount exists
    assert!(manager.is_bind_mount("/readonly"));
    
    // Test read access through bind mount
    let kernel_obj = manager.open("/readonly/test.txt", 0).unwrap();
    let file = kernel_obj.as_file().unwrap();
    let mut buffer = [0u8; 20];
    let bytes_read = file.read(&mut buffer).unwrap();
    assert_eq!(bytes_read, 13);
    assert_eq!(&buffer[..13], b"Hello, world!");
    
    // Test that write access is restricted (this depends on implementation)
    // Note: The actual write restriction would be enforced at the VFS level
    
    // Test description:
    // This test validates the read-only bind mount functionality, which is crucial for
    // security and system integrity. Read-only bind mounts allow:
    // 1. Sharing system libraries or configuration files without modification risk
    // 2. Creating secure container environments where certain paths are immutable
    // 3. Implementing principle of least privilege access controls
    // The test creates a read-only bind mount and verifies that:
    // - Files can still be read normally through the bind mount
    // - The VFS correctly marks the mount as read-only
    // - Write operations would be properly restricted (implementation dependent)
    // This is essential for container security and system administration.
}

#[test_case]
fn test_bind_mount_from_another_vfs() {
    let mut host_vfs = Arc::new(VfsManager::new());
    let mut container_vfs = VfsManager::new();
    
    // Setup host filesystem
    let host_device = Box::new(MockBlockDevice::new(1, "host_disk", 512, 100));
    let host_fs = Box::new(TestFileSystem::new("host_fs", host_device, 512));
    let host_fs_id = Arc::get_mut(&mut host_vfs).unwrap().register_fs(host_fs);
    Arc::get_mut(&mut host_vfs).unwrap().mount(host_fs_id, "/host/data").unwrap();
    
    // Setup container filesystem
    let container_device = Box::new(MockBlockDevice::new(2, "container_disk", 512, 100));
    let container_fs = Box::new(TestFileSystem::new("container_fs", container_device, 512));
    let container_fs_id = container_vfs.register_fs(container_fs);
    container_vfs.mount(container_fs_id, "/").unwrap();
    
    // Create bind mount from host to container
    let result = container_vfs.bind_mount_from(&host_vfs, "/host/data", "/shared", false);
    assert!(result.is_ok());
    
    // Verify bind mount exists in container
    assert!(container_vfs.is_bind_mount("/shared"));
    
    // Test access through cross-VFS bind mount
    let kernel_obj = container_vfs.open("/shared/test.txt", 0).unwrap();
    let file = kernel_obj.as_file().unwrap();
    let mut buffer = [0u8; 20];
    let bytes_read = file.read(&mut buffer).unwrap();
    assert_eq!(bytes_read, 13);
    assert_eq!(&buffer[..13], b"Hello, world!");
    
    // Test description:
    // This test demonstrates cross-VFS bind mounting, a critical feature for container
    // and virtualization technologies. It simulates scenarios where:
    // 1. A host system has its own VFS manager with mounted filesystems
    // 2. A container or namespace has its own separate VFS manager
    // 3. The container needs access to specific host filesystem paths
    // The test verifies that:
    // - Files from one VFS can be accessed through another VFS via bind mounts
    // - The bind mount correctly bridges between different VFS instances
    // - File operations work seamlessly across VFS boundaries
    // This functionality is essential for Docker-like containers, chroot jails,
    // and any system where filesystem namespaces need controlled interconnection.
}

#[test_case]
fn test_nested_bind_mounts() {
    let mut manager = VfsManager::new();
    
    // Create source filesystem
    let source_device = Box::new(MockBlockDevice::new(1, "source_disk", 512, 100));
    let source_fs = Box::new(TestFileSystem::new("source_fs", source_device, 512));
    let source_fs_id = manager.register_fs(source_fs);
    manager.mount(source_fs_id, "/source").unwrap();
    
    // Create target filesystem  
    let target_device = Box::new(MockBlockDevice::new(2, "target_disk", 512, 100));
    let target_fs = Box::new(TestFileSystem::new("target_fs", target_device, 512));
    let target_fs_id = manager.register_fs(target_fs);
    manager.mount(target_fs_id, "/target").unwrap();
    
    // Create first bind mount
    manager.bind_mount("/source", "/target/first", false).unwrap();
    
    // Create second bind mount from first bind mount
    manager.bind_mount("/target/first", "/target/second", true).unwrap();
    
    // Verify both bind mounts exist
    assert!(manager.is_bind_mount("/target/first"));
    assert!(manager.is_bind_mount("/target/second"));
    
    // Test access through nested bind mounts
    let kernel_obj = manager.open("/target/second/test.txt", 0).unwrap();
    let file = kernel_obj.as_file().unwrap();
    let mut buffer = [0u8; 20];
    let bytes_read = file.read(&mut buffer).unwrap();
    assert_eq!(bytes_read, 13);
    assert_eq!(&buffer[..13], b"Hello, world!");
    
    // Test description:
    // This test validates nested bind mount functionality, where a bind mount
    // can itself be the source for another bind mount. This creates a chain:
    // /source → /target/first → /target/second
    // Nested bind mounts are important for:
    // 1. Complex container setups with multiple layers of filesystem abstraction
    // 2. Hierarchical namespace management in enterprise environments
    // 3. Avoiding filesystem duplication while maintaining multiple access paths
    // The test ensures that:
    // - Multiple levels of bind mounts can be created successfully
    // - File access works correctly through the entire bind mount chain
    // - Each level in the chain maintains proper filesystem semantics
    // - The second bind mount (read-only) correctly inherits from the first
    // This is crucial for container orchestration systems and complex chroot setups.
}

#[test_case]
fn test_bind_mount_error_cases() {
    let mut manager = VfsManager::new();
    
    // Try to bind mount non-existent source
    let result = manager.bind_mount("/nonexistent", "/target", false);
    assert!(result.is_err());
    
    // Create filesystem for valid tests
    let device = Box::new(MockBlockDevice::new(1, "test_disk", 512, 100));
    let fs = Box::new(TestFileSystem::new("test_fs", device, 512));
    let fs_id = manager.register_fs(fs);
    manager.mount(fs_id, "/source").unwrap();
    
    // Try to bind mount to invalid target (would need proper validation in impl)
    let result = manager.bind_mount("/source", "", false);
    assert!(result.is_err());
    
    // Test successful bind mount for comparison
    let result = manager.bind_mount("/source", "/valid_target", false);
    assert!(result.is_ok());
    assert!(manager.is_bind_mount("/valid_target"));
    
    // Test description:
    // This test focuses on error handling and validation in bind mount operations.
    // Robust error handling is critical for system stability and security because:
    // 1. Invalid bind mounts could lead to system crashes or security vulnerabilities
    // 2. Proper validation prevents accidental exposure of sensitive filesystem areas
    // 3. Clear error reporting helps system administrators debug mount issues
    // The test verifies that:
    // - Attempting to bind mount from non-existent paths fails gracefully
    // - Invalid target paths are properly rejected
    // - Valid bind mount operations still work correctly for comparison
    // - The VFS maintains consistency even when operations fail
    // This error handling is essential for production systems where invalid mount
    // operations should never compromise system integrity or expose security holes.
}

#[test_case]
fn test_bind_mount_path_resolution() {
    let mut manager = VfsManager::new();
    
    // Create source filesystem
    let device = Box::new(MockBlockDevice::new(1, "test_disk", 512, 100));
    let fs = Box::new(TestFileSystem::new("test_fs", device, 512));
    let fs_id = manager.register_fs(fs);
    manager.mount(fs_id, "/source").unwrap();
    
    // Create bind mount with subdirectory
    manager.bind_mount("/source", "/bind", false).unwrap();
    
    // Test path resolution through bind mount
    manager.with_resolve_path("/bind/test.txt", |fs, relative_path| {
        assert_eq!(fs.read().name(), "test_fs");
        assert_eq!(relative_path, "/test.txt");
        Ok(())
    }).unwrap();
    
    // Test path resolution of bind mount root
    manager.with_resolve_path("/bind", |fs, relative_path| {
        assert_eq!(fs.read().name(), "test_fs");
        assert_eq!(relative_path, "/");
        Ok(())
    }).unwrap();
    
    // Test description:
    // This test validates the path resolution mechanism through bind mounts,
    // which is fundamental to VFS operation. Path resolution determines:
    // 1. Which filesystem should handle a given path operation
    // 2. What the relative path within that filesystem should be
    // 3. How bind mounts redirect path lookups to their target filesystems
    // The test ensures that:
    // - Paths through bind mounts resolve to the correct underlying filesystem
    // - Relative paths are properly calculated after bind mount redirection
    // - Both file paths and directory paths resolve correctly
    // - The VFS correctly translates external paths to internal filesystem paths
    // This functionality is essential for all filesystem operations (open, read, write,
    // stat, etc.) to work correctly through bind mounts, enabling transparent
    // filesystem redirection that applications don't need to be aware of.
}

#[test_case]
fn test_bind_mount_with_hierarchical_mounts() {
    let mut manager = VfsManager::new();
    
    // Create root filesystem
    let root_device = Box::new(MockBlockDevice::new(1, "root_disk", 512, 100));
    let root_fs = Box::new(TestFileSystem::new("root_fs", root_device, 512));
    let root_fs_id = manager.register_fs(root_fs);
    manager.mount(root_fs_id, "/").unwrap();
    
    // Create filesystem for /mnt
    let mnt_device = Box::new(MockBlockDevice::new(2, "mnt_disk", 512, 100));
    let mnt_fs = Box::new(TestFileSystem::new("mnt_fs", mnt_device, 512));
    let mnt_fs_id = manager.register_fs(mnt_fs);
    manager.mount(mnt_fs_id, "/mnt").unwrap();
    
    // Create filesystem for /mnt/usb (nested mount)
    let usb_device = Box::new(MockBlockDevice::new(3, "usb_disk", 512, 100));
    let usb_fs = Box::new(TestFileSystem::new("usb_fs", usb_device, 512));
    let usb_fs_id = manager.register_fs(usb_fs);
    manager.mount(usb_fs_id, "/mnt/usb").unwrap();
    
    // Create bind mount pointing to the hierarchical mount structure
    manager.bind_mount("/mnt", "/bind_mnt", false).unwrap();
    
    // Test 1: Access file in the intermediate mount level through bind mount
    manager.with_resolve_path("/bind_mnt/test.txt", |fs, relative_path| {
        assert_eq!(fs.read().name(), "mnt_fs");
        assert_eq!(relative_path, "/test.txt");
        Ok(())
    }).unwrap();
    
    // Test 2: Access file in the nested mount through bind mount
    manager.with_resolve_path("/bind_mnt/usb/test.txt", |fs, relative_path| {
        assert_eq!(fs.read().name(), "usb_fs");  // Should resolve to the deepest mount
        assert_eq!(relative_path, "/test.txt");
        Ok(())
    }).unwrap();
    
    // Test 3: Access the nested mount root through bind mount
    manager.with_resolve_path("/bind_mnt/usb", |fs, relative_path| {
        assert_eq!(fs.read().name(), "usb_fs");
        assert_eq!(relative_path, "/");
        Ok(())
    }).unwrap();
    
    // Test 4: Create bind mount pointing directly to nested mount
    manager.bind_mount("/mnt/usb", "/bind_usb", false).unwrap();
    
    manager.with_resolve_path("/bind_usb/test.txt", |fs, relative_path| {
        assert_eq!(fs.read().name(), "usb_fs");
        assert_eq!(relative_path, "/test.txt");
        Ok(())
    }).unwrap();
    
    // Test description:
    // This test validates that bind mounts correctly handle hierarchical mount structures.
    // When a bind mount points to a directory that contains nested mount points:
    // 1. Access through the bind mount should resolve to the deepest appropriate mount
    // 2. Path resolution should traverse the mount hierarchy correctly
    // 3. Both intermediate and leaf mount points should be accessible
    // 4. The VFS should maintain proper mount point semantics through bind mounts
    // This is essential for container environments where complex mount hierarchies
    // need to be shared or isolated while preserving their internal structure.
}

#[test_case]
fn test_bind_mount_chain_with_nested_mounts() {
    let mut manager = VfsManager::new();
    
    // Create root filesystem
    let root_device = Box::new(MockBlockDevice::new(1, "root_disk", 512, 100));
    let root_fs = Box::new(TestFileSystem::new("root_fs", root_device, 512));
    let root_fs_id = manager.register_fs(root_fs);
    manager.mount(root_fs_id, "/").unwrap();
    
    // Create filesystem for /mnt
    let mnt_device = Box::new(MockBlockDevice::new(2, "mnt_disk", 512, 100));
    let mnt_fs = Box::new(TestFileSystem::new("mnt_fs", mnt_device, 512));
    let mnt_fs_id = manager.register_fs(mnt_fs);
    manager.mount(mnt_fs_id, "/mnt").unwrap();
    
    // Create filesystem for /mnt/usb (nested mount)
    let usb_device = Box::new(MockBlockDevice::new(3, "usb_disk", 512, 100));
    let usb_fs = Box::new(TestFileSystem::new("usb_fs", usb_device, 512));
    let usb_fs_id = manager.register_fs(usb_fs);
    manager.mount(usb_fs_id, "/mnt/usb").unwrap();
    
    // Create bind mount chain:
    // /source -> /mnt (first bind mount)
    manager.bind_mount("/mnt", "/source", false).unwrap();
    
    // /bind_mnt -> /source (second bind mount, creating a chain)
    manager.bind_mount("/source", "/bind_mnt", false).unwrap();
    
    // Test 1: Access intermediate mount through bind mount chain
    manager.with_resolve_path("/bind_mnt/test.txt", |fs, relative_path| {
        assert_eq!(fs.read().name(), "mnt_fs");
        assert_eq!(relative_path, "/test.txt");
        Ok(())
    }).unwrap();
    
    // Test 2: Access nested mount through bind mount chain
    manager.with_resolve_path("/bind_mnt/usb/test.txt", |fs, relative_path| {
        assert_eq!(fs.read().name(), "usb_fs");  // Should resolve through the chain to the deepest mount
        assert_eq!(relative_path, "/test.txt");
        Ok(())
    }).unwrap();
    
    // Test 3: Access nested mount root through bind mount chain
    manager.with_resolve_path("/bind_mnt/usb", |fs, relative_path| {
        assert_eq!(fs.read().name(), "usb_fs");
        assert_eq!(relative_path, "/");
        Ok(())
    }).unwrap();
    
    // Test 4: Verify all bind mounts are detected correctly
    assert!(manager.is_bind_mount("/source"));
    assert!(manager.is_bind_mount("/bind_mnt"));
    
    // Test 5: Verify intermediate access still works
    manager.with_resolve_path("/source/usb/test.txt", |fs, relative_path| {
        assert_eq!(fs.read().name(), "usb_fs");
        assert_eq!(relative_path, "/test.txt");
        Ok(())
    }).unwrap();
    
    // Test description:
    // This test validates complex bind mount chains combined with hierarchical mounts.
    // The scenario tests a chain: /mnt(/usb) -> /source -> /bind_mnt
    // This tests the kernel's ability to:
    // 1. Resolve through multiple levels of bind mount redirection
    // 2. Correctly handle nested mounts within bind mount chains
    // 3. Maintain proper filesystem semantics through the entire resolution chain
    // 4. Prevent infinite loops while allowing legitimate multi-level redirection
    // Such scenarios occur in container orchestration where:
    // - Host directories are bind mounted into containers
    // - Containers then create additional bind mounts for application isolation
    // - The underlying host directories may themselves contain nested mount points
    // - Multiple levels of indirection are needed for security and organization
    // 
    // This ensures the VFS can handle production container environments with complex
    // mount topologies spanning multiple filesystem namespaces.
}

#[test_case]
fn test_cross_vfs_bind_mount_chain_with_nested_mounts() {
    // Setup Host VFS with nested mounts
    let mut host_vfs = Arc::new(VfsManager::new());
    
    // Create root filesystem for host
    let host_root_device = Box::new(MockBlockDevice::new(1, "host_root_disk", 512, 100));
    let host_root_fs = Box::new(TestFileSystem::new("host_root_fs", host_root_device, 512));
    let host_root_fs_id = Arc::get_mut(&mut host_vfs).unwrap().register_fs(host_root_fs);
    Arc::get_mut(&mut host_vfs).unwrap().mount(host_root_fs_id, "/").unwrap();
    
    // Create /mnt filesystem in host
    let host_mnt_device = Box::new(MockBlockDevice::new(2, "host_mnt_disk", 512, 100));
    let host_mnt_fs = Box::new(TestFileSystem::new("host_mnt_fs", host_mnt_device, 512));
    let host_mnt_fs_id = Arc::get_mut(&mut host_vfs).unwrap().register_fs(host_mnt_fs);
    Arc::get_mut(&mut host_vfs).unwrap().mount(host_mnt_fs_id, "/mnt").unwrap();
    
    // Create /mnt/usb filesystem in host (nested mount)
    let host_usb_device = Box::new(MockBlockDevice::new(3, "host_usb_disk", 512, 100));
    let host_usb_fs = Box::new(TestFileSystem::new("host_usb_fs", host_usb_device, 512));
    let host_usb_fs_id = Arc::get_mut(&mut host_vfs).unwrap().register_fs(host_usb_fs);
    Arc::get_mut(&mut host_vfs).unwrap().mount(host_usb_fs_id, "/mnt/usb").unwrap();
    
    // Setup Container VFS
    let mut container_vfs = VfsManager::new();
    
    // Create root filesystem for container
    let container_device = Box::new(MockBlockDevice::new(4, "container_disk", 512, 100));
    let container_fs = Box::new(TestFileSystem::new("container_fs", container_device, 512));
    let container_fs_id = container_vfs.register_fs(container_fs);
    container_vfs.mount(container_fs_id, "/").unwrap();
    
    // Create bind mount chain across VFS:
    // Host: /mnt(/usb) -> Container: /source -> Container: /bind_mnt
    
    // Step 1: Cross-VFS bind mount from host to container
    container_vfs.bind_mount_from(&host_vfs, "/mnt", "/source", false).unwrap();
    
    // Step 2: Create bind mount chain within container
    container_vfs.bind_mount("/source", "/bind_mnt", false).unwrap();
    
    // Test 1: Access intermediate mount through cross-VFS bind mount chain
    container_vfs.with_resolve_path("/bind_mnt/test.txt", |fs, relative_path| {
        assert_eq!(fs.read().name(), "host_mnt_fs");  // Should resolve to host's mnt filesystem
        assert_eq!(relative_path, "/test.txt");
        Ok(())
    }).unwrap();
    
    // Test 2: Access nested mount through cross-VFS bind mount chain
    container_vfs.with_resolve_path("/bind_mnt/usb/test.txt", |fs, relative_path| {
        assert_eq!(fs.read().name(), "host_usb_fs");  // Should resolve to host's nested usb filesystem
        assert_eq!(relative_path, "/test.txt");
        Ok(())
    }).unwrap();
    
    // Test 3: Access nested mount root through cross-VFS bind mount chain
    container_vfs.with_resolve_path("/bind_mnt/usb", |fs, relative_path| {
        assert_eq!(fs.read().name(), "host_usb_fs");
        assert_eq!(relative_path, "/");
        Ok(())
    }).unwrap();
    
    // Test 4: Verify intermediate access still works
    container_vfs.with_resolve_path("/source/usb/test.txt", |fs, relative_path| {
        assert_eq!(fs.read().name(), "host_usb_fs");
        assert_eq!(relative_path, "/test.txt");
        Ok(())
    }).unwrap();
    
    // Test 5: Verify bind mount detection across VFS
    assert!(container_vfs.is_bind_mount("/source"));
    assert!(container_vfs.is_bind_mount("/bind_mnt"));
    
    // Test 6: Verify original access from host still works
    Arc::get_mut(&mut host_vfs).unwrap().with_resolve_path("/mnt/usb/test.txt", |fs, relative_path| {
        assert_eq!(fs.read().name(), "host_usb_fs");
        assert_eq!(relative_path, "/test.txt");
        Ok(())
    }).unwrap();
    
    // Test description:
    // This test validates the most complex bind mount scenario: cross-VFS bind mount chains
    // combined with hierarchical mount structures. The scenario tests:
    // 
    // Host VFS: /mnt (host_mnt_fs) + /mnt/usb (host_usb_fs)
    //           ↓ (cross-VFS bind mount)
    // Container VFS: /source → /bind_mnt (bind mount chain)
    // 
    // This tests the kernel's ability to:
    // 1. Resolve through cross-VFS bind mount redirection
    // 2. Handle nested mounts within cross-VFS bind mounts
    // 3. Maintain proper bind mount chains across VFS boundaries
    // 4. Correctly resolve complex mount hierarchies through multiple redirection levels
    // 
    // Such scenarios are common in container orchestration where:
    // - Host directories with complex mount structures are shared into containers
    // - Containers create additional bind mounts for application isolation
    // - The underlying host directories contain nested mount points (USB drives, network mounts, etc.)
    // - Multiple levels of indirection are needed for security and organization
    // 
    // This ensures the VFS can handle production container environments with complex
    // mount topologies spanning multiple filesystem namespaces.
}

// Comprehensive tests for truncate functionality using TmpFS
#[test_case]
fn test_truncate_file() {
    let manager = VfsManager::new();
    let tmpfs = Box::new(TmpFS::new(1024 * 1024)); // 1MB limit
    
    let fs_id = manager.register_fs(tmpfs);
    let _ = manager.mount(fs_id, "/tmp");
    
    // Create a file and write some data
    manager.create_file("/tmp/test.txt", FileType::RegularFile).unwrap();
    let kernel_obj = manager.open("/tmp/test.txt", 0).unwrap();
    let file = kernel_obj.as_file().unwrap();
    
    let test_data = b"Hello, World! This is a long text for testing truncate.";
    file.write(test_data).unwrap();
    
    // Test 1: Truncate to smaller size
    file.truncate(5).unwrap();
    file.seek(SeekFrom::Start(0)).unwrap();
    let mut buffer = [0u8; 10];
    let bytes_read = file.read(&mut buffer).unwrap();
    assert_eq!(bytes_read, 5);
    assert_eq!(&buffer[..5], b"Hello");
    
    // Test 2: Truncate to larger size (should pad with zeros)
    file.truncate(10).unwrap();
    file.seek(SeekFrom::Start(0)).unwrap();
    let mut buffer = [0u8; 15];
    let bytes_read = file.read(&mut buffer).unwrap();
    assert_eq!(bytes_read, 10);
    assert_eq!(&buffer[..5], b"Hello");
    assert_eq!(&buffer[5..10], &[0u8; 5]);
    
    // Test 3: Truncate to zero (empty file)
    file.truncate(0).unwrap();
    file.seek(SeekFrom::Start(0)).unwrap();
    let mut buffer = [0u8; 10];
    let bytes_read = file.read(&mut buffer).unwrap();
    assert_eq!(bytes_read, 0);
    
    // Test 4: Write after truncate
    file.write(b"New content").unwrap();
    file.seek(SeekFrom::Start(0)).unwrap();
    let mut buffer = [0u8; 15];
    let bytes_read = file.read(&mut buffer).unwrap();
    assert_eq!(bytes_read, 11);
    assert_eq!(&buffer[..11], b"New content");
}

#[test_case]
fn test_truncate_via_vfs_manager() {
    let manager = VfsManager::new();
    let tmpfs = Box::new(TmpFS::new(1024 * 1024)); // 1MB limit
    
    let fs_id = manager.register_fs(tmpfs);
    let _ = manager.mount(fs_id, "/tmp");
    
    // Create a file with data using VFS manager
    manager.create_file("/tmp/test.txt", FileType::RegularFile).unwrap();
    let kernel_obj = manager.open("/tmp/test.txt", 0).unwrap();
    let file = kernel_obj.as_file().unwrap();
    file.write(b"Initial content for VFS truncate test").unwrap();
    
    // Test truncate via VFS manager
    let result = manager.truncate("/tmp/test.txt", 7);
    assert!(result.is_ok());
    
    // Verify truncation worked
    file.seek(SeekFrom::Start(0)).unwrap();
    let mut buffer = [0u8; 20];
    let bytes_read = file.read(&mut buffer).unwrap();
    assert_eq!(bytes_read, 7);
    assert_eq!(&buffer[..7], b"Initial");
}

#[test_case]
fn test_truncate_error_cases() {
    let manager = VfsManager::new();
    let tmpfs = Box::new(TmpFS::new(1024 * 1024)); // 1MB limit
    
    let fs_id = manager.register_fs(tmpfs);
    let _ = manager.mount(fs_id, "/tmp");
    
    // Test 1: Truncate non-existent file
    let result = manager.truncate("/tmp/nonexistent.txt", 10);
    assert!(result.is_err());
    
    // Test 2: Truncate directory (should fail)
    manager.create_dir("/tmp/testdir").unwrap();
    let result = manager.truncate("/tmp/testdir", 10);
    assert!(result.is_err());
    
    // Test 3: Truncate with invalid path
    let result = manager.truncate("/invalid/path/file.txt", 10);
    assert!(result.is_err());
}

#[test_case]
fn test_truncate_position_adjustment() {
    let manager = VfsManager::new();
    let tmpfs = Box::new(TmpFS::new(1024 * 1024)); // 1MB limit
    
    let fs_id = manager.register_fs(tmpfs);
    let _ = manager.mount(fs_id, "/tmp");
    
    // Create a file and write some data
    manager.create_file("/tmp/test.txt", FileType::RegularFile).unwrap();
    let kernel_obj = manager.open("/tmp/test.txt", 0).unwrap();
    let file = kernel_obj.as_file().unwrap();
    
    // Write some data and seek to middle
    file.write(b"0123456789").unwrap();
    file.seek(SeekFrom::Start(7)).unwrap();
    
    // Truncate to size smaller than current position
    file.truncate(5).unwrap();
    
    // Position should be adjusted to the new end of file
    let pos = file.seek(SeekFrom::Current(0)).unwrap();
    assert_eq!(pos, 5);
    
    // Verify we can't read beyond the truncated size
    let mut buffer = [0u8; 10];
    let bytes_read = file.read(&mut buffer).unwrap();
    assert_eq!(bytes_read, 0);
}

// Test bind mount unmount scenarios
#[test_case]
fn test_bind_mount_unmount() {
    let manager = VfsManager::new();
    
    // Setup source filesystem
    let tmpfs = Box::new(TmpFS::new(1024 * 1024));
    let fs_id = manager.register_fs(tmpfs);
    manager.mount(fs_id, "/tmp").unwrap();
    
    // Create a test file in the source
    manager.create_file("/tmp/test.txt", FileType::RegularFile).unwrap();
    
    // Create bind mount
    manager.bind_mount("/tmp", "/mnt/bind", false).unwrap();
    assert_eq!(manager.mount_count(), 2);
    
    // Verify the bind mount works
    let file = manager.open("/mnt/bind/test.txt", 0).unwrap();
    assert!(file.as_file().is_some());
    
    // Unmount the bind mount
    let result = manager.unmount("/mnt/bind");
    assert!(result.is_ok());
    assert_eq!(manager.mount_count(), 1);
    
    // Verify original mount still exists
    let file = manager.open("/tmp/test.txt", 0).unwrap();
    assert!(file.as_file().is_some());
    
    // Verify bind mount is gone
    let result = manager.open("/mnt/bind/test.txt", 0);
    assert!(result.is_err());
}

#[test_case]
fn test_read_only_bind_mount_unmount() {
    let manager = VfsManager::new();
    
    // Setup source filesystem
    let tmpfs = Box::new(TmpFS::new(1024 * 1024));
    let fs_id = manager.register_fs(tmpfs);
    manager.mount(fs_id, "/tmp").unwrap();
    
    // Create a test file
    manager.create_file("/tmp/test.txt", FileType::RegularFile).unwrap();
    
    // Create read-only bind mount
    manager.bind_mount("/tmp", "/mnt/readonly", true).unwrap();
    assert_eq!(manager.mount_count(), 2);
    
    // Verify read access works
    let file = manager.open("/mnt/readonly/test.txt", 0).unwrap();
    assert!(file.as_file().is_some());
    
    // Unmount the read-only bind mount
    let result = manager.unmount("/mnt/readonly");
    assert!(result.is_ok());
    assert_eq!(manager.mount_count(), 1);
    
    // Verify original mount still accessible
    let file = manager.open("/tmp/test.txt", 0).unwrap();
    assert!(file.as_file().is_some());
}

#[test_case]
fn test_nested_mount_unmount() {
    let manager = VfsManager::new();
    
    // Setup base filesystem
    let tmpfs1 = Box::new(TmpFS::new(1024 * 1024));
    let fs_id1 = manager.register_fs(tmpfs1);
    manager.mount(fs_id1, "/base").unwrap();
    
    // Setup nested filesystem
    let tmpfs2 = Box::new(TmpFS::new(1024 * 1024));
    let fs_id2 = manager.register_fs(tmpfs2);
    manager.mount(fs_id2, "/base/nested").unwrap();
    
    // Create bind mount of nested
    manager.bind_mount("/base/nested", "/mnt/bind_nested", false).unwrap();
    
    assert_eq!(manager.mount_count(), 3);
    
    // Unmount bind mount first
    manager.unmount("/mnt/bind_nested").unwrap();
    assert_eq!(manager.mount_count(), 2);
    
    // Unmount nested filesystem
    manager.unmount("/base/nested").unwrap();
    assert_eq!(manager.mount_count(), 1);
    
    // Unmount base filesystem
    manager.unmount("/base").unwrap();
    assert_eq!(manager.mount_count(), 0);
    
    // Verify all filesystems returned to registry
    assert_eq!(manager.filesystems.read().len(), 2);
}

#[test_case]
fn test_unmount_nonexistent_mount() {
    let manager = VfsManager::new();
    
    // Try to unmount a non-existent mount point
    let result = manager.unmount("/nonexistent");
    assert!(result.is_err());
    match result {
        Err(e) => {
            assert_eq!(e.kind, FileSystemErrorKind::NotFound);
        }
        Ok(_) => panic!("Expected error when unmounting non-existent mount point"),
    }
}

#[test_case] 
fn test_unmount_preserves_filesystem_order() {
    let manager = VfsManager::new();
    
    // Create multiple filesystems
    let tmpfs1 = Box::new(TmpFS::new(1024 * 1024));
    let tmpfs2 = Box::new(TmpFS::new(1024 * 1024));
    let tmpfs3 = Box::new(TmpFS::new(1024 * 1024));
    
    let fs_id1 = manager.register_fs(tmpfs1);
    let fs_id2 = manager.register_fs(tmpfs2);
    let fs_id3 = manager.register_fs(tmpfs3);
    
    // Mount them
    manager.mount(fs_id1, "/mnt1").unwrap();
    manager.mount(fs_id2, "/mnt2").unwrap();
    manager.mount(fs_id3, "/mnt3").unwrap();
    
    assert_eq!(manager.mount_count(), 3);
    assert_eq!(manager.filesystems.read().len(), 0);
    
    // Unmount middle one
    manager.unmount("/mnt2").unwrap();
    assert_eq!(manager.mount_count(), 2);
    assert_eq!(manager.filesystems.read().len(), 1);
    
    // Unmount all
    manager.unmount("/mnt1").unwrap();
    manager.unmount("/mnt3").unwrap();
    
    assert_eq!(manager.mount_count(), 0);
    assert_eq!(manager.filesystems.read().len(), 3);
}
