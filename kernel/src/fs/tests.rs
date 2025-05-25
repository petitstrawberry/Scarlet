use alloc::boxed::Box;
use super::*;
use crate::device::block::mockblk::MockBlockDevice;
use crate::fs::testfs::{TestFileSystem, TestFileSystemDriver};
use crate::task::new_user_task;
use alloc::sync::Arc;

// Test cases
#[test_case]
fn test_vfs_manager_creation() {
    let manager = VfsManager::new();
    assert_eq!(manager.filesystems.read().len(), 0);
    assert_eq!(manager.mount_points.read().len(), 0);
}

#[test_case]
fn test_fs_registration_and_mount() {
    let mut manager = VfsManager::new();
    let device = Box::new(MockBlockDevice::new(1, "test_disk", 512, 100));
    let fs = Box::new(TestFileSystem::new(0, "testfs", device, 512));
    
    let fs_id = manager.register_fs(fs); // Get fs_id
    assert_eq!(manager.filesystems.read().len(), 1);
    
    let result = manager.mount(fs_id, "/mnt"); // Use fs_id
    assert!(result.is_ok());
    assert_eq!(manager.filesystems.read().len(), 0);
    assert_eq!(manager.mount_points.read().len(), 1);
}

#[test_case]
fn test_path_resolution() {
    let mut manager = VfsManager::new();
    let device = Box::new(MockBlockDevice::new(1, "test_disk", 512, 100));
    let fs = Box::new(TestFileSystem::new(0, "testfs", device, 512));
    
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
    let fs = Box::new(TestFileSystem::new(0, "testfs", device, 512));
    
    let fs_id = manager.register_fs(fs); // Get fs_id
    let _ = manager.mount(fs_id, "/mnt"); // Use fs_id
    
    // Open file
    let mut file = manager.open("/mnt/test.txt", 0).unwrap();
    
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
    let fs = Box::new(TestFileSystem::new(0, "testfs", device, 512));
    
    let fs_id = manager.register_fs(fs); // Get fs_id
    let _ = manager.mount(fs_id, "/mnt"); // Use fs_id
    
    // Get directory entries
    let entries = manager.read_dir("/mnt").unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].name, "test.txt");
    assert_eq!(entries[1].name, "testdir");
    assert_eq!(entries[0].file_type, FileType::RegularFile);
    assert_eq!(entries[1].file_type, FileType::Directory);
    
    // Create directory
    let result = manager.create_dir("/mnt/newdir");
    assert!(result.is_ok());
    
    // Verify
    let entries_after = manager.read_dir("/mnt").unwrap();
    assert_eq!(entries_after.len(), 3);
    assert!(entries_after.iter().any(|e| e.name == "newdir" && e.file_type == FileType::Directory));
    
    // Create file
    let result = manager.create_file("/mnt/newdir/newfile.txt");
    assert!(result.is_ok());
    
    // Verify
    let dir_entries = manager.read_dir("/mnt/newdir").unwrap();
    assert_eq!(dir_entries.len(), 1);
    assert_eq!(dir_entries[0].name, "newfile.txt");
    
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
    let fs = GenericFileSystem::new(0, "generic", Box::new(device), 512);
    
    // Prepare test data
    let test_data = [0xAA; 512];
    let mut read_buffer = [0; 512];
    
    // Write test
    let write_result = fs.write_block_internal(0, &test_data);
    assert!(write_result.is_ok());
    
    // Read test
    let read_result = fs.read_block_internal(0, &mut read_buffer);
    assert!(read_result.is_ok());
    
    // Verify data match
    assert_eq!(test_data, read_buffer);
}

