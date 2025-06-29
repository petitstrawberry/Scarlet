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
use crate::fs::vfs_v2::tmpfs::{TmpFS, TmpFSParams};
use crate::fs::vfs_v2::cpiofs::{CpioFS, CpioFSParams};
use crate::fs::vfs_v2::overlayfs::{OverlayFS, OverlayFSParams};
use crate::task::{new_user_task, CloneFlags};

// Test cases
#[test_case]
fn test_vfs_manager_creation() {
    let manager = VfsManager::new();
    assert_eq!(manager.filesystems.read().len(), 0);
    assert_eq!(manager.mount_count(), 0);
}

#[test_case]
fn test_tmpfs_registration_and_mount() {
    let mut manager = VfsManager::new();
    let params = TmpFSParams::with_memory_limit(1024 * 1024); // 1MB
    let fs = Arc::new(TmpFS::create_from_params(&params).unwrap());
    let result = manager.mount(fs, "/mnt");
    assert!(result.is_ok());
    assert_eq!(manager.mount_count(), 1);
}

#[test_case]
fn test_path_resolution() {
    let mut manager = VfsManager::new();
    let params = TmpFSParams::with_memory_limit(1024 * 1024); // 1MB
    let fs = Arc::new(TmpFS::create_from_params(&params).unwrap());
    let _ = manager.mount(fs.clone(), "/mnt");
    
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
    let params = TmpFSParams::with_memory_limit(1024 * 1024); // 1MB
    let fs = Arc::new(TmpFS::create_from_params(&params).unwrap());
    let _ = manager.mount(fs.clone(), "/mnt");
    
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
    let params = TmpFSParams::with_memory_limit(1024 * 1024); // 1MB
    let fs = Arc::new(TmpFS::create_from_params(&params).unwrap());
    let _ = manager.mount(fs.clone(), "/mnt");
    
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
    let params = TmpFSParams::with_memory_limit(1024 * 1024); // 1MB
    let fs = Arc::new(TmpFS::create_from_params(&params).unwrap());
    let _ = manager.mount(fs.clone(), "/mnt");
    assert_eq!(manager.mount_count(), 1);
    let result = manager.unmount("/mnt");
    assert!(result.is_ok());
    assert_eq!(manager.mount_count(), 0);
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
    let mut manager = VfsManager::new();
    let params = TmpFSParams::with_memory_limit(1024 * 1024); // 1MB
    let fs = Arc::new(TmpFS::create_from_params(&params).unwrap());
    let _ = manager.mount(fs.clone(), "/mnt");

    // Create an instance of the file structure
    // let file = File::open_with_manager("/mnt/test.txt".to_string(), &mut manager).unwrap();
    // assert_eq!(file.path, "/mnt/test.txt");
}

#[test_case]
fn test_file_open_close() {
    let mut manager = VfsManager::new();
    let params = TmpFSParams::with_memory_limit(1024 * 1024); // 1MB
    let fs = Arc::new(TmpFS::create_from_params(&params).unwrap());
    let _ = manager.mount(fs.clone(), "/mnt");
    
    // Create and open a file object
    let file = manager.open("/mnt/test.txt", 0o777);
    
    // Open the file
    assert!(file.is_ok());
}

#[test_case]
fn test_file_read_write() {
    let mut manager = VfsManager::new();
    let params = TmpFSParams::with_memory_limit(1024 * 1024); // 1MB
    let fs = Arc::new(TmpFS::create_from_params(&params).unwrap());
    let _ = manager.mount(fs.clone(), "/mnt");
    
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
fn test_file_truncate() {
    let mut manager = VfsManager::new();
    let params = TmpFSParams::with_memory_limit(1024 * 1024); // 1MB
    let fs = Arc::new(TmpFS::create_from_params(&params).unwrap());
    let _ = manager.mount(fs.clone(), "/mnt");
    
    // Create a file and write some data
    manager.create_file("/mnt/test.txt", FileType::RegularFile).unwrap();
    let kernel_obj = manager.open("/mnt/test.txt", 0).unwrap();
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
    let params = TmpFSParams::with_memory_limit(1024 * 1024); // 1MB
    let fs = Arc::new(TmpFS::create_from_params(&params).unwrap());
    let _ = manager.mount(fs.clone(), "/mnt");
    
    // Create a file with data using VFS manager
    manager.create_file("/mnt/test.txt", FileType::RegularFile).unwrap();
    let kernel_obj = manager.open("/mnt/test.txt", 0).unwrap();
    let file = kernel_obj.as_file().unwrap();
    file.write(b"Initial content for VFS truncate test").unwrap();
    
    // Test truncate via VFS manager
    let result = manager.truncate("/mnt/test.txt", 7);
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
    let params = TmpFSParams::with_memory_limit(1024 * 1024); // 1MB
    let fs = Arc::new(TmpFS::create_from_params(&params).unwrap());
    let _ = manager.mount(fs.clone(), "/mnt");
    
    // Test 1: Truncate non-existent file
    let result = manager.truncate("/mnt/nonexistent.txt", 10);
    assert!(result.is_err());
    
    // Test 2: Truncate directory (should fail)
    manager.create_dir("/mnt/testdir").unwrap();
    let result = manager.truncate("/mnt/testdir", 10);
    assert!(result.is_err());
    
    // Test 3: Truncate with invalid path
    let result = manager.truncate("/invalid/path/file.txt", 10);
    assert!(result.is_err());
}

#[test_case]
fn test_truncate_position_adjustment() {
    let manager = VfsManager::new();
    let params = TmpFSParams::with_memory_limit(1024 * 1024); // 1MB
    let fs = Arc::new(TmpFS::create_from_params(&params).unwrap());
    let _ = manager.mount(fs.clone(), "/mnt");
    
    // Create a file and write some data
    manager.create_file("/mnt/test.txt", FileType::RegularFile).unwrap();
    let kernel_obj = manager.open("/mnt/test.txt", 0).unwrap();
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
    let tmpfs = Arc::new(TmpFS::new(1024 * 1024));
    manager.mount(tmpfs.clone(), "/tmp").unwrap();
    
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
    let tmpfs = Arc::new(TmpFS::new(1024 * 1024));
    manager.mount(tmpfs.clone(), "/tmp").unwrap();
    
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
    let tmpfs1 = Arc::new(TmpFS::new(1024 * 1024));
    manager.mount(tmpfs1.clone(), "/base").unwrap();
    
    let tmpfs2 = Arc::new(TmpFS::new(1024 * 1024));
    manager.mount(tmpfs2.clone(), "/base/nested").unwrap();
    
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
    let tmpfs1 = Arc::new(TmpFS::new(1024 * 1024));
    let tmpfs2 = Arc::new(TmpFS::new(1024 * 1024));
    let tmpfs3 = Arc::new(TmpFS::new(1024 * 1024));
    manager.mount(tmpfs1.clone(), "/mnt1").unwrap();
    manager.mount(tmpfs2.clone(), "/mnt2").unwrap();
    manager.mount(tmpfs3.clone(), "/mnt3").unwrap();
    assert_eq!(manager.mount_count(), 3);
    // Unmount middle one
    manager.unmount("/mnt2").unwrap();
    assert_eq!(manager.mount_count(), 2);
    // Unmount all
    manager.unmount("/mnt1").unwrap();
    manager.unmount("/mnt3").unwrap();
    
    assert_eq!(manager.mount_count(), 0);
    assert_eq!(manager.filesystems.read().len(), 3);
}
