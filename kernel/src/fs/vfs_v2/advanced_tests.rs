/// Advanced VFS v2 tests for complex scenarios

use alloc::sync::Arc;
use crate::fs::vfs_v2::{ manager::VfsManager, tmpfs::TmpFS, }; use crate::fs::FileType;


#[test_case]
fn test_basic_bind_mount() {
    // 1. Setup: two filesystems (tmpfs) mounted at /fs1 and /fs2
    let vfs_manager = VfsManager::new();
    let tmpfs1: Arc<dyn crate::fs::vfs_v2::core::FileSystemOperations> = TmpFS::new(1024 * 1024);
    let tmpfs2: Arc<dyn crate::fs::vfs_v2::core::FileSystemOperations> = TmpFS::new(1024 * 1024);

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
    let mut file_obj = vfs.open("/fs2/mount_a/nested_mount/file_b", 0).unwrap();
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
    assert!(result.is_err());
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
    assert!(result.is_err());
}

#[test_case]
fn test_bind_mount_path_traversal() {
    // 1. Setup
    let vfs = VfsManager::new();
    let tmpfs1 = TmpFS::new(0);
    let tmpfs2 = TmpFS::new(0);

    // Create a secret file in the root fs
    vfs.create_file("/root_secret.txt", FileType::RegularFile).unwrap();

    vfs.create_dir("/fs1").unwrap();
    vfs.create_dir("/fs2").unwrap();
    vfs.mount(tmpfs1, "/fs1", 0).unwrap();
    vfs.mount(tmpfs2, "/fs2", 0).unwrap();

    vfs.create_dir("/fs1/dir_a").unwrap();
    vfs.create_dir("/fs2/mount_point").unwrap();

    // 2. Bind mount /fs1/dir_a onto /fs2/mount_point
    vfs.bind_mount("/fs1/dir_a", "/fs2/mount_point").unwrap();

    // 3. Attempt to traverse out of the bind mount
    // This path should resolve to /fs2/root_secret.txt, which does not exist.
    // It must not resolve to /root_secret.txt
    let result = vfs.open("/fs2/mount_point/../../root_secret.txt", 0);

    // 4. Verification
    assert!(result.is_err(), "Path traversal vulnerability detected!");
}