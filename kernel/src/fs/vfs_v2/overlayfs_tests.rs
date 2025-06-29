//! OverlayFS v2 tests (same VFS only)

use super::OverlayFS;
use super::tmpfs::TmpFS;
use super::mount_tree::MountPoint;
use crate::fs::vfs_v2::FileSystemOperations;
use crate::fs::FileType;
use crate::fs::SeekFrom;
use alloc::string::ToString;
use alloc::vec::Vec;
use alloc::vec;
use super::core::VfsEntry;
use alloc::sync::Arc;

// MountPoint生成ヘルパ
fn make_mount(fs: Arc<dyn FileSystemOperations>) -> Arc<MountPoint> {
    let root_node = fs.root_node();
    let root_entry = VfsEntry::new(None, "/".to_string(), root_node);
    MountPoint::new_regular("/".to_string(), root_entry)
}

fn make_mount_and_entry(fs: Arc<dyn FileSystemOperations>) -> (Arc<MountPoint>, Arc<VfsEntry>) {
    let root_node = fs.root_node();
    let root_entry = VfsEntry::new(None, "/".to_string(), root_node);
    let mp = MountPoint::new_regular("/".to_string(), root_entry.clone());
    (mp, root_entry)
}

#[test_case]
fn test_overlayfs_basic() {
    let lower = TmpFS::new(0);
    let upper = TmpFS::new(0);
    
    // Create files in lower layer
    let lower_root = lower.root_node();
    lower.create(&lower_root.clone(), &"foo".to_string(), FileType::RegularFile, 0o644).unwrap();
    lower.create(&lower_root.clone(), &"bar".to_string(), FileType::RegularFile, 0o644).unwrap();
    
    // Create files in upper layer
    let upper_root = upper.root_node();
    upper.create(&upper_root.clone(), &"foo".to_string(), FileType::RegularFile, 0o644).unwrap(); // Override lower
    upper.create(&upper_root.clone(), &"baz".to_string(), FileType::RegularFile, 0o644).unwrap();
    
    let (lower_mp, lower_entry) = make_mount_and_entry(lower.clone() as Arc<dyn FileSystemOperations>);
    let (upper_mp, upper_entry) = make_mount_and_entry(upper.clone() as Arc<dyn FileSystemOperations>);
    let overlay = OverlayFS::new_with_dirs(
        Some((upper_mp.clone(), upper_entry.clone())),
        vec![(lower_mp.clone(), lower_entry.clone())],
        "overlayfs".to_string()
    ).unwrap();
    let root = overlay.root_node();
    // Test lookups
    let foo = overlay.lookup(&root, &"foo".to_string()).unwrap(); // Should be from upper
    assert_eq!(foo.metadata().unwrap().file_type, FileType::RegularFile);
    let bar = overlay.lookup(&root, &"bar".to_string()).unwrap(); // Should be from lower
    assert_eq!(bar.metadata().unwrap().file_type, FileType::RegularFile);
    let baz = overlay.lookup(&root, &"baz".to_string()).unwrap(); // Should be from upper
    assert_eq!(baz.metadata().unwrap().file_type, FileType::RegularFile);
}

#[test_case]
fn test_overlayfs_readdir() {
    let lower = TmpFS::new(0);
    let upper = TmpFS::new(0);
    
    let lower_root = lower.root_node();
    lower.create(&lower_root.clone(), &"a".to_string(), FileType::RegularFile, 0o644).unwrap();
    lower.create(&lower_root.clone(), &"b".to_string(), FileType::RegularFile, 0o644).unwrap();
    
    let upper_root = upper.root_node();
    upper.create(&upper_root.clone(), &"b".to_string(), FileType::RegularFile, 0o644).unwrap(); // Override
    upper.create(&upper_root.clone(), &"c".to_string(), FileType::RegularFile, 0o644).unwrap();
    
    let (lower_mp, lower_entry) = make_mount_and_entry(lower.clone() as Arc<dyn FileSystemOperations>);
    let (upper_mp, upper_entry) = make_mount_and_entry(upper.clone() as Arc<dyn FileSystemOperations>);
    let overlay = OverlayFS::new_with_dirs(
        Some((upper_mp.clone(), upper_entry.clone())),
        vec![(lower_mp.clone(), lower_entry.clone())],
        "overlayfs".to_string()
    ).unwrap();
    
    let root = overlay.root_node();
    let entries = overlay.readdir(&root).unwrap();
    let mut names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
    names.sort();
    
    // Should have . .. a b c (b from upper, not duplicated)
    assert_eq!(names, vec![".", "..", "a", "b", "c"]);
}

