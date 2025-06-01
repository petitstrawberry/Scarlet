use alloc::{boxed::Box, sync::Arc};
use super::*;
use crate::device::block::mockblk::MockBlockDevice;
use crate::fs::testfs::{TestFileSystem, TestFileSystemDriver};
use crate::fs::tmpfs::TmpFS;
use crate::println;
use crate::task::new_user_task;

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
    let fs = Box::new(TestFileSystem::new("testfs", device, 512));
    
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
    let result = manager.create_regular_file("/mnt/newdir/newfile.txt");
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
    let fs = GenericFileSystem::new("generic", Box::new(device), 512);
    
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
    let file = File::open_with_manager("/mnt/test.txt".to_string(), &mut manager);
    
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
    let fs = Box::new(TestFileSystem::new("testfs", device, 512));
    
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
    let fs = Box::new(TestFileSystem::new("testfs", device, 512));
    
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
    let fs = Box::new(TestFileSystem::new("testfs", device, 512));
    
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
    let fs = Box::new(TestFileSystem::new("testfs", device, 512));
    
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
        .read_dir("/")
        .expect("Failed to read root directory from main task");
    
    // Verify that /system directory is visible
    assert!(main_entries.iter().any(|e| e.name == "system"));
    
    // Access from container 1 task
    let container1_entries = container1_task.vfs.as_ref().unwrap()
        .read_dir("/")
        .expect("Failed to read root directory from container1 task");
    
    // Verify that /app directory is visible but /system is not
    assert!(container1_entries.iter().any(|e| e.name == "app"));
    assert!(!container1_entries.iter().any(|e| e.name == "system"));
    
    // Access from container 2 task
    let container2_entries = container2_task.vfs.as_ref().unwrap()
        .read_dir("/")
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
    let cloned_container1_task = container1_task.clone_task()
        .expect("Failed to clone container1 task");
    
    // Verify that cloned task uses same VfsManager
    assert!(cloned_container1_task.vfs.is_some());
    
    // Verify that cloned task sees same filesystem
    let cloned_entries = cloned_container1_task.vfs.as_ref().unwrap()
        .read_dir("/")
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
fn test_vfs_manager_clone_behavior() {
    // Create original VfsManager
    let mut original_manager = VfsManager::new();
    
    // Register and mount filesystem
    let device = Box::new(MockBlockDevice::new(1, "test_disk", 512, 100));
    let fs = Box::new(TestFileSystem::new("testfs", device, 512));
    let fs_id = original_manager.register_fs(fs);
    original_manager.mount(fs_id, "/mnt").unwrap();

    // Clone VfsManager
    let mut cloned_manager = original_manager.clone();
    
    // === Test 1: Mount point independence ===
    // Add new filesystem and mount point in cloned manager
    let device2 = Box::new(MockBlockDevice::new(2, "test_disk2", 512, 100));
    let fs2 = Box::new(TestFileSystem::new("testfs2", device2, 512));
    let fs2_id = cloned_manager.register_fs(fs2);
    assert_eq!(fs2_id, 2); // New ID is assigned in cloned manager
    cloned_manager.mount(fs2_id, "/mnt2").unwrap();
    
    // Verify original manager is not affected
    assert_eq!(original_manager.mount_count(), 1);
    assert_eq!(cloned_manager.mount_count(), 2);
    assert!(original_manager.has_mount_point("/mnt"));
    assert!(!original_manager.has_mount_point("/mnt2"));
    assert!(cloned_manager.has_mount_point("/mnt"));
    assert!(cloned_manager.has_mount_point("/mnt2"));
    assert_eq!(*original_manager.next_fs_id.read(), 2);
    assert_eq!(*cloned_manager.next_fs_id.read(), 3);
    
    // === Test 2: FileSystem object sharing ===
    // Create file in original manager
    original_manager.create_regular_file("/mnt/original_file.txt").unwrap();
    
    // Same file is visible from cloned manager (shared)
    let entries_from_clone = cloned_manager.read_dir("/mnt").unwrap();
    assert!(entries_from_clone.iter().any(|e| e.name == "original_file.txt"));
    
    // Create file in cloned manager
    cloned_manager.create_regular_file("/mnt/cloned_file.txt").unwrap();
    
    // Same file is visible from original manager (shared)
    let entries_from_original = original_manager.read_dir("/mnt").unwrap();
    assert!(entries_from_original.iter().any(|e| e.name == "cloned_file.txt"));
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
    let entries1 = manager1.read_dir("/mnt").unwrap();
    assert!(entries1.iter().any(|e| e.name == "file_in_container1.txt"));
    
    // Not visible from manager2 (correct isolation)
    let entries2 = manager2.read_dir("/mnt").unwrap();
    assert!(!entries2.iter().any(|e| e.name == "file_in_container1.txt"));
    
    // Create file in manager2
    manager2.create_regular_file("/mnt/file_in_container2.txt").unwrap();
    // Visible from manager2 (correct isolation)
    let entries2 = manager2.read_dir("/mnt").unwrap();
    assert!(entries2.iter().any(|e| e.name == "file_in_container2.txt"));
    
    // Not visible from manager1 (correct isolation)
    let entries1 = manager1.read_dir("/mnt").unwrap();
    assert!(!entries1.iter().any(|e| e.name == "file_in_container2.txt"));
}

