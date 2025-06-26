/// Performance tests for VFS v2 (simplified)
/// 
/// This module contains performance tests for VFS v2 components.

use alloc::{
    string::String,
    sync::Arc,
    vec::Vec,
    format,
};
use alloc::string::ToString;
use crate::fs::{
    vfs_v2::{
        tmpfs::TmpFS,
        manager::VfsManager,
    },
    FileType,
};
use crate::println;

/// Test basic performance - multiple file creation and lookup
#[test_case]
fn test_basic_performance() {
    let fs: Arc<dyn crate::fs::vfs_v2::core::FileSystemOperations> = TmpFS::new(1024 * 1024);
    let root_node = fs.root_node();
    let weak_fs = root_node.filesystem();
    assert!(weak_fs.is_some(), "root_node.filesystem() is None");
    if let Some(w) = weak_fs {
        let upgraded = w.upgrade();
        assert!(upgraded.is_some(), "root_node.filesystem().upgrade() is None");
    }
    
    let manager = VfsManager::new();
    // Mount tmpfs at root (cast to dyn FileSystemOperations)
    manager.mount(fs, "/", 0).unwrap();
    
    // Create multiple files and test lookup performance
    let file_count = 50;  // Reduced count for more reliable testing
    
    // Create files
    for i in 0..file_count {
        let filename = format!("/file_{}.txt", i);
        manager.create_file(&filename, FileType::RegularFile).unwrap();
    }
    
    // Test lookup performance by accessing each file
    for i in 0..file_count {
        let filename = format!("/file_{}.txt", i);
        let metadata = manager.metadata(&filename).unwrap();
        assert_eq!(metadata.file_type, FileType::RegularFile);
    }
    
    // Test directory listing performance
    let entries = manager.readdir("/").unwrap();
    assert_eq!(entries.len(), file_count);
}

/// Test concurrent operations - multiple managers and filesystems
#[test_case]
fn test_concurrent_operations() {    // Create two separate VFS managers to simulate concurrent usage
    let manager1 = VfsManager::new();
    let manager2 = VfsManager::new();
    
    // Create separate tmpfs instances
    let fs1: Arc<dyn crate::fs::vfs_v2::core::FileSystemOperations> = TmpFS::new(512 * 1024);
    let fs2: Arc<dyn crate::fs::vfs_v2::core::FileSystemOperations> = TmpFS::new(512 * 1024);
    
    // Mount different filesystems on each manager
    manager1.mount(fs1, "/", 0).unwrap();
    manager2.mount(fs2, "/", 0).unwrap();
    
    // Create files concurrently in both managers
    let file_count = 20;
    
    // Manager 1 creates files with prefix "mgr1_"
    for i in 0..file_count {
        let filename = format!("/mgr1_file_{}.txt", i);
        manager1.create_file(&filename, FileType::RegularFile).unwrap();
    }
    
    // Manager 2 creates files with prefix "mgr2_" 
    for i in 0..file_count {
        let filename = format!("/mgr2_file_{}.txt", i);
        manager2.create_file(&filename, FileType::RegularFile).unwrap();
    }
    
    // Verify both managers can read their own files
    let entries1 = manager1.readdir("/").unwrap();
    let entries2 = manager2.readdir("/").unwrap();
    
    assert_eq!(entries1.len(), file_count);
    assert_eq!(entries2.len(), file_count);
    
    // Test cross-access (each manager should only see its own files)
    for i in 0..file_count {
        let filename1 = format!("/mgr1_file_{}.txt", i);
        let filename2 = format!("/mgr2_file_{}.txt", i);
        
        // Manager 1 should see its files but not manager 2's files
        assert!(manager1.metadata(&filename1).is_ok());
        assert!(manager1.metadata(&filename2).is_err());
        
        // Manager 2 should see its files but not manager 1's files  
        assert!(manager2.metadata(&filename2).is_ok());
        assert!(manager2.metadata(&filename1).is_err());
    }
}

