/// Performance and stress tests for VFS v2
/// 
/// This module contains performance-focused tests for VFS v2:
/// - Mount/unmount performance
/// - File creation/deletion throughput
/// - Path resolution performance
/// - Memory usage optimization
/// - Stress testing with many files/mounts

use crate::fs::vfs_v2::{
    core::*,
    manager_v2::VfsManager,
    mount_tree_v2::{MountTree, MountPoint, MountId, MountType},
    tmpfs_v2::TmpFS,
};
use crate::fs::{FileSystemError, FileType};
use alloc::{
    collections::BTreeMap, format, string::{String, ToString}, sync::Arc, vec::Vec, vec,
};

/// Test performance of many file operations
#[test_case]
fn test_many_file_operations() {
    let manager = VfsManager::new();
    
    // Create many files to test performance
    const NUM_FILES: usize = 100;
    
    // Test file creation performance
    for i in 0..NUM_FILES {
        let file_path = format!("/test_file_{}.txt", i);
        let result = manager.create_file(&file_path, FileType::RegularFile);
        assert!(result.is_ok(), "Failed to create file {}: {:?}", i, result.err());
    }
    
    // Test file access performance
    for i in 0..NUM_FILES {
        let file_path = format!("/test_file_{}.txt", i);
        let result = manager.open(&file_path, 0);
        assert!(result.is_ok(), "Failed to open file {}: {:?}", i, result.err());
    }
}

/// Test performance of many mount operations
#[test_case]
fn test_many_mount_operations() {
    let manager = VfsManager::new();
    
    // Create many mounts to test mount tree performance
    const NUM_MOUNTS: usize = 20; // Reduced for kernel testing
    
    for i in 0..NUM_MOUNTS {
        let tmpfs = Arc::new(TmpFS::new(64 * 1024)); // Smaller size
        let mount_path = format!("/mnt{}", i);
        let result = manager.mount(tmpfs, &mount_path, 0);
        assert!(result.is_ok(), "Failed to mount {}: {:?}", i, result.err());
    }
    
    // Test accessing files in each mount
    for i in 0..NUM_MOUNTS {
        let file_path = format!("/mnt{}/test.txt", i);
        let result = manager.create_file(&file_path, FileType::RegularFile);
        assert!(result.is_ok(), "Failed to create file in mount {}: {:?}", i, result.err());
    }
}

/// Test path resolution performance with deep hierarchies
#[test_case]
fn test_deep_path_resolution() {
    let manager = VfsManager::new();
    
    // Create deep directory hierarchy
    const DEPTH: usize = 20;
    let mut current_path = String::new();
    
    for level in 0..DEPTH {
        current_path.push_str(&format!("/level{}", level));
        let result = manager.create_dir(&current_path);
        assert!(result.is_ok(), "Failed to create directory at depth {}: {:?}", level, result.err());
    }
    
    // Test path resolution performance at maximum depth
    let deep_file_path = format!("{}/deep_file.txt", current_path);
    let result = manager.create_file(&deep_file_path, FileType::RegularFile);
    assert!(result.is_ok(), "Failed to create deep file: {:?}", result.err());
    
    // // Test path normalization performance with complex path
    // let complex_path = format!("{}/./nested/../nested/./file.txt", current_path);
    // let normalized = manager.normalize_path(&complex_path);
    // let expected = format!("{}/nested/file.txt", current_path);
    // assert_eq!(normalized, expected);
}

/// Test mount tree performance with many mount points
#[test_case]
fn test_mount_tree_performance() {
    // Create root filesystem
    let tmpfs = TmpFS::new(1024 * 1024);
    let root_node = tmpfs.root_node();
    let root_entry = VfsEntry::new(None,  "/".to_string(), root_node);
    
    // Create mount tree
    let mount_tree = MountTree::new(root_entry);
    
    // Test performance of path resolution
    const NUM_LOOKUPS: usize = 100;
    
    for i in 0..NUM_LOOKUPS {
        let path = format!("/test_path_{}", i);
        // Test path parsing performance
        let components = mount_tree.parse_path(&path);
        assert!(!components.is_empty(), "Path components should not be empty for path: {}", path);
    }
}

// /// Test VFS v2 memory usage with many VfsEntry objects
// #[test_case]
// fn test_memory_usage_many_entries() {
//     // Create many VfsEntry objects to test memory usage
//     const NUM_ENTRIES: usize = 100;
//     let mut entries = Vec::new();
    
//     for i in 0..NUM_ENTRIES {
//         // Create mock node for each entry
//         let tmpfs = TmpFS::new(64 * 1024);
//         let node = tmpfs.root().expect("Failed to get root node");
//         let entry = VfsEntry::new(format!("entry_{}", i), node, None);
//         let entry_arc = Arc::new(entry);
//         entries.push(entry_arc);
//     }
    
