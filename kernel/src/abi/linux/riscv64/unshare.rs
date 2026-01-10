//! RISC-V64 specific wrappers for unshare and setns functionality
//!
//! This module provides architecture-specific wrappers that properly handle
//! namespace updates for the LinuxRiscv64Abi type.

// Re-export the generic unshare constants and implementation
pub use crate::abi::linux::unshare::*;

use alloc::sync::Arc;
use crate::{
    abi::{AbiModule, linux::riscv64::LinuxRiscv64Abi},
    arch::Trapframe,
    task::{mytask, namespace::TaskNamespace},
};

/// sys_unshare - Architecture-specific wrapper for unshare syscall
///
/// This wrapper provides full PID namespace support by updating the
/// ABI's namespace field when CLONE_NEWPID is used.
pub fn sys_unshare(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX - 1, // -EPERM
    };

    let flags = trapframe.get_arg(0);

    trapframe.increment_pc_next(task);

    // Handle PID namespace isolation with full ABI update
    if (flags & CLONE_NEWPID) != 0 {
        // Create a new child namespace for PID isolation
        let current_namespace = abi.get_task_namespace();
        let new_namespace = TaskNamespace::new_child(
            current_namespace,
            alloc::format!("pid-ns-{}", task.get_id()),
        );
        
        // Update the ABI to use the new namespace
        // Note: The current task keeps its old PID in the parent namespace,
        // but new tasks spawned after this will use the new namespace
        *abi = LinuxRiscv64Abi {
            namespace: new_namespace,
            ..abi.clone()
        };
    }

    // Handle mount namespace isolation
    if (flags & CLONE_NEWNS) != 0 {
        // Create a new VFS instance for mount namespace isolation
        let new_vfs = crate::fs::VfsManager::new();
        task.vfs = Some(Arc::new(new_vfs));
        
        // Setup basic filesystem structure in the new mount namespace
        if let Some(vfs) = &task.vfs {
            // Create basic directories
            let _ = vfs.create_dir("/dev");
            let _ = vfs.create_dir("/proc");
            let _ = vfs.create_dir("/sys");
            let _ = vfs.create_dir("/tmp");
        }
    }

    // Other namespace flags are handled as stubs (no-op but accepted)
    // CLONE_NEWUTS, CLONE_NEWIPC, CLONE_NEWUSER, CLONE_NEWNET, CLONE_NEWCGROUP

    // Success
    0
}

/// sys_setns - Architecture-specific wrapper for setns syscall
///
/// This wrapper calls the generic stub implementation.
pub fn sys_setns(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    crate::abi::linux::unshare::sys_setns(abi, trapframe)
}