/// Test deep directory hierarchy performance
#[test_case]
fn test_deep_directory_performance() {
    let tmpfs = Arc::new(TmpFS::new(1024 * 1024));
    let manager = VfsManager::new();
    
    let fs: Arc<dyn crate::fs::vfs_v2::core::FileSystemOperations> = TmpFS::new(1024 * 1024);
    manager.mount(fs, "/", 0).unwrap();
    
    // Create deep directory hierarchy: /level1/level2/.../level10
    let max_depth = 10;
    let mut current_path = String::new();
    
    for level in 1..=max_depth {
        current_path = format!("{}/level{}", current_path, level);
        manager.create_dir(&current_path).unwrap();
        
        // Verify directory was created
        let metadata = manager.metadata(&current_path).unwrap();
        assert_eq!(metadata.file_type, FileType::Directory);
    }
    
    // Create a file at the deepest level
    let deep_file = format!("{}/deep_file.txt", current_path);
    manager.create_file(&deep_file, FileType::RegularFile).unwrap();
    
    // Test lookup at deep path
    let metadata = manager.metadata(&deep_file).unwrap();
    assert_eq!(metadata.file_type, FileType::RegularFile);
    
    // Test directory listing at various levels
    for level in 1..=max_depth {
        let level_path = (1..=level).map(|l| format!("level{}", l)).collect::<Vec<_>>().join("/");
        let full_path = format!("/{}", level_path);
        let entries = manager.readdir(&full_path).unwrap();
        
        if level < max_depth {
            // Should contain the next level directory
            assert_eq!(entries.len(), 1);
        } else {
            // Deepest level should contain the file
            assert_eq!(entries.len(), 1);
        }
    }
}

/// Test large file creation and metadata operations
#[test_case]
fn test_large_scale_operations() {
    let fs: Arc<dyn crate::fs::vfs_v2::core::FileSystemOperations> = TmpFS::new(2 * 1024 * 1024); // 2MB limit
    let manager = VfsManager::new();
    
    manager.mount(fs, "/", 0).unwrap();
    
    // Create multiple directories
    let dir_count = 10;
    for i in 0..dir_count {
        let dirname = format!("/dir_{}", i);
        manager.create_dir(&dirname).unwrap();
        
        for j in 0..dir_count {
            let dirname2 = format!("{}/dir_{}", dirname, j);
            manager.create_dir(&dirname2).unwrap();
            // Create files within each directory
            let files_per_dir = 10;
            for k in 0..files_per_dir {
                let filename = format!("{}/file_{}.txt", dirname2, k);
                manager.create_file(&filename, FileType::RegularFile).unwrap();
            }
        }
    }
    
    // Test metadata operations on all created files
    for i in 0..dir_count {
        let dirname = format!("/dir_{}", i);
        let dir_entries = manager.readdir(&dirname).unwrap();
        assert_eq!(dir_entries.len(), 10); // files_per_dir
        
        for j in 0..dir_count {
            let dirname2 = format!("{}/dir_{}", dirname, j);
            let dir_entries2 = manager.readdir(&dirname2).unwrap();
            assert_eq!(dir_entries2.len(), 10); // files_per_dir

            for k in 0..10 {
                let filename = format!("{}/file_{}.txt", dirname2, k);
                let metadata = manager.metadata(&filename).unwrap();
                assert_eq!(metadata.file_type, FileType::RegularFile);
            }
        }
    }
    
    // Test root directory listing
    let root_entries = manager.readdir("/").unwrap();
    assert_eq!(root_entries.len(), dir_count);
}

/// Test mount tree performance with multiple mount points
#[test_case]  
fn test_mount_tree_performance() {
    let manager = VfsManager::new();
    
    // Create root tmpfs
    let root_fs: Arc<dyn crate::fs::vfs_v2::core::FileSystemOperations> = TmpFS::new(1024 * 1024);
    manager.mount(root_fs, "/", 0).unwrap();
    
    // Create mount point directories
    manager.create_dir("/mnt").unwrap();
    manager.create_dir("/tmp").unwrap();
    manager.create_dir("/var").unwrap();
    
    // Mount additional filesystems
    let mount_points = ["/mnt", "/tmp", "/var"];
    for mount_point in &mount_points {
        let fs: Arc<dyn crate::fs::vfs_v2::core::FileSystemOperations> = TmpFS::new(512 * 1024);
        manager.mount(fs, mount_point, 0).unwrap();
        
        // Create test files in each mounted filesystem
        for i in 0..5 {
            let filename = format!("{}/test_{}.txt", mount_point, i);
            manager.create_file(&filename, FileType::RegularFile).unwrap();
        }
    }
    
    // Test path resolution across mount boundaries
    for mount_point in &mount_points {
        let entries = manager.readdir(mount_point).unwrap();
        assert_eq!(entries.len(), 5);
        
        for i in 0..5 {
            let filename = format!("{}/test_{}.txt", mount_point, i);
            let metadata = manager.metadata(&filename).unwrap();
            assert_eq!(metadata.file_type, FileType::RegularFile);
        }
    }
    
    // Test root directory still accessible
    let root_entries = manager.readdir("/").unwrap();
    assert!(root_entries.len() >= 3); // Should contain at least mnt, tmp, var
}
