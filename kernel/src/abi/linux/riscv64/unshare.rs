//! Unshare system call implementation for Linux ABI
//!
//! The unshare system call allows a process to disassociate parts of its
//! execution context that are currently being shared with other processes.
//! This is commonly used for namespace isolation in containers.
//!
//! Current implementation supports:
//! - CLONE_NEWNS: Mount namespace isolation (using VFS separation)
//! - CLONE_NEWPID: PID namespace isolation (using Task namespace)
//! - Stub support for other namespace types

use alloc::sync::Arc;
use crate::{
    abi::{AbiModule, linux::riscv64::LinuxRiscv64Abi},
    arch::Trapframe,
    task::{mytask, namespace::TaskNamespace},
};

use super::errno;

/// Unshare flags (from Linux clone flags)
pub const CLONE_NEWNS: usize = 0x00020000;      // Mount namespace
pub const CLONE_NEWUTS: usize = 0x04000000;     // UTS (hostname) namespace
pub const CLONE_NEWIPC: usize = 0x08000000;     // IPC namespace
pub const CLONE_NEWUSER: usize = 0x10000000;    // User namespace
pub const CLONE_NEWPID: usize = 0x20000000;     // PID namespace
pub const CLONE_NEWNET: usize = 0x40000000;     // Network namespace
pub const CLONE_NEWCGROUP: usize = 0x02000000;  // Cgroup namespace

/// sys_unshare - Disassociate parts of the process execution context
///
/// Arguments:
/// - flags: Flags indicating which resources to unshare (CLONE_NEW* flags)
///
/// Returns:
/// - 0 on success
/// - Negative error code on failure
///
/// # Implementation Details
///
/// Currently implements:
/// - CLONE_NEWPID: Creates a new task namespace for PID isolation
/// - CLONE_NEWNS: Creates a new VFS for mount namespace isolation
/// - Other flags: Accepted but stubbed (return success without action)
///
/// Future tasks can be spawned in the new namespace, but the calling
/// task itself continues to use its original IDs in the parent namespace.
pub fn sys_unshare(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(t) => t,
        None => return errno::to_result(errno::EPERM),
    };

    let flags = trapframe.get_arg(0);

    trapframe.increment_pc_next(task);

    // Handle PID namespace isolation
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
        // This uses the existing VFS separation feature
        let new_vfs = crate::fs::VfsManager::new();
        task.vfs = Some(Arc::new(new_vfs));
        
        // Setup basic filesystem structure in the new mount namespace
        // This is a minimal setup; more complete initialization may be needed
        if let Some(vfs) = &task.vfs {
            // Create basic directories
            let _ = vfs.create_dir("/dev");
            let _ = vfs.create_dir("/proc");
            let _ = vfs.create_dir("/sys");
            let _ = vfs.create_dir("/tmp");
        }
    }

    // Handle UTS namespace (hostname/domainname) - stub
    if (flags & CLONE_NEWUTS) != 0 {
        // Stub: Accept but don't implement
        // In a full implementation, this would create a new UTS namespace
        // allowing independent hostname/domainname settings
    }

    // Handle IPC namespace - stub
    if (flags & CLONE_NEWIPC) != 0 {
        // Stub: Accept but don't implement
        // In a full implementation, this would create a new IPC namespace
        // for System V IPC objects
    }

    // Handle user namespace - stub
    if (flags & CLONE_NEWUSER) != 0 {
        // Stub: Accept but don't implement
        // In a full implementation, this would create a new user namespace
        // for UID/GID isolation
    }

    // Handle network namespace - stub
    if (flags & CLONE_NEWNET) != 0 {
        // Stub: Accept but don't implement
        // In a full implementation, this would create a new network namespace
        // for network isolation
    }

    // Handle cgroup namespace - stub
    if (flags & CLONE_NEWCGROUP) != 0 {
        // Stub: Accept but don't implement
        // In a full implementation, this would create a new cgroup namespace
    }

    // Success
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_unshare_flags_defined() {
        // Verify flag values match Linux ABI
        assert_eq!(CLONE_NEWNS, 0x00020000);
        assert_eq!(CLONE_NEWUTS, 0x04000000);
        assert_eq!(CLONE_NEWIPC, 0x08000000);
        assert_eq!(CLONE_NEWUSER, 0x10000000);
        assert_eq!(CLONE_NEWPID, 0x20000000);
        assert_eq!(CLONE_NEWNET, 0x40000000);
        assert_eq!(CLONE_NEWCGROUP, 0x02000000);
    }
}
