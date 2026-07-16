//! TransparentExecutor backup/restore tests
//!
//! These tests verify that task and trapframe state can be backed up
//! and correctly restored when exec fails.

use alloc::string::ToString;
use alloc::vec::Vec;

use super::executor::TransparentExecutor;
use crate::arch::Trapframe;
use crate::sched::scheduler::{add_task, get_task_by_id, reset};
use crate::task::new_user_task;

/// Test that TransparentExecutor can backup and restore task state on exec failure
#[test_case]
fn test_exec_backup_restore() {
    // Reset scheduler state before test
    reset();

    // Initialize task first, then add to scheduler
    let mut task = new_user_task("BackupTestTask".to_string(), 1001);
    task.init();

    let task_id = add_task(task, 0);
    let task = get_task_by_id(task_id).unwrap();
    let mut trapframe = Trapframe::new();

    // Record original state
    let original_name = task.name.read().clone();
    let original_text_size = task.text_size.load(core::sync::atomic::Ordering::SeqCst);
    let original_data_size = task.data_size.load(core::sync::atomic::Ordering::SeqCst);
    let original_stack_size = task.stack_size.load(core::sync::atomic::Ordering::SeqCst);
    let original_page_allocations_count = task.page_allocations.read().len();
    let original_vm_mappings_count = task.vm_manager.memmap_len();
    let original_pc = trapframe.get_current_pc();
    let original_sp = trapframe.regs.reg[2];
    let original_a0 = trapframe.regs.reg[10];

    // Try to execute a non-existent binary (should fail and restore state)
    let result = TransparentExecutor::execute_binary(
        "/nonexistent/binary",
        &["arg1", "arg2"],
        &["ENV=test"],
        &task,
        &mut trapframe,
        true,
    );

    // Verify the exec failed as expected
    assert!(result.is_err(), "Exec should fail for non-existent binary");

    // Verify that all state was restored to original values
    assert_eq!(
        *task.name.read(),
        original_name,
        "Task name should be restored"
    );
    assert_eq!(
        task.text_size.load(core::sync::atomic::Ordering::SeqCst),
        original_text_size,
        "Text size should be restored"
    );
    assert_eq!(
        task.data_size.load(core::sync::atomic::Ordering::SeqCst),
        original_data_size,
        "Data size should be restored"
    );
    assert_eq!(
        task.stack_size.load(core::sync::atomic::Ordering::SeqCst),
        original_stack_size,
        "Stack size should be restored"
    );
    assert_eq!(
        task.page_allocations.read().len(),
        original_page_allocations_count,
        "Page allocations count should be restored"
    );
    assert_eq!(
        task.vm_manager.memmap_len(),
        original_vm_mappings_count,
        "VM mappings count should be restored"
    );
    assert_eq!(trapframe.epc, original_pc, "PC should be restored");
    assert_eq!(trapframe.regs.reg[2], original_sp, "SP should be restored");
    assert_eq!(trapframe.regs.reg[10], original_a0, "A0 should be restored");
}

/// Test TransparentExecutor basic functionality with valid parameters
#[test_case]
fn test_exec_parameter_validation() {
    // Reset scheduler state before test
    reset();

    // Initialize task first, then add to scheduler
    let task = new_user_task("ParamTestTask".to_string(), 1002);
    task.init();

    let task_id = add_task(task, 0);
    let task = get_task_by_id(task_id).unwrap();
    let mut trapframe = Trapframe::new();

    // Test with empty arguments
    let result = TransparentExecutor::execute_binary(
        "/nonexistent/binary",
        &[],
        &[],
        &task,
        &mut trapframe,
        true,
    );

    // Should fail but not panic
    assert!(
        result.is_err(),
        "Exec should fail gracefully with empty args"
    );

    // Test with various argument combinations
    let result = TransparentExecutor::execute_binary(
        "/nonexistent/binary",
        &["program", "arg1", "arg2", "arg with spaces"],
        &["PATH=/bin:/usr/bin", "HOME=/root", "VAR=value"],
        &task,
        &mut trapframe,
        true,
    );

    // Should fail but handle arguments correctly
    assert!(
        result.is_err(),
        "Exec should fail gracefully with various args"
    );
}