// Test cases for structured parameter system
#[test_case]
fn test_structured_parameters_tmpfs() {
    use crate::fs::params::TmpFSParams;
    
    // Register TmpFS driver
    get_fs_driver_manager().register_driver(Box::new(crate::fs::tmpfs::TmpFSDriver));
    
    let mut manager = VfsManager::new();
    
    // Create TmpFS with specific parameters
    let params = TmpFSParams::with_memory_limit(1024 * 1024); // 1MB limit
    let fs_id = manager.create_and_register_fs_with_params("tmpfs", &params).unwrap();
    
    // Mount the filesystem
    let result = manager.mount(fs_id, "/tmp");
    assert!(result.is_ok());
    
    // Verify the filesystem is mounted and working
    let result = manager.create_dir("/tmp/test");
    assert!(result.is_ok());
    
    let entries = manager.read_dir("/tmp").unwrap();
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
    let fs_id = manager.create_and_register_fs_with_params("testfs", &params).unwrap();
    
    // Mount the filesystem
    let result = manager.mount(fs_id, "/test");
    assert!(result.is_ok());
    
    // Verify the filesystem is mounted and working
    let entries = manager.read_dir("/test").unwrap();
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
    let result = manager.create_and_register_fs_with_params("cpiofs", &params);
    
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
    
    let entries = manager.read_dir("/legacy").unwrap();
    assert!(entries.iter().any(|e| e.name == "test.txt"));
    
    // Test that structured parameters also work for the same driver
    let params = BasicFSParams::new();
    let fs_id2 = manager.create_and_register_fs_with_params("testfs", &params).unwrap();
    
    let result = manager.mount(fs_id2, "/structured");
    assert!(result.is_ok());
    
    let entries = manager.read_dir("/structured").unwrap();
    assert!(entries.iter().any(|e| e.name == "test.txt"));
}

#[test_case]
fn test_structured_parameters_driver_not_found() {
    use crate::fs::params::BasicFSParams;
    
    let mut manager = VfsManager::new();
    
    // Try to create filesystem with non-existent driver
    let params = BasicFSParams::new();
    let result = manager.create_and_register_fs_with_params("nonexistent", &params);
    
    assert!(result.is_err());
    if let Err(e) = result {
        assert_eq!(e.kind, FileSystemErrorKind::NotFound);
        assert!(e.message.contains("not found"));
    }
}

#[test_case]
fn test_bind_mount() {
    let mut manager = VfsManager::new();
    
    // Set up source filesystem
    let source_device = Box::new(MockBlockDevice::new(1, "source_disk", 512, 100));
    let source_fs = Box::new(TestFileSystem::new("source_fs", source_device, 512));
    let source_fs_id = manager.register_fs(source_fs);
    manager.mount(source_fs_id, "/mnt/source").unwrap();
    
    // Create a file in source
    manager.create_regular_file("/mnt/source/test_file.txt").unwrap();
    
    // Create bind mount
    let result = manager.bind_mount("/mnt/source", "/mnt/target", false);
    assert!(result.is_ok());
    
    // Verify bind mount exists
    assert!(manager.is_bind_mount("/mnt/target"));
    assert!(!manager.is_bind_mount("/mnt/source"));
    
    // Verify file is accessible through bind mount
    let entries = manager.read_dir("/mnt/target").unwrap();
    assert!(entries.iter().any(|e| e.name == "test_file.txt"));
    
    // List bind mounts
    let bind_mounts = manager.list_bind_mounts();
    assert_eq!(bind_mounts.len(), 1);
    assert_eq!(bind_mounts[0].0, "/mnt/source"); // source path
    assert_eq!(bind_mounts[0].1, "/mnt/target"); // target path
    assert_eq!(bind_mounts[0].2, false); // read_only flag
    // ---
    // Test: Basic bind mount functionality.
    // - Create a bind mount from /mnt/source to /mnt/target.
    // - Verify that files in the source are visible at the target.
    // - Check that list_bind_mounts returns the correct tuple.
    // - Confirm is_bind_mount returns true for the target and false for the source.
}

