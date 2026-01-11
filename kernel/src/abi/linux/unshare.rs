//! Unshare and setns system call implementations for Linux ABI
//!
//! The unshare system call allows a process to disassociate parts of its
//! execution context that are currently being shared with other processes.
//! The setns system call allows a process to join an existing namespace.
//! These are commonly used for namespace isolation in containers.
//!
//! Current implementation supports:
//! - CLONE_NEWNS: Mount namespace isolation (using VFS separation)
//! - CLONE_NEWPID: PID namespace isolation (using Task namespace) - Note: requires architecture-specific support
//! - Stub support for other namespace types

use alloc::sync::Arc;
use crate::{
    arch::Trapframe,
    task::mytask,
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
/// - CLONE_NEWNS: Creates a new VFS for mount namespace isolation
/// - Other flags: Accepted but stubbed (return success without action)
///
/// Note: CLONE_NEWPID namespace isolation requires architecture-specific handling
/// because it needs to update the ABI module's namespace field. Architecture-specific
/// syscall handlers should implement this if needed.
pub fn sys_unshare(_abi: &mut dyn crate::abi::AbiModule, trapframe: &mut Trapframe) -> usize {
    use crate::abi::linux::riscv64::errno::{EPERM, to_result};
    
    let task = match mytask() {
        Some(t) => t,
        None => return to_result(EPERM),
    };

    let flags = trapframe.get_arg(0);

    trapframe.increment_pc_next(task);

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

    // Note: CLONE_NEWPID and other namespace types are handled as stubs
    // Architecture-specific implementations can provide full support by
    // implementing their own sys_unshare that calls this function and
    // additionally handles namespace updates

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
pub fn sys_setns(_abi: &mut dyn crate::abi::AbiModule, trapframe: &mut Trapframe) -> usize {
    use crate::abi::linux::riscv64::errno::{EPERM, ENOSYS, to_result};
    
    let task = match mytask() {
        Some(t) => t,
        None => return to_result(EPERM),
    };

    let _fd = trapframe.get_arg(0);
    let _nstype = trapframe.get_arg(1);

    trapframe.increment_pc_next(task);

    // Stub: Not implemented yet
    // Full implementation would:
    // 1. Validate fd refers to a namespace file
    // 2. Check nstype matches the namespace type (if non-zero)
    // 3. Join the target namespace
    to_result(ENOSYS)
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
