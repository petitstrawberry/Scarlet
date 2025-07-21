//! Test for reproducing the symlink cross-filesystem bug
//!
//! This test reproduces the issue where:
//! - rootfs is cpiofs with tmpfs mounted at /tmp
//! - /symlink is a link to /foo directory
//! - /foo/bar exists
//! - Opening /symlink/bar, reading content, and writing to /tmp/bar fails

use crate::fs::vfs_v2::{
    drivers::{tmpfs::TmpFS, cpiofs::CpioFS},
    manager::{VfsManager, PathResolutionOptions},
};
use crate::fs::FileType;
use alloc::{vec::Vec, string::ToString};

#[test_case]
fn test_symlink_cross_filesystem_file_operations() {
    // Create test CPIO data with a directory structure and symlink
    let cpio_data = create_test_cpio_with_symlink();
    
    // Create CpioFS as root filesystem
    let cpiofs = CpioFS::new("test_cpiofs".to_string(), &cpio_data).expect("Failed to create CpioFS");
    let vfs = VfsManager::new_with_root(cpiofs);
    
    // Mount tmpfs at /tmp (/tmp already exists in CPIO)
    let tmpfs = TmpFS::new(1024 * 1024); // 1MB limit
    vfs.mount(tmpfs, "/tmp", 0).expect("Failed to mount tmpfs at /tmp");
    
    // Verify the symlink exists and points to the correct target
    let (symlink_entry, _) = vfs.resolve_path_with_options("/symlink", &PathResolutionOptions::no_follow())
        .expect("Failed to resolve symlink");
    assert!(symlink_entry.node().is_symlink().expect("Failed to check if symlink"));
    
    let target = symlink_entry.node().read_link().expect("Failed to read symlink target");
    assert_eq!(target, "/foo");
    
    // Verify that /foo/bar exists and can be read through symlink
    let _file_through_symlink = vfs.resolve_path("/symlink/bar").expect("Failed to resolve /symlink/bar");
    
    // Read content from the source file through symlink using VFS open
    let source_content = {
        let file_obj = vfs.open("/symlink/bar", 0x01).expect("Failed to open source file through symlink"); // Read mode
        
        if let crate::object::KernelObject::File(file_obj) = file_obj {
            let mut buffer = Vec::new();
            let mut temp_buf = [0u8; 1024];
            loop {
                match file_obj.read(&mut temp_buf) {
                    Ok(0) => break, // EOF
                    Ok(bytes_read) => {
                        buffer.extend_from_slice(&temp_buf[..bytes_read]);
                    }
                    Err(_) => panic!("Failed to read source file"),
                }
            }
            buffer
        } else {
            panic!("Expected file object");
        }
    };
    
    // Verify we got some content
    assert!(!source_content.is_empty(), "Source file should not be empty");
    
    // Create destination file in tmpfs
    vfs.create_file("/tmp/bar", FileType::RegularFile).expect("Failed to create /tmp/bar");
    
    // Write content to destination file
    {
        let dest_file = vfs.open("/tmp/bar", 0x02).expect("Failed to open destination file for writing"); // Write mode
        
        if let crate::object::KernelObject::File(dest_file_obj) = dest_file {
            dest_file_obj.write(&source_content).expect("Failed to write to destination file");
        } else {
            panic!("Expected file object");
        }
    }
    
    // Try to read back from destination file to verify it worked
    {
        let dest_file = vfs.open("/tmp/bar", 0x01).expect("Failed to open destination file for reading"); // Read mode
        
        if let crate::object::KernelObject::File(dest_file_obj) = dest_file {
            let mut read_buffer = Vec::new();
            let mut temp_buf = [0u8; 1024];
            loop {
                match dest_file_obj.read(&mut temp_buf) {
                    Ok(0) => break, // EOF
                    Ok(bytes_read) => {
                        read_buffer.extend_from_slice(&temp_buf[..bytes_read]);
                    }
                    Err(_) => panic!("Failed to read back from destination"),
                }
            }
            
            assert_eq!(source_content, read_buffer, "Content mismatch between source and destination");
        } else {
            panic!("Expected file object");
        }
    }
}

#[test_case]
fn test_copy_operation_cross_filesystem() {
    // This test simulates the exact copy_file operation from init.rs
    let cpio_data = create_test_cpio_with_symlink();
    
    let cpiofs = CpioFS::new("test_cpiofs".to_string(), &cpio_data).expect("Failed to create CpioFS");
    let vfs = VfsManager::new_with_root(cpiofs);
    
    // Mount tmpfs at /tmp (/tmp already exists in CPIO)
    let tmpfs = TmpFS::new(1024 * 1024); // 1MB limit
    vfs.mount(tmpfs, "/tmp", 0).expect("Failed to mount tmpfs at /tmp");
    
    // Simulate copy_file("/symlink/bar", "/tmp/bar")
    assert!(copy_file_vfs(&vfs, "/symlink/bar", "/tmp/bar"));
}

