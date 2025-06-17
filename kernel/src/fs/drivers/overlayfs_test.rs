//! Tests for OverlayFS implementation
//! 
//! These tests validate the OverlayFS functionality including layer management,
//! file operations, and read-only behavior enforcement.

use super::overlayfs::OverlayFS;
use super::super::*;
use alloc::{string::String, vec::Vec};

/// Test basic OverlayFS creation
/// 
/// This test validates that an OverlayFS can be created with no upper layer
/// (read-only mode) and that it correctly identifies itself as "overlayfs".
#[test_case]
pub fn test_overlay_creation() {
    // Test read-only overlay (no upper layer)
    let lower_mount_nodes = Vec::new();
    let lower_relative_paths = Vec::new();
    
    let overlay = OverlayFS::new(
        None,
        String::new(),
        lower_mount_nodes,
        lower_relative_paths,
    );
    
    assert!(overlay.is_ok(), "Should be able to create read-only overlay");
    
    let overlay = overlay.unwrap();
    assert_eq!(overlay.name(), "overlayfs", "Filesystem name should be 'overlayfs'");
}

/// Test OverlayFS creation with mismatched arrays
/// 
/// This test ensures that OverlayFS creation fails when the number of
/// lower mount nodes doesn't match the number of lower relative paths.
#[test_case]
pub fn test_overlay_creation_mismatched_arrays() {
    let lower_mount_nodes = Vec::new();
    let lower_relative_paths = vec!["/test".to_string()];
    
    let overlay = OverlayFS::new(
        None,
        String::new(),
        lower_mount_nodes,
        lower_relative_paths,
    );
    
    assert!(overlay.is_err(), "Should fail with mismatched array lengths");
}

/// Test that read-only overlay rejects write operations
/// 
/// This test verifies that an OverlayFS without an upper layer correctly
/// rejects write operations like file creation, directory creation, and removal.
#[test_case]
pub fn test_readonly_overlay_rejects_writes() {
    let overlay = OverlayFS::new(
        None, // No upper layer = read-only
        String::new(),
        Vec::new(),
        Vec::new(),
    ).unwrap();
    
    // These operations should fail because there's no upper layer
    let create_result = overlay.create_file("/test.txt", FileType::RegularFile);
    assert!(create_result.is_err(), "Should fail to create file on read-only overlay");
    
    let mkdir_result = overlay.create_dir("/test_dir");
    assert!(mkdir_result.is_err(), "Should fail to create directory on read-only overlay");
    
    let remove_result = overlay.remove("/test.txt");
    assert!(remove_result.is_err(), "Should fail to remove file on read-only overlay");
}
