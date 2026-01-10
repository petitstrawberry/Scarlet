//! RISC-V64 specific wrappers for cgroups functionality
//!
//! This module re-exports the generic Linux cgroup implementation
//! and provides architecture-specific wrappers where needed.

// Re-export the generic cgroup implementation
pub use crate::abi::linux::cgroup::*;

use crate::{
    abi::linux::riscv64::LinuxRiscv64Abi,
    arch::Trapframe,
};

/// Architecture-specific wrapper for sys_cgroup_ops
///
/// This wrapper calls the generic implementation with the concrete ABI type.
#[allow(dead_code)]
pub fn sys_cgroup_ops(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    crate::abi::linux::cgroup::sys_cgroup_ops(abi, trapframe)
}
