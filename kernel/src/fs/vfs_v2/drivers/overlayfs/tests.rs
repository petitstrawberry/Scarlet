//! OverlayFS v2 tests (same VFS only)

use super::OverlayFS;
use super::super::tmpfs::TmpFS;
use crate::fs::mount_tree::MountPoint;
use crate::fs::FileSystemOperations;
use crate::fs::FileType;
use crate::fs::SeekFrom;
use crate::fs::VfsEntry;
use alloc::string::ToString;
use alloc::vec::Vec;
use alloc::vec;
use alloc::sync::Arc;

// Helper to create a MountPoint
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
    /*
    Directory structure:

    lower:/
    ├── foo (file)
    ├── bar (file)
    upper:/
    ├── foo (file, overrides lower)
    ├── baz (file)

    OverlayFS root:
    ├── foo  (from upper)
    ├── bar  (from lower)
    ├── baz  (from upper)
    */
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
    /*
    Directory structure:

    lower:/
    ├── a (file)
    ├── b (file)
    upper:/
    ├── b (file, overrides lower)
    ├── c (file)

    OverlayFS root:
    ├── a
    ├── b (from upper)
    ├── c
    */
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
    /*
    Directory structure:

    lower:/
    ├── testfile (file, "lower content")
    upper:/
    (empty)

    After copy-up:
    upper:/
    ├── testfile (file, "upper content")

    OverlayFS root:
    ├── testfile (from upper after write)
    */
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
    /*
    Directory structure:

    lower:/
    ├── hideme (file)
    upper:/
    (empty)

    After remove:
    upper:/
    ├── .wh.hideme (whiteout)

    OverlayFS root:
    (no hideme)
    */
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
    /*
    Directory structure:

    lower:/
    ├── readonly (file)
    upper:/
    (empty)

    OverlayFS is read-only (no upper layer)
    OverlayFS root:
    ├── readonly (readable, not writable)
    */
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
    /*
    Directory structure:

    lower:/
    ├── dir1/
    upper:/
    ├── dir1/

    After remove:
    upper:/
    ├── .wh.dir1 (whiteout)

    OverlayFS root:
    (no dir1)
    */
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
    /*
    Directory/mount structure:

    lower:/
    └── dir1/
        └── mnt/ (mount point)
    mount_fs:/
    └── file_in_mount (file)

    bind mount:
    mount_fs:/  →  lower:/dir1/mnt

    OverlayFS lower: /dir1/mnt
    OverlayFS root:
    └── file_in_mount
    */
    use crate::fs::vfs_v2::manager::VfsManager;
    use alloc::sync::Arc;

    let lower = TmpFS::new(0);
    let upper = TmpFS::new(0);
    let mount_fs = TmpFS::new(0);

    // Initialize lower_mgr and mount_mgr with new_with_root
    let lower_mgr = VfsManager::new_with_root(lower.clone());
    let mount_mgr = VfsManager::new_with_root(mount_fs.clone());

    // Create /dir1/mnt in lower_mgr
    let lower_root = lower.root_node();
    let dir1 = lower.create(&lower_root, &"dir1".to_string(), FileType::Directory, 0o755).unwrap();
    let mnt = lower.create(&dir1, &"mnt".to_string(), FileType::Directory, 0o755).unwrap();
    // Create a file in mount_fs
    let mount_root = mount_fs.root_node();
    mount_fs.create(&mount_root, &"file_in_mount".to_string(), FileType::RegularFile, 0o644).unwrap();

    // bind mount: mount_mgr:/ → lower_mgr:/dir1/mnt
    lower_mgr.bind_mount_from(Arc::new(mount_mgr), "/", "/dir1/mnt").unwrap();

    // Use /dir1/mnt in lower_mgr as the lower layer for OverlayFS
    let mnt_entry = lower_mgr.resolve_path("/dir1/mnt").unwrap();
    let mnt_mp = make_mount(lower.clone() as Arc<dyn FileSystemOperations>);
    let (upper_mp, upper_entry) = make_mount_and_entry(upper.clone() as Arc<dyn FileSystemOperations>);
    let overlay = OverlayFS::new_with_dirs(
        Some((upper_mp.clone(), upper_entry.clone())),
        vec![(mnt_mp.clone(), mnt_entry.clone())],
        "overlayfs".to_string()
    ).unwrap();
    let root = overlay.root_node();

    // Confirm that file_in_mount is visible via OverlayFS
    let file_node = overlay.lookup(&root, &"file_in_mount".to_string()).unwrap();
    assert_eq!(file_node.metadata().unwrap().file_type, FileType::RegularFile);

    // Remove file_in_mount via OverlayFS (whiteout)
    overlay.remove(&root, &"file_in_mount".to_string()).unwrap();
    // Confirm that file_in_mount is no longer visible
    assert!(overlay.lookup(&root, &"file_in_mount".to_string()).is_err());
    // Confirm that a whiteout file was created in the upper layer
    let upper_root = upper.root_node();
    assert!(upper.lookup(&upper_root, &".wh.file_in_mount".to_string()).is_ok());
}