fn copy_file_vfs(vfs: &VfsManager, src: &str, dest: &str) -> bool {
    // This mirrors the copy_file function in init.rs with detailed debugging
    
    // Read source file
    let src_file = match vfs.open(src, 0x01) { // Read mode
        Ok(file) => file,
        Err(_) => {
            return false;
        }
    };
    
    let src_file_obj = if let crate::object::KernelObject::File(file_obj) = src_file {
        file_obj
    } else {
        return false;
    };
    
    // Create destination file
    if let Err(_) = vfs.create_file(dest, FileType::RegularFile) {
        return false;
    }
    
    // Verify the file exists after creation
    if vfs.resolve_path(dest).is_err() {
        return false; // File creation failed
    }
    
    // Try to open the destination file for writing
    let dest_file = match vfs.open(dest, 0x02) { // Write mode
        Ok(file) => file,
        Err(_) => {
            return false; // This is where the bug might occur!
        }
    };
    
    let dest_file_obj = if let crate::object::KernelObject::File(file_obj) = dest_file {
        file_obj
    } else {
        return false;
    };
    
    // Copy data
    let mut buffer = [0u8; 4096];
    loop {
        match src_file_obj.read(&mut buffer) {
            Ok(0) => break, // EOF
            Ok(bytes_read) => {
                match dest_file_obj.write(&buffer[..bytes_read]) {
                    Ok(bytes_written) if bytes_written == bytes_read => {
                        // Success, continue
                    }
                    _ => {
                        return false;
                    }
                }
            }
            Err(_) => {
                return false;
            }
        }
    }
    
    // Test: Try to open the file again for reading to verify it works
    let _verify_file = match vfs.open(dest, 0x01) { // Read mode
        Ok(file) => file,
        Err(_) => {
            return false; // This might also fail
        }
    };
    
    true
}