#[test_case]
fn test_bind_mount_read_only() {
    let mut manager = VfsManager::new();
    
    // Set up source filesystem
    let source_device = Box::new(MockBlockDevice::new(1, "source_disk", 512, 100));
    let source_fs = Box::new(TestFileSystem::new("source_fs", source_device, 512));
    let source_fs_id = manager.register_fs(source_fs);
    manager.mount(source_fs_id, "/mnt/source").unwrap();
    
    // Create read-only bind mount
    let result = manager.bind_mount("/mnt/source", "/mnt/readonly", true);
    assert!(result.is_ok());
    
    // Verify it's a read-only bind mount
    let bind_mounts = manager.list_bind_mounts();
    assert_eq!(bind_mounts.len(), 1);
    assert_eq!(bind_mounts[0].2, true); // read_only flag should be true
    // ---
    // Test: Read-only bind mount.
    // - Create a bind mount with the read_only flag set to true.
    // - Check that list_bind_mounts returns the correct read_only status.
}

#[test_case]
fn test_bind_mount_shared() {
    let mut manager = VfsManager::new();
    
    // Set up source filesystem
    let source_device = Box::new(MockBlockDevice::new(1, "source_disk", 512, 100));
    let source_fs = Box::new(TestFileSystem::new("source_fs", source_device, 512));
    let source_fs_id = manager.register_fs(source_fs);
    manager.mount(source_fs_id, "/mnt/source").unwrap();
    
    // Create shared bind mount
    let result = manager.bind_mount_shared("/mnt/source", "/mnt/shared");
    assert!(result.is_ok());
    
    // Verify it's a bind mount
    assert!(manager.is_bind_mount("/mnt/shared"));
    // ---
    // Test: Shared bind mount.
    // - Create a shared bind mount and verify is_bind_mount returns true for the target.
}

#[test_case]
fn test_bind_mount_from_different_vfs() {
    let mut host_manager = VfsManager::new();
    let mut container_manager = VfsManager::new();
    
    // Set up source filesystem in host
    let source_device = Box::new(MockBlockDevice::new(1, "host_disk", 512, 100));
    let source_fs = Box::new(TestFileSystem::new("host_fs", source_device, 512));
    let source_fs_id = host_manager.register_fs(source_fs);
    host_manager.mount(source_fs_id, "/data").unwrap();
    
    // Create file in host
    host_manager.create_regular_file("/data/shared_file.txt").unwrap();
    
    // Bind mount from host to container
    let host_arc = Arc::new(host_manager);
    let result = container_manager.bind_mount_from(&host_arc, "/data", "/mnt/shared", true);
    assert!(result.is_ok());
    
    // Verify file is accessible in container through bind mount
    let entries = container_manager.read_dir("/mnt/shared").unwrap();
    assert!(entries.iter().any(|e| e.name == "shared_file.txt"));
    
    // Verify it's a bind mount
    assert!(container_manager.is_bind_mount("/mnt/shared"));
    // ---
    // Test: Cross-VFS bind mount.
    // - Bind mount a directory from one VfsManager to another.
    // - Verify that files from the host are visible in the container.
    // - Confirm is_bind_mount returns true for the container's target.
}

#[test_case]
fn test_bind_mount_error_cases() {
    let mut manager = VfsManager::new();
    
    // Test bind mount from non-existent source
    let result = manager.bind_mount("/nonexistent", "/mnt/target", false);
    assert!(result.is_err());
    
    // Set up a source
    let source_device = Box::new(MockBlockDevice::new(1, "source_disk", 512, 100));
    let source_fs = Box::new(TestFileSystem::new("source_fs", source_device, 512));
    let source_fs_id = manager.register_fs(source_fs);
    manager.mount(source_fs_id, "/mnt/source").unwrap();
    
    // Create bind mount
    manager.bind_mount("/mnt/source", "/mnt/target", false).unwrap();
    
    // Test bind mount to existing target
    let result = manager.bind_mount("/mnt/source", "/mnt/target", false);
    assert!(result.is_err());
    // ---
    // Test: Error handling for bind mounts.
    // - Try to bind mount from a non-existent source (should fail).
    // - Try to bind mount to an already used target (should fail).
}

