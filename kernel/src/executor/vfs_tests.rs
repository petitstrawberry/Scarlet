//! VFS inheritance tests for TransparentExecutor
//!
//! These tests verify that VFS inheritance works correctly during exec.

use alloc::{sync::Arc, string::ToString, boxed::Box};

use super::executor::TransparentExecutor;
use crate::{
    task::{Task, new_user_task},
    arch::Trapframe,
    fs::{VfsManager, FileType},
    abi::{AbiRegistry, AbiModule, scarlet::ScarletAbi},
};

/// Test that VFS inheritance works correctly during exec with unified VFS architecture
#[test_case]
fn test_vfs_inheritance_basic() {
    // Register Scarlet ABI for testing
    AbiRegistry::register::<ScarletAbi>();
    
    // Create a parent task with unified VFS
    let mut parent_task = new_user_task("ParentTask".to_string(), 1001);
    parent_task.init();
    
    // Create unified VFS with basic directories
    let vfs = VfsManager::new();
    let fs = Box::new(crate::fs::drivers::tmpfs::TmpFS::new(1024 * 1024)); // 1MB limit
    let fs_id = vfs.register_fs(fs);
    vfs.mount(fs_id, "/").unwrap();
    
    // Create directories in VFS
    vfs.create_dir("/home").unwrap();
    vfs.create_dir("/home/user").unwrap();
    vfs.create_dir("/tmp").unwrap();
    vfs.create_dir("/etc").unwrap();
    
    // Create a test file using create_file
    vfs.create_file("/home/user/test.txt", FileType::RegularFile).unwrap();
    
    // Set VFS on task (both base and active)
    let vfs_arc = Arc::new(vfs);
    parent_task.set_base_vfs(vfs_arc.clone());
    parent_task.set_vfs(vfs_arc);
    parent_task.cwd = Some("/home/user".to_string());
    
    // Record initial state  
    let _original_cwd = parent_task.cwd.clone();
    
    // Try VFS inheritance with a non-existent binary (should fail but VFS should be set up)
    let mut trapframe = Trapframe::new(0);
    let result = TransparentExecutor::execute_binary(
        "/nonexistent/binary",
        &["arg1", "arg2"],
        &["ENV=test"],
        &mut parent_task,
        &mut trapframe,
    );
    
    // Verify the exec failed as expected (binary doesn't exist)
    assert!(result.is_err(), "Exec should fail for non-existent binary");
    
    // Verify that VFS was restored to original after failure
    assert!(parent_task.get_vfs().is_some(), "VFS should be restored");
    assert_eq!(parent_task.cwd, _original_cwd, "CWD should be restored");
    
    // Check that the VFS still has access to original directories
    let current_vfs = parent_task.get_vfs().unwrap();
    assert!(current_vfs.has_mount_point("/"), "Root should be mounted in VFS");
    
    // Note: Due to backup/restore, the actual inheritance testing requires
    // a successful exec, which needs a real binary file
}

/// Test VFS inheritance with successful ABI switching using unified VFS architecture
#[test_case]  
fn test_vfs_inheritance_abi_switch() {
    // Register Scarlet ABI for testing
    AbiRegistry::register::<ScarletAbi>();
    
    // Create a task with unified VFS
    let mut task = new_user_task("TestTask".to_string(), 1002);
    task.init();
    
    // Create unified VFS with directories
    let vfs = VfsManager::new();
    let fs = Box::new(crate::fs::drivers::tmpfs::TmpFS::new(1024 * 1024)); // 1MB limit
    let fs_id = vfs.register_fs(fs);
    vfs.mount(fs_id, "/").unwrap();
    
    // Create standard directories
    vfs.create_dir("/home").unwrap();
    vfs.create_dir("/tmp").unwrap();
    vfs.create_dir("/etc").unwrap();
    vfs.create_dir("/var").unwrap();
    vfs.create_dir("/usr").unwrap();
    vfs.create_dir("/usr/share").unwrap();
    
    task.set_base_vfs(Arc::new(vfs));
    
    // Test ABI module VFS setup
    let abi = crate::abi::AbiRegistry::instantiate("scarlet").unwrap();
    
    // Test setup_abi_vfs with base VFS
    let abi_vfs = abi.setup_abi_vfs(task.get_base_vfs().unwrap()).unwrap();
    
    // Set as active VFS
    task.set_vfs(abi_vfs);
    
    // Verify ABI VFS has the expected structure
    assert!(task.get_vfs().unwrap().has_mount_point("/"), "Root should be mounted in ABI VFS");
    
    // Test create_initial_vfs
    let initial_vfs = abi.create_initial_vfs().unwrap();
    assert!(initial_vfs.has_mount_point("/"), "Initial VFS should have root");
    
    // Test default working directory
    let default_cwd = abi.get_default_cwd();
    assert_eq!(default_cwd, "/", "Default CWD should be root");
}