#[test_case]
fn test_unmount() {
    let mut manager = VfsManager::new();
    let device = Box::new(MockBlockDevice::new(1, "test_disk", 512, 100));
    let fs = Box::new(TestFileSystem::new(0, "testfs", device, 512));
    
    let fs_id = manager.register_fs(fs); // Get fs_id
    let _ = manager.mount(fs_id, "/mnt"); // Use fs_id
    assert_eq!(manager.mount_points.read().len(), 1);
    
    // Unmount
    let result = manager.unmount("/mnt");
    assert!(result.is_ok());
    assert_eq!(manager.mount_points.read().len(), 0);
    assert_eq!(manager.filesystems.read().len(), 1); // The file system should be returned
    
    // Attempt to unmount an invalid mount point
    let result = manager.unmount("/invalid");
    match result {
        Ok(_) => panic!("Expected an error, but got Ok"),
        Err(e) => {
            assert_eq!(e.kind, FileSystemErrorKind::NotFound);
            assert_eq!(e.message, "Mount point not found".to_string());
        }
    }
}

// Test file structure

#[test_case]
fn test_file_creation() {
    // Setup
    let mut manager = VfsManager::new();
    let device = Box::new(MockBlockDevice::new(1, "test_disk", 512, 100));
    let fs = Box::new(TestFileSystem::new(0, "testfs", device, 512));
    
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
    let fs = Box::new(TestFileSystem::new(0, "testfs", device, 512));
    
    let fs_id = manager.register_fs(fs); // Get fs_id
    let _ = manager.mount(fs_id, "/mnt"); // Use fs_id
    
    // Create and open a file object
    let file = File::open_with_manager("/mnt/test.txt".to_string(), &mut manager);
    
    // Open the file
    assert!(file.is_ok());
}

#[test_case]
fn test_file_read_write() {
    // Setup
    let mut manager = VfsManager::new();
    let device = Box::new(MockBlockDevice::new(1, "test_disk", 512, 100));
    let fs = Box::new(TestFileSystem::new(0, "testfs", device, 512));
    
    let fs_id = manager.register_fs(fs); // Get fs_id
    let _ = manager.mount(fs_id, "/mnt"); // Use fs_id
    
    let mut file = File::open_with_manager("/mnt/test.txt".to_string(), &mut manager).unwrap();

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
    let fs = Box::new(TestFileSystem::new(0, "testfs", device, 512));
    
    let fs_id = manager.register_fs(fs); // Get fs_id
    let _ = manager.mount(fs_id, "/mnt"); // Use fs_id
    
    let mut file = File::open_with_manager("/mnt/test.txt".to_string(), &mut manager).unwrap();
    
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
    let fs = Box::new(TestFileSystem::new(0, "testfs", device, 512));
    
    let fs_id = manager.register_fs(fs); // Get fs_id
    let _ = manager.mount(fs_id, "/mnt"); // Use fs_id
    
    let mut file = File::open_with_manager("/mnt/test.txt".to_string(), &mut manager).unwrap();
    
    // Get metadata (possible even when not open)
    let metadata = file.metadata().unwrap();
    assert_eq!(metadata.file_type, FileType::RegularFile);

    // Write
    file.write(b"Hello, world!").unwrap();
    
    // Get size
    let size = file.size().unwrap();
    assert_eq!(size, 13); // Length of "Hello, world!"
}

#[test_case]
fn test_file_read_all() {
    // Setup
    let mut manager = VfsManager::new();
    let device = Box::new(MockBlockDevice::new(1, "test_disk", 512, 100));
    let fs = Box::new(TestFileSystem::new(0, "testfs", device, 512));
    
    let fs_id = manager.register_fs(fs); // Get fs_id
    let _ = manager.mount(fs_id, "/mnt"); // Use fs_id
    
    // let mut file = File::new("/mnt/test.txt".to_string(), 0);
    let mut file = File::open_with_manager("/mnt/test.txt".to_string(), &mut manager).unwrap();
    // Write
    file.write(b"Hello, world!").unwrap();
    
    // Read the entire file
    let content = file.read_all().unwrap();
    assert_eq!(content, b"Hello, world!");
    
    // Modify part of the file and read again
    file.seek(SeekFrom::Start(0)).unwrap();
    file.write(b"Modified, ").unwrap();
    file.write(b"world!").unwrap();
    
    file.seek(SeekFrom::Start(0)).unwrap();
    let modified_content = file.read_all().unwrap();
    assert_eq!(modified_content, b"Modified, world!");
}

