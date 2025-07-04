//! TransparentExecutor backup/restore tests
//!
//! These tests verify that task and trapframe state can be backed up
//! and correctly restored when exec fails.

use alloc::string::ToString;

use super::executor::TransparentExecutor;
use crate::task::new_user_task;
use crate::arch::Trapframe;

/// Test that TransparentExecutor can backup and restore task state on exec failure
#[test_case]
fn test_exec_backup_restore() {
    let mut task = new_user_task("BackupTestTask".to_string(), 1001);
    task.init();
    let mut trapframe = Trapframe::new(0);
    
    // Record original state
    let original_name = task.name.clone();
    let original_text_size = task.text_size;
    let original_data_size = task.data_size;
    let original_stack_size = task.stack_size;
    let original_managed_pages_count = task.managed_pages.len();
    let original_vm_mappings_count = task.vm_manager.get_memmap().len();
    let original_pc = trapframe.epc;
    let original_sp = trapframe.regs.reg[2];
    let original_a0 = trapframe.regs.reg[10];
    
    // Try to execute a non-existent binary (should fail and restore state)
    let result = TransparentExecutor::execute_binary(
        "/nonexistent/binary",
        &["arg1", "arg2"],
        &["ENV=test"],
        &mut task,
        &mut trapframe,
        true
    );
    
    // Verify the exec failed as expected
    assert!(result.is_err(), "Exec should fail for non-existent binary");
    
    // Verify that all state was restored to original values
    assert_eq!(task.name, original_name, "Task name should be restored");
    assert_eq!(task.text_size, original_text_size, "Text size should be restored");
    assert_eq!(task.data_size, original_data_size, "Data size should be restored");
    assert_eq!(task.stack_size, original_stack_size, "Stack size should be restored");
    assert_eq!(task.managed_pages.len(), original_managed_pages_count, "Managed pages count should be restored");
    assert_eq!(task.vm_manager.get_memmap().len(), original_vm_mappings_count, "VM mappings count should be restored");
    assert_eq!(trapframe.epc, original_pc, "PC should be restored");
    assert_eq!(trapframe.regs.reg[2], original_sp, "SP should be restored");
    assert_eq!(trapframe.regs.reg[10], original_a0, "A0 should be restored");
}

/// Test environment variable handling in TransparentExecutor
#[test_case]
fn test_envp_to_task_env_conversion() {
    let mut task = new_user_task("EnvTestTask".to_string(), 1002);
    task.init();
    
    // Set some initial environment variables
    task.set_env("OLD_VAR".to_string(), "old_value".to_string());
    task.set_env("PATH".to_string(), "/old/path".to_string());
    
    // Simulate envp array (like in execve)
    let envp = [
        "PATH=/usr/bin:/bin",
        "HOME=/home/user", 
        "SHELL=/bin/sh",
        "NEW_VAR=new_value"
    ];
    
    // Convert envp to task environment variables
    TransparentExecutor::set_task_env_from_envp(&mut task, &envp);
    
    // Verify old variables are cleared and new ones are set
    assert_eq!(task.get_env("OLD_VAR"), None, "Old variables should be cleared");
    assert_eq!(task.get_env("PATH"), Some(&"/usr/bin:/bin".to_string()), "PATH should be updated");
    assert_eq!(task.get_env("HOME"), Some(&"/home/user".to_string()), "HOME should be set");
    assert_eq!(task.get_env("SHELL"), Some(&"/bin/sh".to_string()), "SHELL should be set");
    assert_eq!(task.get_env("NEW_VAR"), Some(&"new_value".to_string()), "NEW_VAR should be set");
    
    // Verify total number of environment variables
    assert_eq!(task.get_env_map().len(), 4, "Should have exactly 4 environment variables");
}

/// Test malformed environment variable handling
#[test_case]
fn test_malformed_envp_handling() {
    let mut task = new_user_task("MalformedEnvTestTask".to_string(), 1003);
    task.init();
    
    // Test with malformed environment variables (no '=' sign or empty key)
    let envp = [
        "VALID_VAR=valid_value",
        "INVALID_VAR_NO_EQUALS",        // No '=' sign - invalid
        "ANOTHER_VALID=another_value",
        "=EMPTY_KEY_INVALID",          // Empty key - invalid
        ""                             // Empty string - invalid
    ];
    
    TransparentExecutor::set_task_env_from_envp(&mut task, &envp);
    
    // Only valid variables should be set
    assert_eq!(task.get_env("VALID_VAR"), Some(&"valid_value".to_string()), "Valid variable should be set");
    assert_eq!(task.get_env("ANOTHER_VALID"), Some(&"another_value".to_string()), "Another valid variable should be set");
    assert_eq!(task.get_env("INVALID_VAR_NO_EQUALS"), None, "Invalid variable should not be set");
    assert_eq!(task.get_env(""), None, "Empty key should not be set");
    
    // Should have exactly 2 environment variables
    assert_eq!(task.get_env_map().len(), 2, "Should have exactly 2 valid environment variables");
}