#[test_case]
fn test_overlayfs_copy_up() {
    let lower = TmpFS::new(0);
    let upper = TmpFS::new(0);
    
    // Create file in lower only
    let lower_root = lower.root_node();
    let lower_file = lower.create(&lower_root.clone(), &"testfile".to_string(), FileType::RegularFile, 0o644).unwrap();
    
    // Write content to lower file
    let lower_file_obj = lower.open(&lower_file, 1).unwrap(); // Write mode
    lower_file_obj.write(b"lower content").unwrap();
    
    let (lower_mp, lower_entry) = make_mount_and_entry(lower.clone() as Arc<dyn FileSystemOperations>);
    let (upper_mp, upper_entry) = make_mount_and_entry(upper.clone() as Arc<dyn FileSystemOperations>);
    let overlay = OverlayFS::new_with_dirs(
        Some((upper_mp.clone(), upper_entry.clone())),
        vec![(lower_mp.clone(), lower_entry.clone())],
        "overlayfs".to_string()
    ).unwrap();
    
    // Open file for writing - should trigger copy-up
    let root = overlay.root_node();
    let overlay_file_node = overlay.lookup(&root, &"testfile".to_string()).unwrap();
    let overlay_file_obj = overlay.open(&overlay_file_node, 1).unwrap(); // Write mode
    
    // Write new content
    overlay_file_obj.write(b"upper content").unwrap();
    
    // Verify upper layer now has the file
    let upper_root = upper.root_node();
    let upper_file = upper.lookup(&upper_root, &"testfile".to_string()).unwrap();
    let upper_file_obj = upper.open(&upper_file, 0).unwrap(); // Read mode
    let mut buffer = [0u8; 32];
    let bytes_read = upper_file_obj.read(&mut buffer).unwrap();
    assert_eq!(&buffer[..bytes_read], b"upper content");

    // Verify lower layer still has original content
    let mut lower_buffer = [0u8; 32];
    let lower_file_obj = lower.open(&lower_file, 1).unwrap(); // Write mode
    lower_file_obj.write(b"lower content").unwrap();
    lower_file_obj.seek(SeekFrom::Start(0)).unwrap();
    let lower_bytes_read = lower_file_obj.read(&mut lower_buffer).unwrap();
    assert_eq!(&lower_buffer[..lower_bytes_read], b"lower content");    
}

#[test_case]
fn test_overlayfs_whiteout() {
    let lower = TmpFS::new(0);
    let upper = TmpFS::new(0);
    
    // Create file in lower
    let lower_root = lower.root_node();
    lower.create(&lower_root.clone(), &"hideme".to_string(), FileType::RegularFile, 0o644).unwrap();
    
    let (lower_mp, lower_entry) = make_mount_and_entry(lower.clone() as Arc<dyn FileSystemOperations>);
    let (upper_mp, upper_entry) = make_mount_and_entry(upper.clone() as Arc<dyn FileSystemOperations>);
    let overlay = OverlayFS::new_with_dirs(
        Some((upper_mp.clone(), upper_entry.clone())),
        vec![(lower_mp.clone(), lower_entry.clone())],
        "overlayfs".to_string()
    ).unwrap();
    
    // Remove file - should create whiteout
    let root = overlay.root_node();
    overlay.remove(&root, &"hideme".to_string()).unwrap();
    
    // File should no longer be visible
    assert!(overlay.lookup(&root, &"hideme".to_string()).is_err());
    
    // Verify whiteout file exists in upper
    let upper_root = upper.root_node();
    assert!(upper.lookup(&upper_root, &".wh.hideme".to_string()).is_ok());
}