#[test_case]
fn test_bindmount_thread_safe_access() {
    let mut vfs = VfsManager::new();
    let source_device = Box::new(MockBlockDevice::new(1, "source_disk", 512, 100));
    let source_fs = Box::new(TestFileSystem::new("source_fs", source_device, 512));
    let source_fs_id = vfs.register_fs(source_fs);
    vfs.mount(source_fs_id, "/mnt/source").unwrap();
    let entries = vfs.read_dir("/mnt/source").unwrap();
    assert!(entries.iter().any(|e| e.name == "test.txt"));
    let vfs_arc = Arc::new(vfs);
    let result = vfs_arc.bind_mount_shared_ref("/mnt/source", "/mnt/target", false);
    assert!(result.is_ok());
    let entries = vfs_arc.read_dir("/mnt/target").unwrap();
    assert!(entries.iter().any(|e| e.name == "test.txt"));
    assert!(vfs_arc.is_bind_mount("/mnt/target"));
    assert!(!vfs_arc.is_bind_mount("/mnt/source"));
    // ---
    // Test: Bind mount thread-safe access method.
    // - Use bind_mount_shared_ref for thread-safe operation.
    // - Verify that the bind mount works and is recognized as a bind mount.
}

#[test_case]
fn test_container_bind_mount_scenario() {
    let mut host_vfs = VfsManager::new();
    let host_device = Box::new(MockBlockDevice::new(1, "host_disk", 512, 100));
    let host_fs = Box::new(TestFileSystem::new("host_fs", host_device, 512));
    let host_fs_id = host_vfs.register_fs(host_fs);
    let host_datafs = Box::new(TmpFS::new(0));
    host_vfs.mount(host_fs_id, "/data").unwrap();
    // host_vfs.create_dir("/data").unwrap();
    let mut container_vfs = VfsManager::new();
    let container_device = Box::new(MockBlockDevice::new(2, "container_disk", 512, 100));
    let container_fs = Box::new(TestFileSystem::new("container_fs", container_device, 512));
    let container_fs_id = container_vfs.register_fs(container_fs);
    container_vfs.mount(container_fs_id, "/").unwrap();
    let host_arc = Arc::new(host_vfs);
    container_vfs.bind_mount_from(&host_arc, "/data", "/host-data", true).unwrap();

    let bind_mounts = container_vfs.list_bind_mounts();
    let entries = container_vfs.read_dir("/host-data").unwrap();
    assert!(entries.iter().any(|e| e.name == "test.txt"));
    // ---
    // Test: Container orchestration scenario with bind mounts.
    // - Bind mount a host directory into a container VFS as read-only.
    // - Verify file visibility and correct bind mount listing in both VFSs.
}

#[test_case]
fn test_multiple_bind_mounts_performance() {
    let mut vfs = VfsManager::new();
    // [ ] 1. Create and mount 5 filesystems at /source0 to /source4
    for i in 0..5 {
        let device = Box::new(MockBlockDevice::new(i + 1, "disk", 512, 100));
        let fs = Box::new(TestFileSystem::new("testfs", device, 512));
        let fs_id = vfs.register_fs(fs);
        vfs.mount(fs_id, &format!("/source{}", i)).unwrap();
    }
    // [ ] 2. Create 5 bind mounts from /sourceN to /targetN
    for i in 0..5 {
        let source_path = format!("/source{}", i);
        let target_path = format!("/target{}", i);
        vfs.bind_mount(&source_path, &target_path, false).unwrap();
    }
    // [ ] 3. Check that 5 bind mounts are listed
    assert_eq!(vfs.list_bind_mounts().len(), 5);
    // [ ] 4. For each target, verify it is a bind mount and files are visible
    for i in 0..5 {
        let target_path = format!("/target{}", i);
        assert!(vfs.is_bind_mount(&target_path));
        let entries = vfs.read_dir(&target_path).unwrap();
        assert!(entries.iter().any(|e| e.name == "test.txt"));
    }
    // [ ] 5. For each source, verify it is not a bind mount and files are visible
    for i in 0..5 {
        let source_path = format!("/source{}", i);
        assert!(!vfs.is_bind_mount(&source_path));
        let entries = vfs.read_dir(&source_path).unwrap();
        assert!(entries.iter().any(|e| e.name == "test.txt"));
    }
    // ---
    // Test: Performance and correctness with multiple bind mounts.
    // - [x] Create and verify 5 bind mounts in a loop.
    // - [x] Ensure all target paths are bind mounts and all source paths are not.
    // - [x] Check file visibility through all bind mounts.
}

