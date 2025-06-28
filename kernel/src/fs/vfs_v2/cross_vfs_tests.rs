//! Cross-VFS Bind Mount Tests


#[test_case]
fn test_cross_vfs_bind_mount_basic() {
    use crate::fs::FileType;
    use super::manager::VfsManager;
    use alloc::sync::Arc;

    // source側VFSを作成し、ディレクトリとファイルを用意
    let source_vfs = Arc::new(VfsManager::new());
    source_vfs.create_dir("/srcdir").unwrap();
    source_vfs.create_file("/srcdir/file.txt", FileType::RegularFile).unwrap();

    // target側VFSを作成
    let target_vfs = Arc::new(VfsManager::new());
    target_vfs.create_dir("/mnt").unwrap();
    target_vfs.create_file("/root_file.txt", FileType::RegularFile).unwrap();

    // cross-vfs bind mountを実行
    target_vfs.bind_mount_from(
        source_vfs.clone(),
        "/srcdir",
        "/mnt",
    ).expect("cross-vfs bind mount failed");

    // target側からbind mount配下のファイルにアクセスできるか
    let entry  = target_vfs.resolve_path("/mnt/file.txt").expect("resolve_path failed");
    assert_eq!(entry.name(), "file.txt");

    // ..でbind mountのルートから脱獄できないか（/mnt/..はtarget側の親に戻るはず）
    let entry = target_vfs.resolve_path("/mnt/../root_file.txt").expect("resolve_path failed");
    assert_eq!(entry.name(), "root_file.txt");
}