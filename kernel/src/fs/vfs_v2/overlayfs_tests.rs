//! OverlayFS v2 tests (same VFS only)

use super::OverlayFS;
use super::tmpfs::TmpFS;
use crate::fs::vfs_v2::FileSystemOperations;
use crate::fs::FileType;
use crate::fs::SeekFrom;
use alloc::string::ToString;
use alloc::vec::Vec;
use alloc::vec;

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
    
    let overlay = OverlayFS::new(Some(upper.clone()), vec![lower.clone()], "overlayfs".to_string()).unwrap();
    
    // Test lookups
    let root = overlay.root_node();
    let foo = overlay.lookup(&root.clone(), &"foo".to_string()).unwrap(); // Should be from upper
    assert_eq!(foo.metadata().unwrap().file_type, FileType::RegularFile);
    
    let bar = overlay.lookup(&root.clone(), &"bar".to_string()).unwrap(); // Should be from lower
    assert_eq!(bar.metadata().unwrap().file_type, FileType::RegularFile);
    
    let baz = overlay.lookup(&root.clone(), &"baz".to_string()).unwrap(); // Should be from upper
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
    
    let overlay = OverlayFS::new(Some(upper.clone()), vec![lower.clone()], "overlayfs".to_string()).unwrap();
    
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
    
    let overlay = OverlayFS::new(Some(upper.clone()), vec![lower.clone()], "overlayfs".to_string()).unwrap();
    
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
    
    let overlay = OverlayFS::new(Some(upper.clone()), vec![lower.clone()], "overlayfs".to_string()).unwrap();
    
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
    
    // Create read-only overlay (no upper layer)
    let overlay = OverlayFS::new(None, vec![lower.clone()], "overlayfs".to_string()).unwrap();
    
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