#[test_case]
fn test_overlayfs_read_only() {
    let lower = TmpFS::new(0);
    
    // Create file in lower
    let lower_root = lower.root_node();
    lower.create(&lower_root.clone(), &"readonly".to_string(), FileType::RegularFile, 0o644).unwrap();
    
    let (lower_mp, lower_entry) = make_mount_and_entry(lower.clone() as Arc<dyn FileSystemOperations>);
    // Create read-only overlay (no upper layer)
    let overlay = OverlayFS::new_with_dirs(
        None,
        vec![(lower_mp, lower_entry)],
        "overlayfs".to_string()
    ).unwrap();
    
    assert!(overlay.is_read_only());
    
    // Should be able to read
    let root = overlay.root_node();
    let file_node = overlay.lookup(&root.clone(), &"readonly".to_string()).unwrap();
    let file_obj = overlay.open(&file_node, 0).unwrap(); // Read mode
    
    // Should not be able to write
    let root = overlay.root_node();
    let file_node = overlay.lookup(&root.clone(), &"readonly".to_string()).unwrap();
    assert!(overlay.open(&file_node, 1).is_err()); // Write mode
    
    // Should not be able to create
    let root = overlay.root_node();
    assert!(overlay.create(&root, &"newfile".to_string(), FileType::RegularFile, 0o644).is_err());
}

#[test_case]
fn test_overlayfs_upper_dir_remove_whiteout() {
    let lower = TmpFS::new(0);
    let upper = TmpFS::new(0);

    // Create a directory named "dir1" in both lower and upper layers
    let lower_root = lower.root_node();
    lower.create(&lower_root, &"dir1".to_string(), FileType::Directory, 0o755).unwrap();
    let upper_root = upper.root_node();
    upper.create(&upper_root, &"dir1".to_string(), FileType::Directory, 0o755).unwrap();

    let (lower_mp, lower_entry) = make_mount_and_entry(lower.clone() as Arc<dyn FileSystemOperations>);
    let (upper_mp, upper_entry) = make_mount_and_entry(upper.clone() as Arc<dyn FileSystemOperations>);
    let overlay = OverlayFS::new_with_dirs(
        Some((upper_mp.clone(), upper_entry.clone())),
        vec![(lower_mp.clone(), lower_entry.clone())],
        "overlayfs".to_string()
    ).unwrap();
    let root = overlay.root_node();

    // Remove dir1 via OverlayFS
    overlay.remove(&root, &"dir1".to_string()).unwrap();

    // Confirm that dir1 is no longer visible from OverlayFS
    assert!(overlay.lookup(&root, &"dir1".to_string()).is_err());

    // Confirm that a whiteout file was created in the upper layer
    let upper_dir1_whiteout = upper.lookup(&upper_root, &".wh.dir1".to_string());
    assert!(upper_dir1_whiteout.is_ok());
}