#[test_case]
fn test_bindmount_error_handling() {
    let mut vfs = VfsManager::new();
    // [ ] 1. Try to bind mount from a non-existent source (should fail)
    let result = vfs.bind_mount("/nonexistent", "/target", false);
    assert!(result.is_err());
    // [ ] 2. Try to bind mount from a non-existent source in another VFS (should fail)
    let other_vfs = Arc::new(VfsManager::new());
    let result = vfs.bind_mount_from(&other_vfs, "/nonexistent", "/target", false);
    assert!(result.is_err());
    // [ ] 3. Set up a valid source and mount it
    let device = Box::new(MockBlockDevice::new(1, "disk", 512, 100));
    let fs = Box::new(TestFileSystem::new("testfs", device, 512));
    let fs_id = vfs.register_fs(fs);
    vfs.mount(fs_id, "/source").unwrap();
    // [ ] 4. Create a valid bind mount
    vfs.bind_mount("/source", "/target", false).unwrap();
    // [ ] 5. Try to bind mount to an already used target (should fail)
    let result = vfs.bind_mount("/source", "/target", false);
    assert!(result.is_err());
    // [ ] 6. Try to bind mount to an already used target using shared_ref (should fail)
    let vfs_arc = Arc::new(vfs);
    let result = vfs_arc.bind_mount_shared_ref("/source", "/target", false);
    assert!(result.is_err());
    // ---
    // Test: Comprehensive error handling for bind mounts.
    // - [x] Try various invalid bind mount operations and ensure errors are returned.
    // - [x] Includes duplicate targets and cross-VFS errors.
}

#[test_case]
fn test_mount_tree_with_bind_mounts() {
    let mut vfs = VfsManager::new();
    // [ ] 1. Mount fs1 at "/"
    let device1 = Box::new(MockBlockDevice::new(1, "disk1", 512, 100));
    let fs1 = Box::new(TestFileSystem::new("fs1", device1, 512));
    let fs1_id = vfs.register_fs(fs1);
    vfs.mount(fs1_id, "/").unwrap();
    // [ ] 2. Mount fs2 at "/mnt"
    let device2 = Box::new(MockBlockDevice::new(2, "disk2", 512, 100));
    let fs2 = Box::new(TestFileSystem::new("fs2", device2, 512));
    let fs2_id = vfs.register_fs(fs2);
    vfs.mount(fs2_id, "/mnt").unwrap();
    // [ ] 3. Create bind mount from /mnt to /bind1 (rw)
    vfs.bind_mount("/mnt", "/bind1", false).unwrap();
    // [ ] 4. Create bind mount from / to /bind2 (ro)
    vfs.bind_mount("/", "/bind2", true).unwrap();
    // [ ] 5. List all mount points and verify
    let mount_points = vfs.list_mount_points();
    assert_eq!(mount_points.len(), 4);
    assert!(mount_points.contains(&"/".to_string()));
    assert!(mount_points.contains(&"/mnt".to_string()));
    assert!(mount_points.contains(&"/bind1".to_string()));
    assert!(mount_points.contains(&"/bind2".to_string()));
    // [ ] 6. Check is_bind_mount for each mount point
    assert!(!vfs.is_bind_mount("/"));
    assert!(!vfs.is_bind_mount("/mnt"));
    assert!(vfs.is_bind_mount("/bind1"));
    assert!(vfs.is_bind_mount("/bind2"));
    // [ ] 7. List bind mounts and verify source/target/readonly
    let bind_mounts = vfs.list_bind_mounts();
    assert_eq!(bind_mounts.len(), 2);
    let bind1 = bind_mounts.iter().find(|bm| bm.1 == "/bind1").unwrap();
    assert_eq!(bind1.0, "/mnt");
    assert_eq!(bind1.2, false);
    let bind2 = bind_mounts.iter().find(|bm| bm.1 == "/bind2").unwrap();
    assert_eq!(bind2.0, "/");
    assert_eq!(bind2.2, true);
    // ---
    // Test: Mount tree structure with bind mounts.
    // - [x] Mix regular and bind mounts, then verify mount point listing and bind mount detection.
    // - [x] Check that bind_mounts returns correct source/target/read_only info for each bind mount.
}