#[test_case]
fn test_file_auto_close() {
    // Setup
    let mut manager = VfsManager::new();
    let device = Box::new(MockBlockDevice::new(1, "test_disk", 512, 100));
    let fs = Box::new(TestFileSystem::new(0, "testfs", device, 512));
    
    let fs_id = manager.register_fs(fs); // Get fs_id
    let _ = manager.mount(fs_id, "/mnt"); // Use fs_id
    
    // Open a file within a scope
    {
        // let mut file = File::new("/mnt/test.txt".to_string(), 0);
        let file = File::open_with_manager("/mnt/test.txt".to_string(), &mut manager);
        assert!(file.is_ok());
        
        // Exiting the scope will automatically close the file due to the Drop trait
    }
    
    // Verify that a new file object can be created and opened
    let file2 = File::open_with_manager("/mnt/test.txt".to_string(), &mut manager);
    assert!(file2.is_ok());
}

#[test_case]
fn test_directory_creation() {
    // Setup
    let mut manager = VfsManager::new();
    let device = Box::new(MockBlockDevice::new(1, "test_disk", 512, 100));
    let fs = Box::new(TestFileSystem::new(0, "testfs", device, 512));
    
    let fs_id = manager.register_fs(fs); // Get fs_id
    let _ = manager.mount(fs_id, "/mnt"); // Use fs_id
    
    // Create an instance of the directory structure
    let dir = Directory::open_with_manager("/mnt".to_string(), &mut manager);
    assert_eq!(dir.path, "/mnt");
}

#[test_case]
fn test_directory_read_entries() {
    // Setup
    let mut manager = VfsManager::new();
    let device = Box::new(MockBlockDevice::new(1, "test_disk", 512, 100));
    let fs = Box::new(TestFileSystem::new(0, "testfs", device, 512));
    
    let fs_id = manager.register_fs(fs); // Get fs_id
    let _ = manager.mount(fs_id, "/mnt"); // Use fs_id
    
    // Read directory entries
    let dir = Directory::open_with_manager("/mnt".to_string(), &mut manager);
    let entries = dir.read_entries().unwrap();
    
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].name, "test.txt");
    assert_eq!(entries[1].name, "testdir");
    assert_eq!(entries[0].file_type, FileType::RegularFile);
    assert_eq!(entries[1].file_type, FileType::Directory);
}

#[test_case]
fn test_directory_create_file() {
    // Setup
    let mut manager = VfsManager::new();
    let device = Box::new(MockBlockDevice::new(1, "test_disk", 512, 100));
    let fs = Box::new(TestFileSystem::new(0, "testfs", device, 512));
    
    let fs_id = manager.register_fs(fs); // Get fs_id
    let _ = manager.mount(fs_id, "/mnt"); // Use fs_id
    
    // Create a file in the directory
    let dir = Directory::open_with_manager("/mnt".to_string(), &mut manager);
    let result = dir.create_file("newfile.txt");
    assert!(result.is_ok());
    
    // Verify the created file
    let entries = dir.read_entries().unwrap();
    assert!(entries.iter().any(|e| e.name == "newfile.txt" && e.file_type == FileType::RegularFile));
    
    // Try opening the file
    let file = File::open_with_manager("/mnt/newfile.txt".to_string(), &mut manager);
    assert!(file.is_ok());
}

