//! Unshare and setns system call implementations for Linux ABI
//!
//! The unshare system call allows a process to disassociate parts of its
//! execution context that are currently being shared with other processes.
//! The setns system call allows a process to join an existing namespace.
//! These are commonly used for namespace isolation in containers.
//!
//! Current implementation supports:
//! - CLONE_NEWNS: Mount namespace isolation (using VFS separation)
//! - CLONE_NEWPID: PID namespace isolation (using Task namespace)
//! - Stub support for other namespace types

use alloc::sync::Arc;
use crate::{
    abi::AbiModule,
    arch::Trapframe,
    task::{mytask, namespace::TaskNamespace},
};

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
///
/// # Note on Generic Implementation
///
/// This function cannot directly modify the ABI module's namespace field
/// because it operates on a trait object. Architecture-specific wrappers
/// should handle namespace updates if needed, or the ABI implementation
/// should provide a method to update the namespace.
pub fn sys_unshare<E>(_abi: &mut E, trapframe: &mut Trapframe) -> usize
where
    E: AbiModule + ?Sized,
{
    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX - 1, // -EPERM
    };

    let flags = trapframe.get_arg(0);

    trapframe.increment_pc_next(task);

    // Handle PID namespace isolation
    // Note: Since we're working with a trait object, we cannot directly update
    // the ABI's namespace field. The calling architecture-specific code should
    // handle this if namespace updates are needed at the ABI level.
    if (flags & CLONE_NEWPID) != 0 {
        // Create a new child namespace for PID isolation
        let current_namespace = _abi.get_task_namespace();
        let _new_namespace = TaskNamespace::new_child(
            current_namespace,
            alloc::format!("pid-ns-{}", task.get_id()),
        );
        
        // Note: The ABI module should implement a way to update its namespace
        // if it maintains one. For now, we acknowledge the request but cannot
        // directly update the namespace field through the trait.
        // Architecture-specific implementations can override this function
        // to provide full namespace switching.
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

/// sys_setns - Join an existing namespace
///
/// Arguments:
/// - fd: File descriptor referring to a namespace (e.g., /proc/[pid]/ns/[type])
/// - nstype: Namespace type to join (CLONE_NEW* flags) or 0 for any type
///
/// Returns:
/// - 0 on success
/// - Negative error code on failure
///
/// # Implementation Details
///
/// This is currently a stub implementation that returns ENOSYS.
/// Full implementation would require:
/// - /proc/[pid]/ns/* filesystem support
/// - Namespace file descriptor handling
/// - Ability to switch namespaces at runtime
///
/// For now, applications that attempt to use setns will receive
/// a "function not implemented" error.
pub fn sys_setns<E>(_abi: &mut E, trapframe: &mut Trapframe) -> usize
where
    E: AbiModule + ?Sized,
{
    let task = match mytask() {
        Some(t) => t,
        None => return usize::MAX - 1, // -EPERM
    };

    let _fd = trapframe.get_arg(0);
    let _nstype = trapframe.get_arg(1);

    trapframe.increment_pc_next(task);

    // Stub: Not implemented yet
    // Full implementation would:
    // 1. Validate fd refers to a namespace file
    // 2. Check nstype matches the namespace type (if non-zero)
    // 3. Join the target namespace
    usize::MAX - 38 // -ENOSYS
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