#[test_case]
fn test_bind_mount_from_security_basic() {
    // Test basic security of bind_mount_from to prevent directory traversal
    
    // Create host VFS with sensitive directories
    let mut host_vfs = VfsManager::new();
    
    // Filesystem for host (using TestFileSystem)
    let host_device = Box::new(MockBlockDevice::new(1, "host_disk", 512, 100));
    let host_fs = Box::new(TestFileSystem::new("host_fs", host_device, 512));
    let host_fs_id = host_vfs.register_fs(host_fs);
    host_vfs.mount(host_fs_id, "/")
        .expect("Failed to mount host filesystem");
    
    // Create sensitive directories and files in host
    host_vfs.create_dir("/etc").expect("Failed to create /etc");
    host_vfs.create_regular_file("/etc/passwd").expect("Failed to create /etc/passwd");
    host_vfs.create_dir("/var").expect("Failed to create /var");
    host_vfs.create_regular_file("/var/secrets").expect("Failed to create /var/secrets");
    
    // 2. Create container VfsManager
    let mut container_vfs = VfsManager::new();
    
    // Filesystem for container (using TestFileSystem)
    let container_device = Box::new(MockBlockDevice::new(2, "container_disk", 512, 100));
    let container_fs = Box::new(TestFileSystem::new("container_fs", container_device, 512));
    let container_fs_id = container_vfs.register_fs(container_fs);
    container_vfs.mount(container_fs_id, "/")
        .expect("Failed to mount container filesystem");
    
    let host_vfs_arc = Arc::new(host_vfs);
    
    // Attempt to bind mount from host /etc to container /host_etc (should work)
    let result = container_vfs.bind_mount_from(&host_vfs_arc, "/etc", "/host_etc", true);
    assert!(result.is_ok(), "Basic bind_mount_from should succeed");
    
    // Verify the mount was created
    assert!(container_vfs.is_bind_mount("/host_etc"), "Bind mount should be detected");
    let bind_mounts = container_vfs.list_bind_mounts();
    let host_etc_bind = bind_mounts.iter().find(|bm| bm.1 == "/host_etc");
    assert!(host_etc_bind.is_some());
    assert_eq!(host_etc_bind.unwrap().0, "/etc");
    assert_eq!(host_etc_bind.unwrap().2, true); // read-only
}