//     // Test that all entries are accessible
//     for (i, entry) in entries.iter().enumerate() {
//         assert_eq!(entry.name(), &format!("entry_{}", i));
//         assert_eq!(entry.file_type(), FileType::Directory); // TmpFS root is directory
//     }
    
//     // Test reference counting
//     let clone_entries: Vec<_> = entries.iter().cloned().collect();
//     assert_eq!(entries.len(), clone_entries.len());
    
//     // Verify entries are still valid after cloning
//     for (original, clone) in entries.iter().zip(clone_entries.iter()) {
//         assert_eq!(original.name(), clone.name());
//     }
// }

/// Test concurrent access simulation with high load
#[test_case]
fn test_high_load_concurrent_access() {
    let manager = Arc::new(VfsManager::new());
    
    // Simulate high concurrent load
    const NUM_OPERATIONS: usize = 50;
    
    // First batch: file creation
    let manager1 = manager.clone();
    for i in 0..NUM_OPERATIONS {
        let result = manager1.create_file(&format!("/batch1_file_{}.txt", i), FileType::RegularFile);
        assert!(result.is_ok(), "Failed batch1 file creation {}: {:?}", i, result.err());
    }
    
    // Second batch: directory creation
    let manager2 = manager.clone();
    for i in 0..NUM_OPERATIONS {
        let result = manager2.create_dir(&format!("/batch2_dir_{}", i));
        assert!(result.is_ok(), "Failed batch2 directory creation {}: {:?}", i, result.err());
    }
    
    // Third batch: file access
    let manager3 = manager.clone();
    for i in 0..NUM_OPERATIONS {
        let result = manager3.open(&format!("/batch1_file_{}.txt", i), 0);
        assert!(result.is_ok(), "Failed batch3 file access {}: {:?}", i, result.err());
    }
    
    // Verify all operations completed successfully
    for i in 0..NUM_OPERATIONS {
        // Check batch1 files exist
        let file_result = manager.open(&format!("/batch1_file_{}.txt", i), 0);
        assert!(file_result.is_ok(), "Batch1 file {} missing: {:?}", i, file_result.err());
        
        // Check batch2 directories exist (by trying to create a file in them)
        let nested_file = manager.create_file(&format!("/batch2_dir_{}/nested.txt", i), FileType::RegularFile);
        assert!(nested_file.is_ok(), "Batch2 directory {} inaccessible: {:?}", i, nested_file.err());
    }
}

/// Test VFS v2 with very wide directory structures
#[test_case]
fn test_wide_directory_structures() {
    let manager = VfsManager::new();
    
    // Create wide directory structure (many files in one directory)
    const NUM_FILES_PER_DIR: usize = 100;
    
    // Create a test directory
    let test_dir_result = manager.create_dir("/wide_test");
    assert!(test_dir_result.is_ok(), "Failed to create test directory: {:?}", test_dir_result.err());
    
    // Create many files in the directory
    for i in 0..NUM_FILES_PER_DIR {
        let file_path = format!("/wide_test/file_{:03}.txt", i);
        let result = manager.create_file(&file_path, FileType::RegularFile);
        assert!(result.is_ok(), "Failed to create wide file {}: {:?}", i, result.err());
    }
    
    // Test accessing files in the wide directory
    for i in 0..NUM_FILES_PER_DIR {
        let file_path = format!("/wide_test/file_{:03}.txt", i);
        let result = manager.open(&file_path, 0);
        assert!(result.is_ok(), "Failed to access wide file {}: {:?}", i, result.err());
    }
}

/// Test mount/unmount cycle performance
#[test_case]
fn test_mount_unmount_cycles() {
    let manager = VfsManager::new();
    
    // Test repeated mount/unmount cycles
    const NUM_CYCLES: usize = 10;
    
    for cycle in 0..NUM_CYCLES {
        // Mount filesystem
        let tmpfs = Arc::new(TmpFS::new(256 * 1024));
        let mount_path = format!("/cycle_{}", cycle);
        let mount_result = manager.mount(tmpfs, &mount_path, 0);
        assert!(mount_result.is_ok(), "Failed to mount cycle {}: {:?}", cycle, mount_result.err());
        
        // Create some files
        for file_num in 0..5 {
            let file_path = format!("{}/file_{}.txt", mount_path, file_num);
            let file_result = manager.create_file(&file_path, FileType::RegularFile);
            assert!(file_result.is_ok(), "Failed to create file in cycle {}: {:?}", cycle, file_result.err());
        }
        
        // Unmount filesystem
        let unmount_result = manager.unmount(&mount_path);
        assert!(unmount_result.is_ok(), "Failed to unmount cycle {}: {:?}", cycle, unmount_result.err());
        
        // Verify files are no longer accessible
        let access_result = manager.open(&format!("{}/file_0.txt", mount_path), 0);
        assert!(access_result.is_err(), "File should not be accessible after unmount in cycle {}", cycle);
    }
}

