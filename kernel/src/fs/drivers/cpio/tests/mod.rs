use crate::fs::{
    FileSystemErrorKind, FileType,
};

use super::*;

use alloc::vec;

#[test_case]
fn test_initramfs_mount_and_unmount() {
    let cpio_data = include_bytes!("mkfs/initramfs.cpio"); // Test CPIO data
    let mut initramfs = Cpiofs::new( "initramfs", cpio_data).unwrap();

    // Check the state before mounting
    assert_eq!(initramfs.mounted, false);

    // Perform mounting
    initramfs.mount("/").unwrap();
    assert_eq!(initramfs.mounted, true);
    assert_eq!(initramfs.mount_point, "/");

    // Perform unmounting
    initramfs.unmount().unwrap();
    assert_eq!(initramfs.mounted, false);
    assert_eq!(initramfs.mount_point, "");
}

#[test_case]
fn test_initramfs_read_dir() {
    let cpio_data = include_bytes!("mkfs/initramfs.cpio"); // Test CPIO data
    let initramfs = Cpiofs::new( "initramfs", cpio_data).unwrap();

    // Read the contents of the root directory
    let entries = initramfs.read_dir("/").unwrap();

    // Verify the entries
    assert!(entries.iter().any(|e| e.name == "file1.txt"));
    assert!(entries.iter().any(|e| e.name == "file2.txt"));
}

#[test_case]
fn test_initramfs_open_file() {
    let cpio_data = include_bytes!("mkfs/initramfs.cpio"); // Test CPIO data
    let initramfs = Cpiofs::new( "initramfs", cpio_data).unwrap();

    // Open a file
    let file_handle = initramfs.open("/file1.txt", 0).unwrap();

    // Verify the file metadata
    let metadata = file_handle.metadata().unwrap();
    assert_eq!(metadata.file_type, FileType::RegularFile);
    assert_eq!(metadata.size, 13);
}

#[test_case]
fn test_initramfs_read_file() {
    let cpio_data = include_bytes!("mkfs/initramfs.cpio"); // Test CPIO data
    let initramfs = Cpiofs::new( "initramfs", cpio_data).unwrap();

    // Open a file
    let mut file_handle = initramfs.open("/file1.txt", 0).unwrap();

    // Read the contents of the file
    let mut buffer = vec![0u8; 512];
    let bytes_read = file_handle.read(&mut buffer).unwrap();

    // Verify the read data
    assert_eq!(bytes_read, 13);
    assert_eq!(&buffer[..bytes_read], b"Hello, world!"); // Example: File content is "Hello, world!"
}

#[test_case]
fn test_initramfs_read_only_operations() {
    let cpio_data = include_bytes!("mkfs/initramfs.cpio"); // Test CPIO data
    let initramfs = Cpiofs::new( "initramfs", cpio_data).unwrap();

    // Attempt a write operation
    let result = initramfs.create_file("/new_file.txt", FileType::RegularFile);
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().kind, FileSystemErrorKind::ReadOnly);

    // Attempt to create a directory
    let result = initramfs.create_dir("/new_dir");
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().kind, FileSystemErrorKind::ReadOnly);

    // Attempt to delete a file
    let result = initramfs.remove("/file1.txt");
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().kind, FileSystemErrorKind::ReadOnly);
}

#[test_case]
fn test_initramfs_metadata() {
    let cpio_data = include_bytes!("mkfs/initramfs.cpio"); // Test CPIO data
    let initramfs = Cpiofs::new( "initramfs", cpio_data).unwrap();

    // Retrieve file metadata
    let metadata = initramfs.metadata("/file1.txt").unwrap();

    // Verify the metadata content
    assert_eq!(metadata.file_type, FileType::RegularFile);
    assert_eq!(metadata.size, 13);
    assert!(metadata.permissions.read);
    assert!(!metadata.permissions.write);
    assert!(!metadata.permissions.execute);
}

#[test_case]
fn test_read_dir() {
    let cpio_data = include_bytes!("mkfs/initramfs.cpio"); // Test CPIO data
    let initramfs = Cpiofs::new( "initramfs", cpio_data).unwrap();

    // Get entries in the root directory
    let root_entries = initramfs.read_dir("/").unwrap();
    assert!(!root_entries.is_empty());
    assert!(root_entries.iter().any(|e| e.name == "file1.txt"));

    // Get entries in a subdirectory
    let subdir_entries = initramfs.read_dir("/subdir").unwrap();
    assert!(subdir_entries.iter().any(|e| e.name == "file1.txt"));

    let subsubdir_entries = initramfs.read_dir("/subdir/subsubdir").unwrap();
    assert!(subsubdir_entries.iter().any(|e| e.name == "file1.txt"));

    // Specify a nonexistent directory
    let result = initramfs.read_dir("/nonexistent");
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().kind, FileSystemErrorKind::NotFound);
}