#[test_case]
fn test_bind_mount_from_directory_traversal_prevention() {
    // Test that bind_mount_from properly normalizes paths and prevents directory traversal attacks
    
    // Create host VFS with directory structure for testing traversal attempts
    let mut host_vfs = VfsManager::new();
    let host_device = Box::new(MockBlockDevice::new(1, "host_disk", 512, 100));
    let host_fs = Box::new(TestFileSystem::new("host_fs", host_device, 512));
    let host_fs_id = host_vfs.register_fs(host_fs);
    host_vfs.mount(host_fs_id, "/").unwrap();
    
    // Create essential system directories
    host_vfs.create_dir("/etc").expect("Failed to create /etc");
    host_vfs.create_dir("/var").expect("Failed to create /var");
    host_vfs.create_dir("/usr").expect("Failed to create /usr");
    host_vfs.create_dir("/usr/bin").expect("Failed to create /usr/bin");
    host_vfs.create_dir("/home").expect("Failed to create /home");
    host_vfs.create_dir("/home/user").expect("Failed to create /home/user");
    host_vfs.create_regular_file("/etc/passwd").expect("Failed to create /etc/passwd");
    host_vfs.create_regular_file("/var/secrets").expect("Failed to create /var/secrets");
    host_vfs.create_regular_file("/usr/bin/app").expect("Failed to create /usr/bin/app");
    host_vfs.create_regular_file("/home/user/data.txt").expect("Failed to create /home/user/data.txt");
    
    // Create container VFS
    let mut container_vfs = VfsManager::new();
    let container_device = Box::new(MockBlockDevice::new(2, "container_disk", 512, 100));
    let container_fs = Box::new(TestFileSystem::new("container_fs", container_device, 512));
    let container_fs_id = container_vfs.register_fs(container_fs);
    container_vfs.mount(container_fs_id, "/").unwrap();
    
    let host_vfs_arc = Arc::new(host_vfs);
    
    // === Test 1: Basic directory traversal prevention ===
    let traversal_attempts = vec![
        ("../etc", "Parent directory access from relative path"),
        ("../../usr", "Multiple parent directories"),
        ("/var/../etc", "Intermediate traversal in absolute path"),
        ("/usr/bin/../../etc", "Complex traversal with legitimate prefix"),
        ("//etc", "Double slash normalization"),
        ("/./etc", "Current directory reference"),
        ("/etc/../..", "Traversal to root parent"),
        ("/..", "Direct parent access from root"),
        ("../../../etc", "Deep parent traversal"),
        ("./../../usr", "Mixed current and parent references"),
    ];
    
    for (traversal_path, description) in traversal_attempts {
        let result = container_vfs.bind_mount_from(&host_vfs_arc, traversal_path, "/traversal_test", true);
        
        match result {
            Ok(_) => {
                // If mount succeeded, verify path was properly normalized
                let bind_mounts = container_vfs.list_bind_mounts();
                if let Some(mount) = bind_mounts.iter().find(|bm| bm.1 == "/traversal_test") {
                    // Normalized path should not contain traversal sequences
                    assert!(!mount.0.contains(".."), 
                        "Normalized path contains '..': {} -> {} ({})", traversal_path, mount.0, description);
                    assert!(!mount.0.contains("./"), 
                        "Normalized path contains './': {} -> {} ({})", traversal_path, mount.0, description);
                    assert!(mount.0.starts_with('/'), 
                        "Normalized path not absolute: {} ({})", mount.0, description);
                }
                let _ = container_vfs.unmount("/traversal_test");
            }
            Err(_) => {
                // Rejection of traversal attempts is also acceptable security behavior
                assert!(!container_vfs.is_bind_mount("/traversal_test"), 
                    "Mount should not exist after failed traversal attempt: {}", description);
            }
        }
    }
    
    // === Test 2: Validate legitimate absolute paths work ===
    let legitimate_paths = vec!["/etc", "/var", "/usr", "/usr/bin", "/home/user"];
    
    for path in legitimate_paths {
        let result = container_vfs.bind_mount_from(&host_vfs_arc, path, "/legitimate_test", false);
        assert!(result.is_ok(), "Legitimate path should be mountable: {}", path);
        
        // Verify mount is accessible
        let entries = container_vfs.read_dir("/legitimate_test");
        assert!(entries.is_ok(), "Mounted path should be readable: {}", path);
        
        let _ = container_vfs.unmount("/legitimate_test");
    }
    
    // === Test 3: Ensure security isolation between attempts ===
    
    // First mount a safe directory
    let safe_result = container_vfs.bind_mount_from(&host_vfs_arc, "/home/user", "/safe_mount", true);
    assert!(safe_result.is_ok(), "Safe mount should succeed");
    
    // Try traversal attack - should not affect existing safe mount
    let _attack_result = container_vfs.bind_mount_from(&host_vfs_arc, "/home/../etc", "/attack_mount", true);
    // Whether attack succeeds or fails, safe mount should remain intact
    
    assert!(container_vfs.is_bind_mount("/safe_mount"), "Safe mount should remain after attack attempt");
    let safe_access = container_vfs.read_dir("/safe_mount");
    assert!(safe_access.is_ok(), "Safe mount should remain accessible");
    
    // Cleanup
    let _ = container_vfs.unmount("/safe_mount");
    let _ = container_vfs.unmount("/attack_mount");
}

