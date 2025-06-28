//! Cross-VFS Bind Mount Tests


#[test_case]
fn test_cross_vfs_bind_mount_basic() {
    use crate::fs::FileType;
    use super::manager::VfsManager;
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
        source_vfs.clone(),
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