#[test_case]
fn test_overlayfs_nested_mnt_bind_mounts() {
    /*
    Directory/mount structure:

    lower:/
    └── mnt/ (mount point)
    mount1:/
    └── file1 (file)
    mount2:/
    └── file2 (file)

    bind mount:
    mount1:/      → lower:/mnt
    mount2:/      → lower:/mnt/child

    Resulting structure:
    /mnt
    ├── file1      (from mount1)
    └── child/     (mount point)
        └── file2  (from mount2)

    OverlayFS lower: /mnt
    OverlayFS root:
    ├── file1
    └── child/
    OverlayFS /mnt/child:
    └── file2
    */
    use crate::fs::vfs_v2::manager::VfsManager;
    use alloc::sync::Arc;

    // Prepare each FS
    let lower = TmpFS::new(0);
    let mount1 = TmpFS::new(0);
    let mount2 = TmpFS::new(0);

    // Create /mnt/child in lower
    let lower_root = lower.root_node();
    let mnt = lower.create(&lower_root, &"mnt".to_string(), FileType::Directory, 0o755).unwrap();

    // Create mount1:/file1, mount2:/file2
    let mount1_root = mount1.root_node();
    mount1.create(&mount1_root, &"file1".to_string(), FileType::RegularFile, 0o644).unwrap();
    let child = lower.create(&mount1_root, &"child".to_string(), FileType::Directory, 0o755).unwrap();

    let mount2_root = mount2.root_node();
    mount2.create(&mount2_root, &"file2".to_string(), FileType::RegularFile, 0o644).unwrap();

    // Bind mount with VfsManager
    let lower_mgr = VfsManager::new_with_root(lower.clone());
    let mount1_mgr = VfsManager::new_with_root(mount1.clone());
    let mount2_mgr = VfsManager::new_with_root(mount2.clone());
    lower_mgr.bind_mount_from(Arc::new(mount1_mgr), "/", "/mnt").unwrap();
    lower_mgr.bind_mount_from(Arc::new(mount2_mgr), "/", "/mnt/child").unwrap();

    // Check the lower_mgr's readdir for /mnt and /mnt/child
    // Expected structure:
    // /mnt
    // ├── file1      ← from mount1
    // └── child
    //     └── file2  ← from mount2
    let entries = lower_mgr.readdir("/mnt").unwrap();
    assert!(entries.iter().any(|e| e.name == "file1"));
    assert!(entries.iter().any(|e| e.name == "child"));
    let entries = lower_mgr.readdir("/mnt/child").unwrap();
    assert!(entries.iter().any(|e| e.name == "file2"));


    // Get VfsEntry and MountPoint for lower:/mnt
    let (mnt_entry, mnt_mp) = lower_mgr.mount_tree.resolve_path("/mnt").unwrap();
    // Check mnt_mp has child mount
    let children = &mnt_mp.children;
    assert!(children.read().values().any(|c| c.path == "child"));

    // Upper layer is an empty TmpFS
    let upper = TmpFS::new(0);
    let (upper_mp, upper_entry) = make_mount_and_entry(upper.clone() as Arc<dyn FileSystemOperations>);

    // Create OverlayFS
    let overlay = OverlayFS::new_with_dirs(
        Some((upper_mp, upper_entry)),
        vec![(mnt_mp, mnt_entry.clone())],
        "overlayfs".to_string()
    ).unwrap();
    let root = overlay.root_node();

    // file1 and child should be visible at OverlayFS root
    let entries = overlay.readdir(&root).unwrap();
    let mut names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
    names.sort();
    assert!(names.contains(&"file1"));
    assert!(names.contains(&"child"));

    // file2 should be visible in child directory
    let child_node = overlay.lookup(&root, &"child".to_string()).unwrap();
    let child_entries = overlay.readdir(&child_node).unwrap();
    let child_names: Vec<_> = child_entries.iter().map(|e| e.name.as_str()).collect();
    assert!(child_names.contains(&"file2"));
}