/// Create CPIO data with a symlink pointing to a directory containing a file
fn create_test_cpio_with_symlink() -> Vec<u8> {
    let mut cpio_data = Vec::new();
    
    // Create /tmp directory (empty, will be used as mount point)
    let tmp_dir_name = b"tmp\0";
    
    // CPIO header for /tmp directory (mode: 0o040755)
    cpio_data.extend_from_slice(b"070701");           // magic
    cpio_data.extend_from_slice(b"00000001");         // inode
    cpio_data.extend_from_slice(b"000041ed");         // mode (0o040755)
    cpio_data.extend_from_slice(b"00000000");         // uid
    cpio_data.extend_from_slice(b"00000000");         // gid
    cpio_data.extend_from_slice(b"00000002");         // nlink
    cpio_data.extend_from_slice(b"00000000");         // mtime
    cpio_data.extend_from_slice(b"00000000");         // filesize (0 for directory)
    cpio_data.extend_from_slice(b"00000000");         // dev_maj
    cpio_data.extend_from_slice(b"00000000");         // dev_min
    cpio_data.extend_from_slice(b"00000000");         // rdev_maj
    cpio_data.extend_from_slice(b"00000000");         // rdev_min
    cpio_data.extend_from_slice(b"00000004");         // namesize (4)
    cpio_data.extend_from_slice(b"00000000");         // checksum
    
    // Directory name (padded to 4-byte boundary)
    cpio_data.extend_from_slice(tmp_dir_name);
    // Add padding to align to 4-byte boundary
    while cpio_data.len() % 4 != 0 {
        cpio_data.push(0);
    }
    
    // Create /foo directory
    let dir_name = b"foo\0";
    
    // CPIO header for directory (mode: 0o040755)
    cpio_data.extend_from_slice(b"070701");           // magic
    cpio_data.extend_from_slice(b"00000002");         // inode
    cpio_data.extend_from_slice(b"000041ed");         // mode (0o040755)
    cpio_data.extend_from_slice(b"00000000");         // uid
    cpio_data.extend_from_slice(b"00000000");         // gid
    cpio_data.extend_from_slice(b"00000002");         // nlink
    cpio_data.extend_from_slice(b"00000000");         // mtime
    cpio_data.extend_from_slice(b"00000000");         // filesize (0 for directory)
    cpio_data.extend_from_slice(b"00000000");         // dev_maj
    cpio_data.extend_from_slice(b"00000000");         // dev_min
    cpio_data.extend_from_slice(b"00000000");         // rdev_maj
    cpio_data.extend_from_slice(b"00000000");         // rdev_min
    cpio_data.extend_from_slice(b"00000004");         // namesize (4)
    cpio_data.extend_from_slice(b"00000000");         // checksum
    
    // Directory name (padded to 4-byte boundary)
    cpio_data.extend_from_slice(dir_name);
    // Add padding to align to 4-byte boundary
    while cpio_data.len() % 4 != 0 {
        cpio_data.push(0);
    }
    
    // Create /foo/bar file
    let file_content = b"This is test content for /foo/bar file";
    let file_name = b"foo/bar\0";
    
    // CPIO header for regular file (mode: 0o100644)
    cpio_data.extend_from_slice(b"070701");           // magic
    cpio_data.extend_from_slice(b"00000003");         // inode
    cpio_data.extend_from_slice(b"000081a4");         // mode (0o100644)
    cpio_data.extend_from_slice(b"00000000");         // uid
    cpio_data.extend_from_slice(b"00000000");         // gid
    cpio_data.extend_from_slice(b"00000001");         // nlink
    cpio_data.extend_from_slice(b"00000000");         // mtime
    cpio_data.extend_from_slice(b"00000027");         // filesize (39)
    cpio_data.extend_from_slice(b"00000000");         // dev_maj
    cpio_data.extend_from_slice(b"00000000");         // dev_min
    cpio_data.extend_from_slice(b"00000000");         // rdev_maj
    cpio_data.extend_from_slice(b"00000000");         // rdev_min
    cpio_data.extend_from_slice(b"00000008");         // namesize (8)
    cpio_data.extend_from_slice(b"00000000");         // checksum
    
    // File name (padded to 4-byte boundary)
    cpio_data.extend_from_slice(file_name);
    // Add padding to align to 4-byte boundary
    while cpio_data.len() % 4 != 0 {
        cpio_data.push(0);
    }
    
    // File content (padded to 4-byte boundary)
    cpio_data.extend_from_slice(file_content);
    // Add padding to align to 4-byte boundary
    while cpio_data.len() % 4 != 0 {
        cpio_data.push(0);
    }
    
    // Create /symlink -> /foo
    let symlink_target = b"/foo";
    let symlink_name = b"symlink\0";
    
    // CPIO header for symbolic link (mode: 0o120777)
    cpio_data.extend_from_slice(b"070701");           // magic
    cpio_data.extend_from_slice(b"00000004");         // inode
    cpio_data.extend_from_slice(b"0000a1ff");         // mode (0o120777)
    cpio_data.extend_from_slice(b"00000000");         // uid
    cpio_data.extend_from_slice(b"00000000");         // gid
    cpio_data.extend_from_slice(b"00000001");         // nlink
    cpio_data.extend_from_slice(b"00000000");         // mtime
    cpio_data.extend_from_slice(b"00000004");         // filesize (4 - length of "/foo")
    cpio_data.extend_from_slice(b"00000000");         // dev_maj
    cpio_data.extend_from_slice(b"00000000");         // dev_min
    cpio_data.extend_from_slice(b"00000000");         // rdev_maj
    cpio_data.extend_from_slice(b"00000000");         // rdev_min
    cpio_data.extend_from_slice(b"00000008");         // namesize (8)
    cpio_data.extend_from_slice(b"00000000");         // checksum
    
    // Symlink name (padded to 4-byte boundary)
    cpio_data.extend_from_slice(symlink_name);
    // Add padding to align to 4-byte boundary
    while cpio_data.len() % 4 != 0 {
        cpio_data.push(0);
    }
    
    // Symlink target (padded to 4-byte boundary)
    cpio_data.extend_from_slice(symlink_target);
    // Add padding to align to 4-byte boundary
    while cpio_data.len() % 4 != 0 {
        cpio_data.push(0);
    }
    
    // TRAILER!!! entry to mark end of archive
    let trailer_name = b"TRAILER!!!\0";
    
    // CPIO header for TRAILER!!!
    cpio_data.extend_from_slice(b"070701");           // magic
    cpio_data.extend_from_slice(b"00000000");         // inode
    cpio_data.extend_from_slice(b"00000000");         // mode
    cpio_data.extend_from_slice(b"00000000");         // uid
    cpio_data.extend_from_slice(b"00000000");         // gid
    cpio_data.extend_from_slice(b"00000000");         // nlink
    cpio_data.extend_from_slice(b"00000000");         // mtime
    cpio_data.extend_from_slice(b"00000000");         // filesize
    cpio_data.extend_from_slice(b"00000000");         // dev_maj
    cpio_data.extend_from_slice(b"00000000");         // dev_min
    cpio_data.extend_from_slice(b"00000000");         // rdev_maj
    cpio_data.extend_from_slice(b"00000000");         // rdev_min
    cpio_data.extend_from_slice(b"0000000b");         // namesize (11)
    cpio_data.extend_from_slice(b"00000000");         // checksum
    
    // TRAILER!!! name (padded to 4-byte boundary)
    cpio_data.extend_from_slice(trailer_name);
    // Add padding to align to 4-byte boundary
    while cpio_data.len() % 4 != 0 {
        cpio_data.push(0);
    }
    
    cpio_data
}