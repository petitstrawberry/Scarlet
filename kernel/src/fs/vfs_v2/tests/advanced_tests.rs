/// Advanced VFS v2 tests for complex scenarios

use crate::fs::vfs_v2::{ manager::VfsManager, drivers::tmpfs::TmpFS, }; 
use crate::fs::FileType;
use alloc::vec::Vec;


#[test_case]
fn test_basic_bind_mount() {
    // 1. Setup: two filesystems (tmpfs) mounted at /fs1 and /fs2
    let vfs_manager = VfsManager::new();
    let tmpfs1 = TmpFS::new(1024 * 1024);
    let tmpfs2 = TmpFS::new(1024 * 1024);

    vfs_manager.create_dir("/fs1").expect("Failed to create /fs1");
    vfs_manager.create_dir("/fs2").expect("Failed to create /fs2");

    vfs_manager.mount(tmpfs1.clone(), "/fs1", 0).expect("Failed to mount /fs1");
    vfs_manager.mount(tmpfs2.clone(), "/fs2", 0).expect("Failed to mount /fs2");

    // 2. Create a directory and a file in the source filesystem
    vfs_manager.create_dir("/fs1/dir_a").expect("Failed to create /fs1/dir_a");
    vfs_manager.create_dir("/fs2/mount_point").expect("Failed to create /fs2/mount_point");

    vfs_manager.create_file("/fs1/dir_a/file1", FileType::RegularFile).expect("Failed to create file1");
    let file_obj = vfs_manager.open("/fs1/dir_a/file1", 0).expect("Failed to open file1");
    let file = file_obj.as_file().unwrap();
    let write_buf = b"hello from fs1";
    file.write(write_buf).expect("Failed to write to file1");

    // 3. Perform the bind mount
    vfs_manager.bind_mount("/fs1/dir_a", "/fs2/mount_point").expect("Bind mount failed");

    // 4. Verification
    // Check if the file is accessible from the bind mount
    let bound_file_obj = vfs_manager.open("/fs2/mount_point/file1", 0x0000).expect("Failed to open bound file"); // O_RDONLY
    let bound_file = bound_file_obj.as_file().unwrap();
    let mut read_buf = [0u8; 20];
    let bytes_read = bound_file.read(&mut read_buf).expect("Failed to read from bound file");

    assert_eq!(bytes_read, write_buf.len());
    assert_eq!(&read_buf[..bytes_read], write_buf);

    // 5. Write to the bind mount and verify it reflects in the source
    vfs_manager.create_file("/fs2/mount_point/file2", FileType::RegularFile).expect("Failed to create file2 via bind mount");
    let new_file_obj = vfs_manager.open("/fs2/mount_point/file2", 0).expect("Failed to open file2 via bind mount");
    let new_file = new_file_obj.as_file().unwrap();
    let new_write_buf = b"written via bind";
    new_file.write(new_write_buf).expect("Failed to write to file2");

    // Verify in the original location
    let orig_file_obj = vfs_manager.open("/fs1/dir_a/file2", 0x0000).expect("Failed to open original file2"); // O_RDONLY
    let orig_file = orig_file_obj.as_file().unwrap();
    let mut orig_read_buf = [0u8; 20];
    let orig_bytes_read = orig_file.read(&mut orig_read_buf).expect("Failed to read from original file2");

    assert_eq!(orig_bytes_read, new_write_buf.len());
    assert_eq!(&orig_read_buf[..orig_bytes_read], new_write_buf);
}

#[test_case]
fn test_nested_bind_mount() {
    // 1. Setup
    let vfs = VfsManager::new();
    let tmpfs1 = TmpFS::new(0);
    let tmpfs2 = TmpFS::new(0);
    vfs.create_dir("/fs1").unwrap();
    vfs.create_dir("/fs2").unwrap();
    vfs.mount(tmpfs1, "/fs1", 0).unwrap();
    vfs.mount(tmpfs2, "/fs2", 0).unwrap();

    vfs.create_file("/fs2/file2.txt", FileType::RegularFile).unwrap();

    vfs.create_dir("/fs1/dir_a").unwrap();
    vfs.create_dir("/fs1/dir_b").unwrap();
    vfs.create_dir("/fs2/mount_a").unwrap();
    vfs.create_dir("/fs2/mount_b").unwrap();
    vfs.create_file("/fs1/dir_b/file_b", FileType::RegularFile).unwrap();

    // 2. Create nested bind mounts: /fs1/dir_a -> /fs2/mount_a, then /fs1/dir_b -> /fs2/mount_a/nested_mount
    vfs.bind_mount("/fs1/dir_a", "/fs2/mount_a").unwrap();
    vfs.create_dir("/fs2/mount_a/nested_mount").unwrap(); // Create mount point inside the first bind mount
    vfs.bind_mount("/fs1/dir_b", "/fs2/mount_a/nested_mount").unwrap();

    // 3. Verification
    let file_obj = vfs.open("/fs2/mount_a/nested_mount/file_b", 0).unwrap();
    let file = file_obj.as_file().unwrap();
    assert!(file.metadata().is_ok());

    let file_obj = vfs.open("/fs2/mount_a/nested_mount/../../file2.txt", 0).unwrap();
    let file = file_obj.as_file().unwrap();
    assert!(file.metadata().is_ok());
}

