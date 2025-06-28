//! Cross-VFS Bind Mount Tests


use alloc::{sync::Arc, format};
use crate::{println};

use super::{
    manager::VfsManager,
    mount_tree::{BindType, MountType},
    tmpfs::TmpFS,
    core::{FileSystemOperations},
};
use crate::fs::{FileType};

#[test_case]
fn test_vfs_manager_id_unique() {
    let vfs1 = VfsManager::new();
    let vfs2 = VfsManager::new();
    let vfs3 = VfsManager::new();
    
    assert_ne!(vfs1.id(), vfs2.id(), "VFS Manager IDs should be unique");
    assert_ne!(vfs2.id(), vfs3.id(), "VFS Manager IDs should be unique");
    assert_ne!(vfs1.id(), vfs3.id(), "VFS Manager IDs should be unique");
    
    println!("VFS IDs: {:?}, {:?}, {:?}", vfs1.id(), vfs2.id(), vfs3.id());
}

#[test_case]
fn test_basic_cross_vfs_bind_mount() {
    let source_vfs = Arc::new(VfsManager::new());
    source_vfs.create_dir("/source_dir1").expect("Failed to create source directory");
    source_vfs.create_file("/source_dir1/file1.txt", FileType::RegularFile)
        .expect("Failed to create file1.txt in source VFS");

    let target_vfs = Arc::new(VfsManager::new());

    // Create mount point in target VFS
    target_vfs.create_dir("/mount_point").expect("Failed to create mount point");
    
    // Register cross-VFS reference
    target_vfs.register_cross_vfs_ref(source_vfs.clone())
        .expect("Failed to register cross-VFS ref");
    
    // Create cross-VFS bind mount
    let result = target_vfs.cross_vfs_bind_mount(
        source_vfs.id(),
        "/source_dir1",
        "/mount_point",
        false
    );
    
    assert!(result.is_ok(), "Cross-VFS bind mount should succeed");
    
    // Verify mount was created
    let mounts = target_vfs.list_mounts();
    println!("Mounts after bind: {:?}", mounts);
    let mount_found = mounts.iter().any(|(path, mount_type)| {
            path == "/mount_point" && matches!(mount_type, MountType::Bind { bind_type: BindType::CrossVfs { .. } })
        });
    
    assert!(mount_found, "Cross-VFS bind mount should be in mount list");
}

#[test_case]
fn test_cross_vfs_path_resolution() {
    let source_vfs = Arc::new(VfsManager::new());
    source_vfs.create_dir("/source_dir1").expect("Failed to create source directory");
    source_vfs.create_file("/source_dir1/file1.txt", FileType::RegularFile)
        .expect("Failed to create file1.txt in source VFS");
    source_vfs.create_dir("/source_dir1/subdir").expect("Failed to create subdirectory");
    source_vfs.create_file("/source_dir1/subdir/file2.txt", FileType::RegularFile)
        .expect("Failed to create file2.txt in subdirectory");

    let target_vfs = Arc::new(VfsManager::new());
    
    // Create bind target
    target_vfs.create_dir("/bind_target").expect("Failed to create bind target");
    
    // Register cross-VFS reference
    target_vfs.register_cross_vfs_ref(source_vfs.clone())
        .expect("Failed to register cross-VFS ref");
    
    // Create cross-VFS bind mount
    target_vfs.cross_vfs_bind_mount(
        source_vfs.id(),
        "/source_dir1",
        "/bind_target",
        false
    ).expect("Failed to create cross-VFS bind mount");
    
    // Test path resolution through the bind mount
    let result = target_vfs.resolve_path("/bind_target/file1.txt");
    assert!(result.is_ok(), "Path resolution through cross-VFS bind should work");
    
    // Test subdirectory resolution
    let result = target_vfs.resolve_path("/bind_target/subdir/file2.txt");
    assert!(result.is_ok(), "Subdirectory resolution through cross-VFS bind should work");

        // Test subdirectory resolution
    let result = target_vfs.resolve_path("/bind_target/subdir/../subdir/file2.txt");
    assert!(result.is_ok(), "Subdirectory resolution through cross-VFS bind should work");
    
    // Test directory resolution
    let result = target_vfs.resolve_path("/bind_target/subdir");
    assert!(result.is_ok(), "Failed to resolve subdirectory through cross-VFS bind");
}

