//! Cross-VFS Bind Mount Tests


#[test_case]
fn test_cross_vfs_bind_mount_basic() {
    use crate::fs::FileType;
    use crate::fs::manager::VfsManager;
    use alloc::sync::Arc;

    // Create the source VFS and prepare a directory and file
    let source_vfs = Arc::new(VfsManager::new());
    source_vfs.create_dir("/srcdir").unwrap();
    source_vfs.create_file("/srcdir/file.txt", FileType::RegularFile).unwrap();

    // Create the target VFS
    let target_vfs = Arc::new(VfsManager::new());
    target_vfs.create_dir("/mnt").unwrap();
    target_vfs.create_file("/root_file.txt", FileType::RegularFile).unwrap();

    // Perform cross-vfs bind mount
    target_vfs.bind_mount_from(
        &source_vfs,
        "/srcdir",
        "/mnt",
    ).expect("cross-vfs bind mount failed");

    // Check if files under the bind mount can be accessed from the target side
    let entry  = target_vfs.resolve_path("/mnt/file.txt").expect("resolve_path failed");
    assert_eq!(entry.name(), "file.txt");

    // Check that .. from the bind mount root does not escape ("/mnt/.." should return to the parent on the target side)
    let entry = target_vfs.resolve_path("/mnt/../root_file.txt").expect("resolve_path failed");
    assert_eq!(entry.name(), "root_file.txt");
}

#[test_case]
fn test_cross_vfs_bind_mount_file_create_delete() {
    use crate::fs::FileType;
    use crate::fs::manager::VfsManager;
    use alloc::sync::Arc;

    let source_vfs = Arc::new(VfsManager::new());
    source_vfs.create_dir("/srcdir").unwrap();
    let target_vfs = Arc::new(VfsManager::new());
    target_vfs.create_dir("/mnt").unwrap();
    target_vfs.bind_mount_from(&source_vfs, "/srcdir", "/mnt").unwrap();

    // Create file from target side
    target_vfs.create_file("/mnt/newfile.txt", FileType::RegularFile).unwrap();
    // Should be visible from source side
    let entry = source_vfs.resolve_path("/srcdir/newfile.txt").expect("file not visible from source");
    assert_eq!(entry.name(), "newfile.txt");

    // Delete file from source side
    source_vfs.remove("/srcdir/newfile.txt").unwrap();
    // Should not be visible from target side
    assert!(target_vfs.resolve_path("/mnt/newfile.txt").is_err());
}

#[test_case]
fn test_cross_vfs_bind_mount_recursive() {
    use crate::fs::FileType;
    use crate::fs::manager::VfsManager;
    use alloc::sync::Arc;

    let source_vfs = Arc::new(VfsManager::new());
    source_vfs.create_dir("/a").unwrap();
    source_vfs.create_dir("/a/b").unwrap();
    source_vfs.create_file("/a/b/file.txt", FileType::RegularFile).unwrap();
    let target_vfs = Arc::new(VfsManager::new());
    target_vfs.create_dir("/mnt").unwrap();
    target_vfs.bind_mount_from(&source_vfs, "/a", "/mnt").unwrap();

    // Recursively access file
    let entry = target_vfs.resolve_path("/mnt/b/file.txt").expect("recursive bind mount failed");
    assert_eq!(entry.name(), "file.txt");
}

#[test_case]
fn test_cross_vfs_bind_mount_multiple() {
    use crate::fs::FileType;
    use crate::fs::manager::VfsManager;
    use alloc::sync::Arc;

    let source1 = Arc::new(VfsManager::new());
    let source2 = Arc::new(VfsManager::new());
    source1.create_dir("/d1").unwrap();
    source2.create_dir("/d2").unwrap();
    source1.create_file("/d1/f1", FileType::RegularFile).unwrap();
    source2.create_file("/d2/f2", FileType::RegularFile).unwrap();
    let target = Arc::new(VfsManager::new());
    target.create_dir("/mnt1").unwrap();
    target.create_dir("/mnt2").unwrap();
    target.bind_mount_from(&source1, "/d1", "/mnt1").unwrap();
    target.bind_mount_from(&source2, "/d2", "/mnt2").unwrap();
    let e1 = target.resolve_path("/mnt1/f1").expect("mnt1 failed");
    let e2 = target.resolve_path("/mnt2/f2").expect("mnt2 failed");
    assert_eq!(e1.name(), "f1");
    assert_eq!(e2.name(), "f2");
}

#[test_case]
fn test_cross_vfs_bind_mount_parent_traversal() {
    use crate::fs::FileType;
    use crate::fs::manager::VfsManager;
    use alloc::sync::Arc;

    let source = Arc::new(VfsManager::new());
    source.create_dir("/d").unwrap();
    source.create_file("/d/f", FileType::RegularFile).unwrap();
    let target = Arc::new(VfsManager::new());
    target.create_dir("/mnt").unwrap();
    target.create_file("/outside", FileType::RegularFile).unwrap();
    target.bind_mount_from(&source, "/d", "/mnt").unwrap();
    // .. from inside bind mount should not escape to source VFS
    let e = target.resolve_path("/mnt/../outside").expect("parent traversal failed");
    assert_eq!(e.name(), "outside");
    // .. from inside bind mount root should not escape to source VFS
    assert!(target.resolve_path("/mnt/../../d/f").is_err());
}