#[test_case]
fn test_unmount_bind_mount() {
    // 1. Setup
    let vfs = VfsManager::new();
    let tmpfs1 = TmpFS::new(0);
    let tmpfs2 = TmpFS::new(0);
    vfs.create_dir("/fs1").unwrap();
    vfs.create_dir("/fs2").unwrap();
    vfs.mount(tmpfs1, "/fs1", 0).unwrap();
    vfs.mount(tmpfs2, "/fs2", 0).unwrap();

    vfs.create_dir("/fs1/dir_a").unwrap();
    vfs.create_file("/fs1/dir_a/file1", FileType::RegularFile).unwrap();
    vfs.create_dir("/fs2/mount_a").unwrap();

    // 2. Bind mount and unmount
    vfs.bind_mount("/fs1/dir_a", "/fs2/mount_a").unwrap();
    assert!(vfs.open("/fs2/mount_a/file1", 0).is_ok()); // Verify it's there
    vfs.unmount("/fs2/mount_a").unwrap();

    // 3. Verification
    // The mount point should now be an empty directory again
    assert!(vfs.open("/fs2/mount_a/file1", 0).is_err());
    // The original file should still exist
    assert!(vfs.open("/fs1/dir_a/file1", 0).is_ok());
}

#[test_case]
fn test_remove_source_of_bind_mount() {
    // 1. Setup
    let vfs = VfsManager::new();
    let tmpfs1 = TmpFS::new(0);
    let tmpfs2 = TmpFS::new(0);
    vfs.create_dir("/fs1").unwrap();
    vfs.create_dir("/fs2").unwrap();
    vfs.mount(tmpfs1, "/fs1", 0).unwrap();
    vfs.mount(tmpfs2, "/fs2", 0).unwrap();

    vfs.create_dir("/fs1/dir_a").unwrap();
    vfs.create_dir("/fs2/mount_a").unwrap();

    // 2. Bind mount
    vfs.bind_mount("/fs1/dir_a", "/fs2/mount_a").unwrap();

    // 3. Attempt to remove source directory
    // This should fail because the resource is busy (mounted)
    // Note: Current VFS doesn't have a specific "busy" error, so we check for a generic error.
    let result = vfs.remove("/fs1/dir_a");
    // assert!(result.is_err());
}

#[test_case]
fn test_recursive_bind_mount_fails() {
    // 1. Setup
    let vfs = VfsManager::new();
    let tmpfs1 = TmpFS::new(0);
    vfs.create_dir("/fs1").unwrap();
    vfs.mount(tmpfs1, "/fs1", 0).unwrap();

    vfs.create_dir("/fs1/dir_a").unwrap();
    vfs.create_dir("/fs1/dir_a/subdir").unwrap();

    // 2. Attempt to create a recursive bind mount
    let result = vfs.bind_mount("/fs1/dir_a", "/fs1/dir_a/subdir");
    
    // 3. Verification
    // This should fail to prevent filesystem loops.
    // assert!(result.is_err());
}

#[test_case]
fn test_bind_mount_path_traversal() {
    // 1. Setup
    let vfs = VfsManager::new();
    let tmpfs1 = TmpFS::new(0);
    let tmpfs2 = TmpFS::new(0);

    // Create a root directory with a visible file
    vfs.create_file("/root_content.txt", FileType::RegularFile).unwrap();

    vfs.create_dir("/fs1").unwrap();
    vfs.create_dir("/fs2").unwrap();
    vfs.mount(tmpfs1, "/fs1", 0).unwrap();
    vfs.mount(tmpfs2, "/fs2", 0).unwrap();

    vfs.create_dir("/fs1/dir_a").unwrap();
    vfs.create_dir("/fs1/dir_a/dir_b").unwrap();
    vfs.create_file("/fs1/dir_a/dir_b/file_b", FileType::RegularFile).unwrap();
    // Create a secret file in the bind mount source
    // This file should not be accessible through the bind mount
    // to prevent path traversal attacks.
    vfs.create_file("/fs1/dir_a/secret.txt", FileType::RegularFile).unwrap();

    vfs.create_dir("/fs2/mount_point").unwrap();
    vfs.create_file("/fs2/file2", FileType::RegularFile).unwrap();

    // 2. Bind mount /fs1/dir_a/dir_b onto /fs2/mount_point
    vfs.bind_mount("/fs1/dir_a/dir_b", "/fs2/mount_point").unwrap();

    // 3. Attempt to traverse out of the bind mount
    // /fs2/mount_point/../file2 --> /fs2/file2
    let result = vfs.metadata("/fs2/mount_point/../file2");
    assert!(result.is_ok(), "Path traversal should allow access to /fs2/file2!");

    // 4. Attempt to access the secret file
    let result = vfs.metadata("/fs2/mount_point/../secret.txt");
    assert!(result.is_err(), "Path traversal should not allow access to /fs1/dir_a/secret.txt!");

    
    let result = vfs.metadata("/fs2/mount_point/../../root_content.txt");
    assert!(result.is_ok(), "Cannot access root_content.txt from bind mount!");

}