/// Test VFS inheritance functionality directly with unified VFS architecture
#[test_case]
fn test_abi_vfs_methods() {
    let abi = ScarletAbi::default();
    
    // Test create_initial_vfs
    let initial_vfs = abi.create_initial_vfs().unwrap();
    
    // Verify root directory exists and standard directories are created
    assert!(initial_vfs.has_mount_point("/"), "Root should be mounted");
    
    // Test setup_abi_vfs with no existing VFS
    let abi_vfs = abi.setup_abi_vfs(&initial_vfs).unwrap();
    assert!(abi_vfs.has_mount_point("/"), "ABI VFS should have root mounted");
    
    // Test setup_abi_vfs with existing VFS
    let abi_vfs_with_existing = abi.setup_abi_vfs(&initial_vfs).unwrap();
    assert!(abi_vfs_with_existing.has_mount_point("/"), "ABI VFS with existing should have root mounted");
    
    // Test get_default_cwd
    let default_cwd = abi.get_default_cwd();
    assert_eq!(default_cwd, "/", "Default CWD should be root");
}

/// Test VFS inheritance during executor operations with unified VFS architecture
#[test_case]
fn test_executor_vfs_inheritance() {
    // Register Scarlet ABI for testing
    AbiRegistry::register::<ScarletAbi>();
    
    // Create a parent task with unified VFS
    let mut parent_task = new_user_task("ParentTask".to_string(), 1001);
    parent_task.init();
    
    // Create unified VFS with some directories
    let vfs = VfsManager::new();
    let fs = Box::new(crate::fs::drivers::tmpfs::TmpFS::new(1024 * 1024)); // 1MB limit
    let fs_id = vfs.register_fs(fs);
    vfs.mount(fs_id, "/").unwrap();
    
    // Create directories and files in VFS
    vfs.create_dir("/home").unwrap();
    vfs.create_dir("/tmp").unwrap();
    vfs.create_file("/tmp/test_file.txt", FileType::RegularFile).unwrap();
    
    // Set VFS on task (both base and active)
    let vfs_arc = Arc::new(vfs);
    parent_task.set_base_vfs(vfs_arc.clone());
    parent_task.set_vfs(vfs_arc);
    parent_task.cwd = Some("/tmp".to_string());
    
    // Create executor and test VFS inheritance preparation
    // Note: prepare_vfs_inheritance is private, so we test the public API that uses it
    
    // Test that a task starts with base VFS and gets ABI VFS created properly
    let original_base_vfs = parent_task.get_base_vfs().cloned();
    let _original_cwd = parent_task.cwd.clone();
    
    // Simulate what happens during exec - base VFS should be preserved, active VFS should be recreated
    // We can't directly call prepare_vfs_inheritance since it's private,
    // but we can verify the VFS inheritance logic through the ABI methods
    
    let abi = ScarletAbi::default();
    if let Some(base_vfs_ref) = &original_base_vfs {
        let abi_vfs = abi.setup_abi_vfs(base_vfs_ref).unwrap();
        
        // Verify the ABI VFS has proper structure
        assert!(abi_vfs.has_mount_point("/"), "ABI VFS should have root mounted");
        
        // Update task with ABI VFS (base VFS remains unchanged)
        parent_task.set_vfs(abi_vfs);
        parent_task.cwd = Some(abi.get_default_cwd().to_string());
    }
    
    assert!(parent_task.get_base_vfs().is_some(), "Task should have base VFS after inheritance");
    assert!(parent_task.get_vfs().is_some(), "Task should have active VFS after inheritance");
    assert!(parent_task.cwd.is_some(), "Task should have CWD after inheritance");
}