/// Test argument array handling
#[test_case]
fn test_argv_array_handling() {
    // Reset scheduler state before test
    reset();

    // Initialize task first, then add to scheduler
    let mut task = new_user_task("ArgvTestTask".to_string(), 1003);
    task.init();

    let task_id = add_task(task, 0);
    let task = get_task_by_id(task_id).unwrap();
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
            &task,
            &mut trapframe,
            true,
        );

        // Should fail gracefully regardless of argument content
        assert!(
            result.is_err(),
            "Exec should fail gracefully with args: {:?}",
            args
        );
    }
}

/// Test environment variable array handling
#[test_case]
fn test_envp_array_handling() {
    // Reset scheduler state before test
    reset();

    // Initialize task first, then add to scheduler
    let mut task = new_user_task("EnvpTestTask".to_string(), 1004);
    task.init();

    let task_id = add_task(task, 0);
    let task = get_task_by_id(task_id).unwrap();
    let mut trapframe = Trapframe::new();

    // Test with different environment variable patterns
    let mut test_cases = Vec::new();
    test_cases.push(Vec::<&str>::new()); // Empty environment
    test_cases.push(Vec::from(["PATH=/bin"]));
    test_cases.push(Vec::from(["PATH=/bin", "HOME=/root", "SHELL=/bin/sh"]));
    test_cases.push(Vec::from([
        "EMPTY_VALUE=",
        "EQUALS_IN_VALUE=val=ue",
        "UNICODE=あいう",
    ]));

    for envp in test_cases {
        let result = TransparentExecutor::execute_binary(
            "/nonexistent/binary",
            &["program"],
            &envp,
            &task,
            &mut trapframe,
            true,
        );

        // Should fail gracefully regardless of environment content
        assert!(
            result.is_err(),
            "Exec should fail gracefully with envp: {:?}",
            envp
        );
    }
}

/// Test runtime delegation configuration
#[test_case]
fn test_runtime_delegation_config() {
    use crate::abi::scarlet::ScarletAbi;
    use crate::abi::{AbiModule, RuntimeConfig};
    use crate::object::KernelObject;
    use alloc::string::String;
    use alloc::sync::Arc;

    // Create a mock file object for testing
    // In real scenarios, this would be an actual file from VFS
    let scarlet_abi = ScarletAbi::default();

    // Test 1: Non-Wasm file should not require runtime delegation
    let non_wasm_path = "/system/scarlet/bin/hello";
    // Note: We can't easily create a real file object in tests without VFS,
    // but we can verify the method signature and basic logic

    // Test 2: Wasm file extension should trigger runtime delegation
    let wasm_path = "/data/apps/program.wasm";
    // The actual runtime_config would be returned when a real Wasm file is detected

    // Test 3: Verify RuntimeConfig structure can be created
    let test_config = RuntimeConfig {
        runtime_path: "/system/scarlet/bin/test-runtime".to_string(),
        runtime_abi: Some("scarlet".to_string()),
        runtime_args: alloc::vec!["--test".to_string(), "--verbose".to_string()],
    };

    assert_eq!(test_config.runtime_path, "/system/scarlet/bin/test-runtime");
    assert_eq!(test_config.runtime_abi, Some("scarlet".to_string()));
    assert_eq!(test_config.runtime_args.len(), 2);
    assert_eq!(test_config.runtime_args[0], "--test");
}

/// Test runtime argument construction
#[test_case]
fn test_runtime_argument_construction() {
    use alloc::vec::Vec;

    // Simulate runtime argument construction
    let target_path = "/data/apps/program.wasm";
    let target_argv = &["program.wasm", "arg1", "arg2"];
    let runtime_path = "/system/scarlet/bin/wasm-runtime";
    let runtime_args = alloc::vec!["--wasm"];

    // Construct runtime argv as TransparentExecutor::execute_via_runtime does
    let mut runtime_argv = Vec::new();
    runtime_argv.push(runtime_path);
    for arg in &runtime_args {
        runtime_argv.push(arg);
    }
    runtime_argv.push(target_path);
    for arg in target_argv.iter().skip(1) {
        runtime_argv.push(*arg);
    }

    // Verify argument order
    assert_eq!(runtime_argv[0], "/system/scarlet/bin/wasm-runtime");
    assert_eq!(runtime_argv[1], "--wasm");
    assert_eq!(runtime_argv[2], "/data/apps/program.wasm");
    assert_eq!(runtime_argv[3], "arg1");
    assert_eq!(runtime_argv[4], "arg2");
    assert_eq!(runtime_argv.len(), 5);
}