#[test_case]
fn test_directory_create_subdirectory() {
    // Setup
    let mut manager = VfsManager::new();
    let device = Box::new(MockBlockDevice::new(1, "test_disk", 512, 100));
    let fs = Box::new(TestFileSystem::new(0, "testfs", device, 512));
    
    let fs_id = manager.register_fs(fs); // Get fs_id
    let _ = manager.mount(fs_id, "/mnt"); // Use fs_id
    
    // Create a subdirectory
    let dir = Directory::open_with_manager("/mnt".to_string(), &mut manager);
    let result = dir.create_dir("subdir");
    assert!(result.is_ok());
    
    // Verify the created directory
    let entries = dir.read_entries().unwrap();
    assert!(entries.iter().any(|e| e.name == "subdir" && e.file_type == FileType::Directory));
    
    // Operate on the subdirectory
    let subdir = Directory::open_with_manager("/mnt/subdir".to_string(), &mut manager);
    let entries = subdir.read_entries().unwrap();
    assert!(entries.is_empty()); // Newly created directory is empty
}

#[test_case]
fn test_directory_nested_operations() {
    // Setup
    let mut manager = VfsManager::new();
    let device = Box::new(MockBlockDevice::new(1, "test_disk", 512, 100));
    let fs = Box::new(TestFileSystem::new(0, "testfs", device, 512));
    
    let fs_id = manager.register_fs(fs); // Get fs_id
    let _ = manager.mount(fs_id, "/mnt"); // Use fs_id
    
    // Nested operations
    let root_dir = Directory::open_with_manager("/mnt".to_string(), &mut manager);
    
    // Create a subdirectory
    root_dir.create_dir("level1").unwrap();
    
    // Operate on the subdirectory
    let level1_dir = Directory::open_with_manager("/mnt/level1".to_string(), &mut manager);
    level1_dir.create_dir("level2").unwrap();
    level1_dir.create_file("file_in_level1.txt").unwrap();
    
    // Operate on a deeper level
    let level2_dir = Directory::open_with_manager("/mnt/level1/level2".to_string(), &mut manager);
    level2_dir.create_file("deep_file.txt").unwrap();
    
    // Verify
    let entries = level2_dir.read_entries().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "deep_file.txt");
}

#[test_case]
fn test_directory_error_handling() {
    // Setup
    let mut manager = VfsManager::new();
    let device = Box::new(MockBlockDevice::new(1, "test_disk", 512, 100));
    let fs = Box::new(TestFileSystem::new(0, "testfs", device, 512));
    
    let fs_id = manager.register_fs(fs); // Get fs_id
    let _ = manager.mount(fs_id, "/mnt"); // Use fs_id
    
    // Non-existent directory
    let nonexistent_dir = Directory::open_with_manager("/mnt/nonexistent".to_string(), &mut manager);
    let result = nonexistent_dir.read_entries();
    match result {
        Ok(_) => panic!("Expected an error, but got success"),
        Err(e) => {
            assert_eq!(e.kind, FileSystemErrorKind::NotFound);
            assert_eq!(e.message, "Directory not found".to_string());
        }
    }
    
    // Create a file in a non-existent directory
    let result = nonexistent_dir.create_file("test.txt");
    match result {
        Ok(_) => panic!("Expected an error, but got success"),
        Err(e) => {
            assert_eq!(e.kind, FileSystemErrorKind::NotFound);
            assert_eq!(e.message, "Parent directory not found".to_string());
        }
    }
    
    // Treat a file as a directory
    let file_as_dir = Directory::open_with_manager("/mnt/test.txt".to_string(), &mut manager);
    let result = file_as_dir.read_entries();
    match result {
        Ok(_) => panic!("Expected an error, but got success"),
        Err(e) => {
            assert_eq!(e.kind, FileSystemErrorKind::NotADirectory);
            assert_eq!(e.message, "Not a directory".to_string());
        }
    }
}