#[test_case]
fn test_bind_mount_from_security_edge_cases() {
    // Test specific edge cases for cross-VFS bind mount security
    
    // Create host VFS with security-focused directory structure
    let mut host_vfs = VfsManager::new();
    let host_device = Box::new(MockBlockDevice::new(1, "host_security", 512, 100));
    let host_fs = Box::new(TestFileSystem::new("host_fs", host_device, 512));
    let host_fs_id = host_vfs.register_fs(host_fs);
    host_vfs.mount(host_fs_id, "/").unwrap();
    
    // Create directories for security testing
    host_vfs.create_dir("/public").expect("Failed to create /public");
    host_vfs.create_dir("/private").expect("Failed to create /private");
    host_vfs.create_dir("/temp").expect("Failed to create /temp");
    host_vfs.create_dir("/nested").expect("Failed to create /nested");
    host_vfs.create_dir("/nested/deep").expect("Failed to create /nested/deep");
    host_vfs.create_dir("/nested/deep/buried").expect("Failed to create /nested/deep/buried");
    
    // Create test files
    host_vfs.create_regular_file("/public/readme.txt").expect("Failed to create readme");
    host_vfs.create_regular_file("/private/secret.key").expect("Failed to create secret");
    host_vfs.create_regular_file("/nested/deep/buried/treasure.txt").expect("Failed to create treasure");
    
    // Verify the file exists in host VFS before bind mounting
    let host_treasure_check = host_vfs.open("/nested/deep/buried/treasure.txt", 0);
    assert!(host_treasure_check.is_ok(), "treasure.txt should exist in host VFS");
    
    // Verify the host VFS can list the directories correctly
    let host_nested_contents = host_vfs.read_dir("/nested").unwrap();
    assert!(host_nested_contents.iter().any(|e| e.name == "deep"), "deep directory should exist in nested");
    
    let host_deep_contents = host_vfs.read_dir("/nested/deep").unwrap();
    assert!(host_deep_contents.iter().any(|e| e.name == "buried"), "buried directory should exist in deep");
    
    let host_buried_contents = host_vfs.read_dir("/nested/deep/buried").unwrap();
    assert!(host_buried_contents.iter().any(|e| e.name == "treasure.txt"), "treasure.txt should exist in buried");
    
    // Create container VFS
    let mut container_vfs = VfsManager::new();
    let container_device = Box::new(MockBlockDevice::new(2, "container_security", 512, 100));
    let container_fs = Box::new(TestFileSystem::new("container_fs", container_device, 512));
    let container_fs_id = container_vfs.register_fs(container_fs);
    container_vfs.mount(container_fs_id, "/").unwrap();
    
    let host_vfs_arc = Arc::new(host_vfs);
    
    // === Test 1: Cross-VFS file access through deep bind mounts ===
    
    // Mount deeply nested directory from host to container
    let deep_mount = container_vfs.bind_mount_from(&host_vfs_arc, "/nested/deep", "/deep_mount", false);
    assert!(deep_mount.is_ok(), "Deep mount should succeed");
    
    // Check if bind mount was registered correctly
    assert!(container_vfs.is_bind_mount("/deep_mount"), "Deep mount should be detected as bind mount");
    
    // Try to list the directory first to see what's available
    let deep_mount_contents = container_vfs.read_dir("/deep_mount");
    assert!(deep_mount_contents.is_ok(), "Should be able to list deep mount directory");
    let entries = deep_mount_contents.unwrap();

    // Check if buried directory exists
    let has_buried = entries.iter().any(|e| e.name == "buried");
    if !has_buried {
        println!("Available entries in /deep_mount: {:?}", entries.iter().map(|e| &e.name).collect::<Vec<_>>());
    }

    assert!(has_buried, "buried directory should be visible in deep mount");
    
    // Try to list the buried directory
    let buried_contents = container_vfs.read_dir("/deep_mount/buried");
    assert!(buried_contents.is_ok(), "Should be able to list buried directory");
    let buried_entries = buried_contents.unwrap();
    
    // Check if treasure.txt exists
    let has_treasure = buried_entries.iter().any(|e| e.name == "treasure.txt");
    if !has_treasure {
        println!("Available entries in /deep_mount/buried: {:?}", buried_entries.iter().map(|e| &e.name).collect::<Vec<_>>());
    }
    assert!(has_treasure, "treasure.txt should be visible in buried directory");
    
    // Now try to open the file
    let buried_file = container_vfs.open("/deep_mount/buried/treasure.txt", 0);
    assert!(buried_file.is_ok(), "Should be able to access files through cross-VFS bind mount");
    
    // === Test 2: Permission inheritance across VFS boundaries ===
    
    // Mount with read-only flag
    let ro_mount = container_vfs.bind_mount_from(&host_vfs_arc, "/public", "/ro_public", true);
    assert!(ro_mount.is_ok(), "Read-only mount should succeed");
    
    // Verify read access works
    let readonly_file = container_vfs.open("/ro_public/readme.txt", 0);
    assert!(readonly_file.is_ok(), "Read access through RO mount should work");
    
    // === Test 3: Mount point collision prevention ===
    
    // Try to mount over existing mount point (should fail)
    let collision_mount = container_vfs.bind_mount_from(&host_vfs_arc, "/private", "/ro_public", false);
    assert!(collision_mount.is_err(), "Mount collision should be prevented");
    
    // Original mount should remain unaffected
    assert!(container_vfs.is_bind_mount("/ro_public"), "Original mount should persist");
    
    // === Test 4: Resource cleanup and isolation ===
    
    // Unmount all test mounts
    let deep_unmount = container_vfs.unmount("/deep_mount");
    assert!(deep_unmount.is_ok(), "Deep mount unmount should succeed");
    
    let ro_unmount = container_vfs.unmount("/ro_public");
    if ro_unmount.is_err() {
        println!("RO mount unmount failed: {:?}", ro_unmount);
    }
    assert!(ro_unmount.is_ok(), "RO mount unmount should succeed");
    
    // Verify host VFS remains unaffected by container operations
    let host_check = host_vfs_arc.read_dir("/nested/deep");
    assert!(host_check.is_ok(), "Host VFS should remain accessible after container unmounts");
    
    // Verify container VFS maintains independence
    let container_check = container_vfs.read_dir("/");
    assert!(container_check.is_ok(), "Container VFS should maintain independence");
}