#[test_case]
fn test_overlayfs_lower_mount_visibility_and_whiteout() {
    use crate::fs::vfs_v2::manager::VfsManager;
    use alloc::sync::Arc;

    let lower = TmpFS::new(0);
    let upper = TmpFS::new(0);
    let mount_fs = TmpFS::new(0);

    // lower_mgrとmount_mgrをnew_with_rootで初期化
    let lower_mgr = VfsManager::new_with_root(lower.clone());
    let mount_mgr = VfsManager::new_with_root(mount_fs.clone());

    // lower_mgrの/dir1/mntを作成
    let lower_root = lower.root_node();
    let dir1 = lower.create(&lower_root, &"dir1".to_string(), FileType::Directory, 0o755).unwrap();
    let mnt = lower.create(&dir1, &"mnt".to_string(), FileType::Directory, 0o755).unwrap();
    // mount_fsにファイルを作成
    let mount_root = mount_fs.root_node();
    mount_fs.create(&mount_root, &"file_in_mount".to_string(), FileType::RegularFile, 0o644).unwrap();

    // bind mount: mount_mgr:/ → lower_mgr:/dir1/mnt
    lower_mgr.bind_mount_from(Arc::new(mount_mgr), "/", "/dir1/mnt").unwrap();

    // OverlayFSはlower_mgrの/dir1/mntを使う
    let mnt_entry = lower_mgr.resolve_path("/dir1/mnt").unwrap();
    let mnt_node = mnt_entry.node();

    let (lower_mp, lower_entry) = make_mount_and_entry(lower.clone() as Arc<dyn FileSystemOperations>);
    let (upper_mp, upper_entry) = make_mount_and_entry(upper.clone() as Arc<dyn FileSystemOperations>);
    let overlay = OverlayFS::new_with_dirs(
        Some((upper_mp.clone(), upper_entry.clone())),
        vec![(lower_mp.clone(), lower_entry.clone())],
        "overlayfs".to_string()
    ).unwrap();
    let root = overlay.root_node();

    // OverlayFS経由で/dir1/mnt/file_in_mountが見えることを確認
    let file_node = overlay.lookup(&root, &"file_in_mount".to_string()).unwrap();
    assert_eq!(file_node.metadata().unwrap().file_type, FileType::RegularFile);

    // OverlayFS経由でmntをwhiteout（remove）
    overlay.remove(&root, &"file_in_mount".to_string()).unwrap();
    // file_in_mountが見えなくなっていることを確認
    assert!(overlay.lookup(&root, &"file_in_mount".to_string()).is_err());
    // upper層にwhiteoutファイルができていることを確認
    let upper_root = upper.root_node();
    assert!(upper.lookup(&upper_root, &".wh.file_in_mount".to_string()).is_ok());
}

