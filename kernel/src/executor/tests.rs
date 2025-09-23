//! TransparentExecutor backup/restore tests
//!
//! These tests verify that task and trapframe state can be backed up
//! and correctly restored when exec fails.

use alloc::string::ToString;
use alloc::vec::Vec;

use super::executor::TransparentExecutor;
use crate::task::new_user_task;
use crate::arch::Trapframe;

/// Test that TransparentExecutor can backup and restore task state on exec failure
#[test_case]
fn test_exec_backup_restore() {
    let mut task = new_user_task("BackupTestTask".to_string(), 1001);
    task.init();
    let mut trapframe = Trapframe::new();
    
    // Record original state
    let original_name = task.name.clone();
    let original_text_size = task.text_size;
    let original_data_size = task.data_size;
    let original_stack_size = task.stack_size;
    let original_managed_pages_count = task.managed_pages.len();
    let original_vm_mappings_count = task.vm_manager.memmap_len();
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
    assert_eq!(task.vm_manager.memmap_len(), original_vm_mappings_count, "VM mappings count should be restored");
    assert_eq!(trapframe.epc, original_pc, "PC should be restored");
    assert_eq!(trapframe.regs.reg[2], original_sp, "SP should be restored");
    assert_eq!(trapframe.regs.reg[10], original_a0, "A0 should be restored");
}

/// Test TransparentExecutor basic functionality with valid parameters
#[test_case]
fn test_exec_parameter_validation() {
    let mut task = new_user_task("ParamTestTask".to_string(), 1002);
    task.init();
    let mut trapframe = Trapframe::new();
    
    // Test with empty arguments
    let result = TransparentExecutor::execute_binary(
        "/nonexistent/binary",
        &[],
        &[],
        &mut task,
        &mut trapframe,
        true
    );
    
    // Should fail but not panic
    assert!(result.is_err(), "Exec should fail gracefully with empty args");
    
    // Test with various argument combinations
    let result = TransparentExecutor::execute_binary(
        "/nonexistent/binary",
        &["program", "arg1", "arg2", "arg with spaces"],
        &["PATH=/bin:/usr/bin", "HOME=/root", "VAR=value"],
        &mut task,
        &mut trapframe,
        true
    );
    
    // Should fail but handle arguments correctly
    assert!(result.is_err(), "Exec should fail gracefully with various args");
}

/// Test argument array handling
#[test_case] 
fn test_argv_array_handling() {
    let mut task = new_user_task("ArgvTestTask".to_string(), 1003);
    task.init();
    let mut trapframe = Trapframe::new();
    
    // Test with different argument patterns
    let mut test_cases = Vec::new();
    test_cases.push(Vec::from(["program"]));
    test_cases.push(Vec::from(["program", "single_arg"]));
    test_cases.push(Vec::from(["program", "arg1", "arg2", "arg3"]));
    test_cases.push(Vec::from(["program", "", "empty_arg_test"]));
    test_cases.push(Vec::from(["program", "unicode_test_あいう"]));
    
    for args in test_cases {
        let arg_refs: Vec<&str> = args.iter().map(|s| s.as_ref()).collect();
        let result = TransparentExecutor::execute_binary(
            "/nonexistent/binary",
            &arg_refs,
            &["TEST=1"],
            &mut task,
            &mut trapframe,
            true
        );
        
        // Should fail gracefully regardless of argument content
        assert!(result.is_err(), "Exec should fail gracefully with args: {:?}", args);
    }
}

/// Test environment variable array handling
#[test_case]
fn test_envp_array_handling() {
    let mut task = new_user_task("EnvpTestTask".to_string(), 1004);
    task.init();
    let mut trapframe = Trapframe::new();
    
    // Test with different environment variable patterns
    let mut test_cases = Vec::new();
    test_cases.push(Vec::<&str>::new());  // Empty environment
    test_cases.push(Vec::from(["PATH=/bin"]));
    test_cases.push(Vec::from(["PATH=/bin", "HOME=/root", "SHELL=/bin/sh"]));
    test_cases.push(Vec::from(["EMPTY_VALUE=", "EQUALS_IN_VALUE=val=ue", "UNICODE=あいう"]));
    
    for envp in test_cases {
        let result = TransparentExecutor::execute_binary(
            "/nonexistent/binary",
            &["program"],
            &envp,
            &mut task,
            &mut trapframe,
            true
        );
        
        // Should fail gracefully regardless of environment content
        assert!(result.is_err(), "Exec should fail gracefully with envp: {:?}", envp);
    }
}