// /// Test path normalization performance with complex paths
// #[test_case]
// fn test_path_normalization_performance() {
//     let manager = VfsManager::new();
    
//     // Test normalization of complex paths
//     let complex_paths = vec![
//         "/a/b/c/d/e/f/g/h/../../../../../../../../",
//         "/very/long/path/with/many/components/and/../../redundant/../parts/./././",
//         "//multiple///slashes////everywhere/////",
//         "/a/../b/../c/../d/../e/../f/../g/../h/",
//         "/./././././././././././././././.",
//         "/a/./b/./c/./d/./e/./f/./g/./h/./",
//     ];
    
//     for path in complex_paths {
//         let normalized = manager.normalize_path(path);
//         // Verify normalization produces valid paths
//         assert!(!normalized.is_empty(), "Normalized path should not be empty for: {}", path);
//         assert!(normalized.starts_with('/'), "Normalized path should be absolute for: {}", path);
//         assert!(!normalized.contains("//"), "Normalized path should not contain double slashes: {}", normalized);
//         assert!(!normalized.contains("/./"), "Normalized path should not contain current directory references: {}", normalized);
//     }
// }

/// Test VFS v2 with alternating mount/file operations
#[test_case]
fn test_interleaved_operations() {
    let manager = VfsManager::new();
    
    // Test performance with interleaved mount and file operations
    const NUM_ITERATIONS: usize = 20;
    
    for i in 0..NUM_ITERATIONS {
        // Mount a filesystem
        let tmpfs = Arc::new(TmpFS::new(128 * 1024));
        let mount_path = format!("/interleaved_{}", i);
        let mount_result = manager.mount(tmpfs, &mount_path, 0);
        assert!(mount_result.is_ok(), "Failed interleaved mount {}: {:?}", i, mount_result.err());
        
        // Create files in root
        let root_file = manager.create_file(&format!("/root_file_{}.txt", i), FileType::RegularFile);
        assert!(root_file.is_ok(), "Failed root file creation {}: {:?}", i, root_file.err());
        
        // Create files in mounted filesystem
        let mount_file = manager.create_file(&format!("{}/mount_file.txt", mount_path), FileType::RegularFile);
        assert!(mount_file.is_ok(), "Failed mount file creation {}: {:?}", i, mount_file.err());
        
        // Access both files
        let root_access = manager.open(&format!("/root_file_{}.txt", i), 0);
        assert!(root_access.is_ok(), "Failed root file access {}: {:?}", i, root_access.err());
        
        let mount_access = manager.open(&format!("{}/mount_file.txt", mount_path), 0);
        assert!(mount_access.is_ok(), "Failed mount file access {}: {:?}", i, mount_access.err());
    }
}

/// Test VFS v2 resource cleanup under stress
#[test_case]
fn test_resource_cleanup_stress() {
    // Test that resources are properly cleaned up under stress conditions
    const NUM_STRESS_ITERATIONS: usize = 5;
    
    for iteration in 0..NUM_STRESS_ITERATIONS {
        let manager = VfsManager::new();
        
        // Create many resources
        for i in 0..20 {
            // Mount filesystem
            let tmpfs = Arc::new(TmpFS::new(64 * 1024));
            let mount_result = manager.mount(tmpfs, &format!("/stress_mnt_{}", i), 0);
            assert!(mount_result.is_ok(), "Failed stress mount {}/{}: {:?}", iteration, i, mount_result.err());
            
            // Create files
            for j in 0..5 {
                let file_result = manager.create_file(&format!("/stress_mnt_{}/file_{}.txt", i, j), FileType::RegularFile);
                assert!(file_result.is_ok(), "Failed stress file {}/{}/{}: {:?}", iteration, i, j, file_result.err());
            }
        }
        
        // Let manager go out of scope to test cleanup
        // All resources should be automatically cleaned up
    }
}

/// Test VFS v2 edge case performance
#[test_case]
fn test_edge_case_performance() {
    let manager = VfsManager::new();
    
    // Test performance with edge case paths
    let edge_case_operations = vec![
        // Very long file names (but still valid)
        ("a".repeat(100), FileType::RegularFile),
        ("b".repeat(50), FileType::Directory),
        // Files with numbers
        ("12345678901234567890".to_string(), FileType::RegularFile),
        // Files with mixed case
        ("MixedCaseFileName".to_string(), FileType::RegularFile),
    ];
    
    for (name, file_type) in edge_case_operations {
        let path = format!("/{}", name);
        let result = match file_type {
            FileType::RegularFile => manager.create_file(&path, file_type),
            FileType::Directory => manager.create_dir(&path),
            _ => continue,
        };
        assert!(result.is_ok(), "Failed edge case operation for '{}': {:?}", name, result.err());
        
        // Test access to the created item
        let access_result = manager.open(&path, 0);
        assert!(access_result.is_ok(), "Failed edge case access for '{}': {:?}", name, access_result.err());
    }
}