#[test_case]
fn test_overlayfs_nested_mnt_bind_mounts() {
    use crate::fs::vfs_v2::manager::VfsManager;
    use alloc::sync::Arc;

    let lower = TmpFS::new(0);
    let upper = TmpFS::new(0);
    let mount_fs1 = TmpFS::new(0);
    let mount_fs2 = TmpFS::new(0);

    // VFSマネージャを初期化
    let lower_mgr = VfsManager::new_with_root(lower.clone());
    let mount_mgr1 = VfsManager::new_with_root(mount_fs1.clone());
    let mount_mgr2 = VfsManager::new_with_root(mount_fs2.clone());

    // lower側に/dir1/mnt/mnt2を作成
    let lower_root = lower.root_node();
    let dir1 = lower.create(&lower_root, &"dir1".to_string(), FileType::Directory, 0o755).unwrap();
    let mnt = lower.create(&dir1, &"mnt".to_string(), FileType::Directory, 0o755).unwrap();
    let mnt2 = lower.create(&mnt, &"mnt2".to_string(), FileType::Directory, 0o755).unwrap();

    // mount_fs1, mount_fs2にファイル・ディレクトリを作成
    let mount_root1 = mount_fs1.root_node();
    mount_fs1.create(&mount_root1, &"file1".to_string(), FileType::RegularFile, 0o644).unwrap();
    // mount_fs1上にmnt2ディレクトリを作成（bind mountのため）
    let mount_mnt2 = mount_fs1.create(&mount_root1, &"mnt2".to_string(), FileType::Directory, 0o755).unwrap();
    let mount_root2 = mount_fs2.root_node();
    mount_fs2.create(&mount_root2, &"file2".to_string(), FileType::RegularFile, 0o644).unwrap();

    crate::println!("Before bind mounts:");
    // Check entries in /dir1/mnt
    let entries = lower_mgr.readdir("/dir1/mnt").unwrap();
    for entry in entries {
        crate::println!("Entry in /dir1/mnt: {}", entry.name);
    }

    // bind mount: mount_mgr1:/ → lower_mgr:/dir1/mnt
    lower_mgr.bind_mount_from(Arc::new(mount_mgr1), "/", "/dir1/mnt").unwrap();

    // bind mount: mount_mgr2:/ → lower_mgr:/dir1/mnt/mnt2
    lower_mgr.bind_mount_from(Arc::new(mount_mgr2), "/", "/dir1/mnt/mnt2").unwrap();

    // OverlayFSはlower_mgrの/dir1/mntを使う
    let mnt_entry = lower_mgr.resolve_path("/dir1/mnt").unwrap();
    let mnt_node = mnt_entry.node();

    // bind mount後のmnt_nodeでfile1だけが見えることを確認
    let mnt_dirents = lower.readdir(&mnt_node).unwrap();
    let mnt_names: Vec<_> = mnt_dirents.iter().map(|e| &e.name).collect();
    assert_eq!(mnt_names, vec![&"file1".to_string(), &"mnt2".to_string()], "mnt_nodeに余計なエントリが見えている: {:?}", mnt_names);

    // mnt2ディレクトリの中身はfile2のみ
    let mnt2_entry = lower_mgr.resolve_path("/dir1/mnt/mnt2").unwrap();
    let mnt2_node = mnt2_entry.node();
    let mnt2_dirents = lower.readdir(&mnt2_node).unwrap();
    crate::println!("Entries in /dir1/mnt/mnt2:");
    for entry in &mnt2_dirents {
        crate::println!(" - {}", entry.name);
    }
    let mnt2_names: Vec<_> = mnt2_dirents.iter().map(|e| &e.name).collect();
    assert_eq!(mnt2_names, vec![&"file2".to_string()], "mnt2_nodeに余計なエントリが見えている: {:?}", mnt2_names);

    // --- デバッグ: OverlayFS lowerに渡しているmnt2ノードでfile2が見えるか確認 ---
    let mnt2_entry = lower_mgr.resolve_path("/dir1/mnt/mnt2").unwrap();
    let mnt2_node = mnt2_entry.node();
    let mnt2_dirents = lower.readdir(&mnt2_node).unwrap();
    crate::println!("Direct readdir on lower mnt2 node:");
    for entry in &mnt2_dirents {
        crate::println!(" - {}", entry.name);
    }
    assert!(mnt2_dirents.iter().any(|e| e.name == "file2"), "Not found: {:?}", mnt2_dirents);
    // --- ここまでデバッグ ---

    let (lower_mp, lower_entry) = make_mount_and_entry(lower.clone() as Arc<dyn FileSystemOperations>);
    let (upper_mp, upper_entry) = make_mount_and_entry(upper.clone() as Arc<dyn FileSystemOperations>);
    // OverlayFSはlower_mgrの/dir1/mntを使う
    let overlay = OverlayFS::new_with_dirs(
        Some((upper_mp.clone(), upper_entry.clone())),
        vec![(lower_mp.clone(), lower_entry.clone())],
        "overlayfs".to_string()
    ).unwrap();
    let root = overlay.root_node();

    // /file1が見える（mount_fs1由来）
    let file1_node = overlay.lookup(&root, &"file1".to_string()).unwrap();
    assert_eq!(file1_node.metadata().unwrap().file_type, FileType::RegularFile);

    let dirents = overlay.readdir(&root).unwrap();
    for entry in &dirents {
        crate::println!("OverlayFS root entry: {}", entry.name);
    }

    // /mnt2/file2が見える（mount_fs2由来）
    let mnt2_node = overlay.lookup(&root, &"mnt2".to_string()).unwrap();
    let dirents = overlay.readdir(&mnt2_node).unwrap();
    for entry in &dirents {
        crate::println!("OverlayFS /mnt2 entry: {}", entry.name);
    }
    let mnt2_node = overlay.lookup(&root, &"mnt2".to_string()).unwrap();
    crate::println!("Looking up /mnt2/file2 in OverlayFS");
    let file2_node = overlay.lookup(&mnt2_node, &"file2".to_string()).unwrap();
    assert_eq!(file2_node.metadata().unwrap().file_type, FileType::RegularFile);

    // /mnt2/file2をwhiteout（remove）
    overlay.remove(&mnt2_node, &"file2".to_string()).unwrap();
    assert!(overlay.lookup(&mnt2_node, &"file2".to_string()).is_err());
    // upper層にwhiteoutファイルができていることを確認
    let upper_root = upper.root_node();
    let upper_mnt2 = upper.lookup(&upper_root, &"mnt2".to_string()).unwrap();
    assert!(upper.lookup(&upper_mnt2, &".wh.file2".to_string()).is_ok());
}