#[test_case]
fn test_directory_with_global_manager() {
    // Setup the global VFS manager
    let device = Box::new(MockBlockDevice::new(1, "test_disk", 512, 100));
    let fs = Box::new(TestFileSystem::new(0, "testfs", device, 512));
    
    let global_manager = get_vfs_manager();
    let fs_id = global_manager.register_fs(fs);
    let _ = global_manager.mount(fs_id, "/mnt"); // Use fs_id
    
    // Directory operations using the global manager
    let dir = Directory::open("/mnt".to_string());
    let entries = dir.read_entries().unwrap();
    assert_eq!(entries.len(), 2);
    
    // Cleanup
    let _ = global_manager.unmount("/mnt");
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
    assert_eq!(fs_id, 0); // The first registration should have ID 0
    assert_eq!(manager.filesystems.read().len(), 1);

    // Check the name of the registered file system
    let registered_fs = manager.filesystems.read()[0].clone();
    assert_eq!(registered_fs.read().name(), "testfs");

    // Mount and verify functionality
    let result = manager.mount(fs_id, "/mnt");
    assert!(result.is_ok());
    assert_eq!(manager.mount_points.read().len(), 1);
    assert!(manager.mount_points.read().contains_key("/mnt"));
}

#[test_case]
fn test_nested_mount_points() {
    // Setup: Create 3 different file systems
    let mut manager = VfsManager::new();
    
    // Root file system
    let root_device = Box::new(MockBlockDevice::new(1, "root_disk", 512, 100));
    let root_fs = Box::new(TestFileSystem::new(0, "rootfs", root_device, 512));
    let root_fs_id = manager.register_fs(root_fs);
    
    // File system for /mnt
    let mnt_device = Box::new(MockBlockDevice::new(2, "mnt_disk", 512, 100));
    let mnt_fs = Box::new(TestFileSystem::new(0, "mntfs", mnt_device, 512));
    let mnt_fs_id = manager.register_fs(mnt_fs);
    
    // File system for /mnt/usb
    let usb_device = Box::new(MockBlockDevice::new(3, "usb_disk", 512, 100));

    let usb_fs = Box::new(TestFileSystem::new(0, "usbfs", usb_device, 512));
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
    manager.create_file("/rootfile.txt").unwrap();
    
    // Create file in /mnt
    manager.create_file("/mnt/mntfile.txt").unwrap();
    
    // Create file in /mnt/usb
    manager.create_file("/mnt/usb/usbfile.txt").unwrap();
    
    // Verify directory listings at each mount point
    let root_entries = manager.read_dir("/").unwrap();
    let mnt_entries = manager.read_dir("/mnt").unwrap();
    let usb_entries = manager.read_dir("/mnt/usb").unwrap();
    
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
    assert_eq!(manager.mount_points.read().len(), 0);
    // Ensure all file systems are returned to the registration list
    assert_eq!(manager.filesystems.read().len(), 3);
}