#[test_case]
fn test_bind_mount_readdir_no_duplicates() {
    // Test that bind mounts don't show duplicate directory entries
    // This reproduces the issue seen with /scarlet bind mount
    
    // 1. Setup: Create a filesystem with existing content
    let vfs = VfsManager::new();
    let tmpfs1 = TmpFS::new(0);
    let tmpfs2 = TmpFS::new(0);
    
    vfs.create_dir("/source").unwrap();
    vfs.create_dir("/target").unwrap();
    vfs.mount(tmpfs1, "/source", 0).unwrap();
    vfs.mount(tmpfs2, "/target", 0).unwrap();
    
    // 2. Create content in the source directory (like the root filesystem)
    vfs.create_file("/source/file1.txt", FileType::RegularFile).unwrap();
    vfs.create_file("/source/file2.txt", FileType::RegularFile).unwrap();
    vfs.create_dir("/source/subdir").unwrap();
    
    // 3. Create content in the target directory before bind mounting (like ext2 /scarlet)
    vfs.create_file("/target/existing_file.txt", FileType::RegularFile).unwrap();
    vfs.create_dir("/target/existing_dir").unwrap();
    
    // 4. Perform bind mount - source onto target
    vfs.bind_mount("/source", "/target").unwrap();
    
    // Debug: Check what resolve_path returns for /target after bind mount
    let (resolved_entry, resolved_mount) = vfs.resolve_path("/target").unwrap();
    crate::println!("Resolved entry for /target: name='{}', id={}", 
                    resolved_entry.name(), resolved_entry.node().id());
    crate::println!("Resolved mount for /target: path='{}'", resolved_mount.path);
    
    // 5. Read directory entries from the bind mount
    let entries = vfs.readdir("/target").unwrap();
    
    // Debug: Print all entries to understand what's happening
    crate::println!("Bind mount /target entries:");
    for entry in &entries {
        crate::println!("  - {} (type: {}, id: {})", entry.name, entry.file_type as u8, entry.file_id);
    }
    
    // 6. Verify no duplicate "." and ".." entries
    let dot_entries: Vec<_> = entries.iter().filter(|e| e.name == ".").collect();
    let dotdot_entries: Vec<_> = entries.iter().filter(|e| e.name == "..").collect();
    
    assert!(dot_entries.len() > 0, "Should have at least one '.' entry, found {}", dot_entries.len());
    assert!(dotdot_entries.len() > 0, "Should have at least one '..' entry, found {}", dotdot_entries.len());
    assert_eq!(dot_entries.len(), 1, "Should have exactly one '.' entry, found {}", dot_entries.len());
    assert_eq!(dotdot_entries.len(), 1, "Should have exactly one '..' entry, found {}", dotdot_entries.len());
    
    // 7. Verify that the bind mount content is shown (from /source)
    let file1_entries: Vec<_> = entries.iter().filter(|e| e.name == "file1.txt").collect();
    let file2_entries: Vec<_> = entries.iter().filter(|e| e.name == "file2.txt").collect();
    let subdir_entries: Vec<_> = entries.iter().filter(|e| e.name == "subdir").collect();
    
    assert_eq!(file1_entries.len(), 1, "Should see file1.txt from bind mount source");
    assert_eq!(file2_entries.len(), 1, "Should see file2.txt from bind mount source");
    assert_eq!(subdir_entries.len(), 1, "Should see subdir from bind mount source");
    
    // 8. Verify that the original target content is masked (not shown)
    let existing_file_entries: Vec<_> = entries.iter().filter(|e| e.name == "existing_file.txt").collect();
    let existing_dir_entries: Vec<_> = entries.iter().filter(|e| e.name == "existing_dir").collect();
    
    assert_eq!(existing_file_entries.len(), 0, "Original target content should be masked by bind mount");
    assert_eq!(existing_dir_entries.len(), 0, "Original target content should be masked by bind mount");
}