#[test_case]
fn test_cross_vfs_error_cases() {
    let source_vfs = Arc::new(VfsManager::new());
    let target_vfs = Arc::new(VfsManager::new());

    // Register cross-VFS reference
    target_vfs.register_cross_vfs_ref(source_vfs.clone())
        .expect("Failed to register cross-VFS ref");
    
    // Test binding to non-existent source path
    let result = target_vfs.cross_vfs_bind_mount(
        source_vfs.id(),
        "/nonexistent",
        "/target_path",
        false
    );
    assert!(result.is_err(), "Binding to non-existent source should fail");
    
    // Test binding to non-existent target path
    let result = target_vfs.cross_vfs_bind_mount(
        source_vfs.id(),
        "/source_dir1",
        "/nonexistent_target",
        false
    );
    assert!(result.is_err(), "Binding to non-existent target should fail");
    
    // Test recursive bind (binding VFS to itself)
    target_vfs.create_dir("/recursive_test").expect("Failed to create recursive test dir");
    let result = target_vfs.cross_vfs_bind_mount(
        target_vfs.id(),
        "/target_dir1",
        "/recursive_test",
        false
    );
    assert!(result.is_err(), "Recursive bind mount should fail");
}

#[test_case]
fn test_cross_vfs_cleanup() {
    let source_vfs = Arc::new(VfsManager::new());
    let target_vfs = Arc::new(VfsManager::new());

    // Register cross-VFS reference
    target_vfs.register_cross_vfs_ref(source_vfs.clone())
        .expect("Failed to register cross-VFS ref");
    
    let refs_before = target_vfs.get_cross_vfs_ref_count();
    assert_eq!(refs_before, 1, "Should have one cross-VFS reference");
    
    // Drop the source VFS
    drop(source_vfs);
    
    // Clean up weak references
    target_vfs.cleanup_cross_vfs_refs();
    
    let refs_after = target_vfs.get_cross_vfs_ref_count();
    assert_eq!(refs_after, 0, "Stale references should be cleaned up");
}

#[test_case]
fn test_multiple_cross_vfs_binds() {
    let source1_vfs = Arc::new(VfsManager::new());
    source1_vfs.create_dir("/source1_dir1").expect("Failed to create source1 directory");
    source1_vfs.create_file("/source1_dir1/file_src1.txt", FileType::RegularFile)
        .expect("Failed to create file_src1.txt in source1 VFS");

    let source2_vfs = Arc::new(VfsManager::new());
    source2_vfs.create_dir("/source2_dir1").expect("Failed to create source2 directory");
    source2_vfs.create_file("/source2_dir1/file_src2.txt", FileType::RegularFile)
        .expect("Failed to create file_src2.txt in source2 VFS");

    let target_vfs = Arc::new(VfsManager::new());

    // Create bind targets
    target_vfs.create_dir("/bind1").expect("Failed to create bind1");
    target_vfs.create_dir("/bind2").expect("Failed to create bind2");
    
    // Register cross-VFS references
    target_vfs.register_cross_vfs_ref(source1_vfs.clone())
        .expect("Failed to register cross-VFS ref 1");
    target_vfs.register_cross_vfs_ref(source2_vfs.clone())
        .expect("Failed to register cross-VFS ref 2");
    
    // Create multiple cross-VFS bind mounts
    target_vfs.cross_vfs_bind_mount(source1_vfs.id(), "/source1_dir1", "/bind1", false)
        .expect("Failed to create first cross-VFS bind");
    target_vfs.cross_vfs_bind_mount(source2_vfs.id(), "/source2_dir1", "/bind2", false)
        .expect("Failed to create second cross-VFS bind");
    
    // Verify both mounts exist
    let mounts = target_vfs.list_mounts();
    let cross_vfs_mounts = mounts.iter().filter(|(_, mount_type)| {
            matches!(mount_type, MountType::Bind { bind_type: BindType::CrossVfs { .. } })
        }).count();
    assert_eq!(cross_vfs_mounts, 2, "Should have two cross-VFS bind mounts");

    // Verify paths in both mounts
    let result1 = target_vfs.resolve_path("/bind1/file_src1.txt");
    assert!(result1.is_ok(), "Should resolve path in first cross-VFS bind");
    let result2 = target_vfs.resolve_path("/bind2/file_src2.txt");
    assert!(result2.is_ok(), "Should resolve path in second cross-VFS bind");
}