#[test_case]
fn test_directory_boundary_handling() {
    // Setup: Create similar mount points
    let mut manager = VfsManager::new();
    
    // Create 4 different file systems
    let root_fs = Box::new(TestFileSystem::new(0, "rootfs", 
        Box::new(MockBlockDevice::new(1, "root_disk", 512, 100)), 512));
    let mnt_fs = Box::new(TestFileSystem::new(0, "mntfs", 
        Box::new(MockBlockDevice::new(2, "mnt_disk", 512, 100)), 512));
    let mnt_data_fs = Box::new(TestFileSystem::new(0, "mnt_datafs", 
        Box::new(MockBlockDevice::new(3, "mnt_data_disk", 512, 100)), 512));
    let mnt_sub_fs = Box::new(TestFileSystem::new(0, "mnt_subfs", 
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
    manager.create_file("/mnt_data/testfile.txt").unwrap();
    
    // Create file in /mnt/sub
    manager.create_file("/mnt/sub/testfile.txt").unwrap();
    
    // Ensure files are correctly created in each file system
    let mnt_data_entries = manager.read_dir("/mnt_data").unwrap();
    assert!(mnt_data_entries.iter().any(|e| e.name == "testfile.txt"));
    
    let mnt_sub_entries = manager.read_dir("/mnt/sub").unwrap();
    assert!(mnt_sub_entries.iter().any(|e| e.name == "testfile.txt"));
    
    // Ensure delete operations work correctly
    manager.remove("/mnt_data/testfile.txt").unwrap();
    let mnt_data_entries = manager.read_dir("/mnt_data").unwrap();
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
    
    // 1. メインシステム用のファイルシステムをグローバルVfsManagerに設定
    let global_vfs = get_vfs_manager();
    
    // メインシステム用のファイルシステム（TestFileSystemを使用）
    let main_device = Box::new(MockBlockDevice::new(1, "main_disk", 512, 100));
    let main_fs = Box::new(TestFileSystem::new(0, "main_testfs", main_device, 512));
    let main_fs_id = global_vfs.register_fs(main_fs);
    global_vfs.mount(main_fs_id, "/")
        .expect("Failed to mount main filesystem");
    
    // メインシステムでディレクトリとファイルを作成
    global_vfs.create_dir("/system").expect("Failed to create /system");
    global_vfs.create_file("/system/main.conf").expect("Failed to create main config");
    
    // 2. コンテナ1用の独立したVfsManagerを作成
    let mut container1_vfs = VfsManager::new();
    
    // コンテナ1用のファイルシステム
    let container1_device = Box::new(MockBlockDevice::new(2, "container1_disk", 512, 100));
    let container1_fs = Box::new(TestFileSystem::new(1, "container1_testfs", container1_device, 512));
    let container1_fs_id = container1_vfs.register_fs(container1_fs);
    container1_vfs.mount(container1_fs_id, "/")
        .expect("Failed to mount container1 filesystem");
    
    // コンテナ1のファイルシステムにアプリケーション用のファイルを作成
    container1_vfs.create_dir("/app").expect("Failed to create /app");
    container1_vfs.create_file("/app/config.json").expect("Failed to create app config");
    container1_vfs.create_dir("/tmp").expect("Failed to create /tmp");
    
    // 3. コンテナ2用の独立したVfsManagerを作成
    let mut container2_vfs = VfsManager::new();
    
    // コンテナ2用のファイルシステム
    let container2_device = Box::new(MockBlockDevice::new(3, "container2_disk", 512, 100));
    let container2_fs = Box::new(TestFileSystem::new(2, "container2_testfs", container2_device, 512));
    let container2_fs_id = container2_vfs.register_fs(container2_fs);
    container2_vfs.mount(container2_fs_id, "/")
        .expect("Failed to mount container2 filesystem");
    
    // コンテナ2のファイルシステムに異なるアプリケーション用のファイルを作成
    container2_vfs.create_dir("/service").expect("Failed to create /service");
    container2_vfs.create_file("/service/daemon.conf").expect("Failed to create daemon config");
    container2_vfs.create_dir("/data").expect("Failed to create /data");
    
    // 4. 異なるVfsManagerを持つタスクを作成
    
    // メインシステムタスク（グローバルVfsManagerを使用）
    let mut main_task = new_user_task("main_system".to_string(), 0);
    main_task.vfs = None; // グローバルVfsManagerを使用
    main_task.cwd = Some("/".to_string());
    
    // コンテナ1タスク（独立したVfsManagerを使用）
    let mut container1_task = new_user_task("container1_app".to_string(), 0);
    container1_task.vfs = Some(Arc::new(container1_vfs));
    container1_task.cwd = Some("/app".to_string());
    
    // コンテナ2タスク（独立したVfsManagerを使用）
    let mut container2_task = new_user_task("container2_service".to_string(), 0);
    container2_task.vfs = Some(Arc::new(container2_vfs));
    container2_task.cwd = Some("/service".to_string());
    
    // 5. 各タスクからのファイルシステムアクセスをテスト
    
    // メインシステムタスクからのアクセス（グローバルVfsManagerを使用）
    let main_entries = global_vfs
        .read_dir("/")
        .expect("Failed to read root directory from main task");
    
    // /systemディレクトリが見えることを確認
    assert!(main_entries.iter().any(|e| e.name == "system"));
    
    // コンテナ1タスクからのアクセス
    let container1_entries = container1_task.vfs.as_ref().unwrap()
        .read_dir("/")
        .expect("Failed to read root directory from container1 task");
    
    // /appディレクトリが見えて、/systemは見えないことを確認
    assert!(container1_entries.iter().any(|e| e.name == "app"));
    assert!(!container1_entries.iter().any(|e| e.name == "system"));
    
    // コンテナ2タスクからのアクセス
    let container2_entries = container2_task.vfs.as_ref().unwrap()
        .read_dir("/")
        .expect("Failed to read root directory from container2 task");
    
    // /serviceディレクトリが見えて、/systemや/appは見えないことを確認
    assert!(container2_entries.iter().any(|e| e.name == "service"));
    assert!(!container2_entries.iter().any(|e| e.name == "system"));
    assert!(!container2_entries.iter().any(|e| e.name == "app"));
    
    // 6. パス解決のテスト
    
    // 各タスクの現在のワーキングディレクトリからの相対パス解決
    let main_abs_path = VfsManager::to_absolute_path(&main_task, "main.conf")
        .expect("Failed to resolve path in main task");
    assert_eq!(main_abs_path, "/main.conf");
    
    let container1_abs_path = VfsManager::to_absolute_path(&container1_task, "config.json")
        .expect("Failed to resolve path in container1 task");
    assert_eq!(container1_abs_path, "/app/config.json");
    
    let container2_abs_path = VfsManager::to_absolute_path(&container2_task, "daemon.conf")
        .expect("Failed to resolve path in container2 task");
    assert_eq!(container2_abs_path, "/service/daemon.conf");
    
    // 7. ファイルアクセスの分離をテスト
    
    // メインシステムのファイルにコンテナからアクセスできないことを確認
    let container1_main_access = container1_task.vfs.as_ref().unwrap()
        .open("/system/main.conf", 0);
    assert!(container1_main_access.is_err(), "Container1 should not access main system files");
    
    // コンテナ1のファイルにコンテナ2からアクセスできないことを確認
    let container2_app_access = container2_task.vfs.as_ref().unwrap()
        .open("/app/config.json", 0);
    assert!(container2_app_access.is_err(), "Container2 should not access container1 files");
    
    // 各コンテナは自分のファイルにはアクセスできることを確認
    let container1_own_access = container1_task.vfs.as_ref().unwrap()
        .open("/app/config.json", 0);
    assert!(container1_own_access.is_ok(), "Container1 should access its own files");
    
    let container2_own_access = container2_task.vfs.as_ref().unwrap()
        .open("/service/daemon.conf", 0);
    assert!(container2_own_access.is_ok(), "Container2 should access its own files");
    
    // 8. タスククローン時のVfsManager継承をテスト
    
    // コンテナ1タスクをクローンして、VfsManagerが継承されることを確認
    let cloned_container1_task = container1_task.clone_task()
        .expect("Failed to clone container1 task");
    
    // クローンされたタスクも同じVfsManagerを使用することを確認
    assert!(cloned_container1_task.vfs.is_some());
    
    // クローンされたタスクからも同じファイルシステムが見えることを確認
    let cloned_entries = cloned_container1_task.vfs.as_ref().unwrap()
        .read_dir("/")
        .expect("Failed to read directory from cloned task");
    assert!(cloned_entries.iter().any(|e| e.name == "app"));
    assert!(!cloned_entries.iter().any(|e| e.name == "system"));
    
    // 9. VfsManagerの統計情報を確認
    assert_eq!(global_vfs.mount_points.read().len(), 1);
    assert_eq!(container1_task.vfs.as_ref().unwrap().mount_points.read().len(), 1);
    assert_eq!(container2_task.vfs.as_ref().unwrap().mount_points.read().len(), 1);
    
    // 10. クリーンアップ
    let _ = global_vfs.unmount("/");
    
    // === Container Demo completed successfully! ===
    // 実証された機能:
    // - ✓ Multiple isolated VfsManager instances
    // - ✓ Different filesystem views per container/task
    // - ✓ Path resolution isolation
    // - ✓ File access isolation between containers
    // - ✓ VfsManager inheritance in task cloning
    // - ✓ Container-like filesystem namespace isolation
}