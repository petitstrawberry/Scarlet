// Architecture-aware environment constants.
//
// This module is intentionally split by arch, but re-exported here so callers can keep using
// `crate::environment::FOO` without being aware of the split.

#[path = "environment/common.rs"]
mod common;

#[cfg(target_arch = "aarch64")]
#[path = "environment/aarch64.rs"]
mod arch;

#[cfg(target_arch = "riscv64")]
#[path = "environment/riscv64.rs"]
mod arch;

#[cfg(target_arch = "x86_64")]
#[path = "environment/x86_64.rs"]
mod arch;

pub use arch::*;
pub use common::*;

// Derived constants (depend on arch-specific end addresses)
pub const KERNEL_VM_STACK_START: usize = KERNEL_VM_STACK_END - KERNEL_VM_STACK_SIZE + 1;

// Kernel high-VA stack window region (per-task windows in shared kernel PT)
// One guard page + task kernel stack per slot
pub const KERNEL_KSTACK_SLOT_SIZE: usize = TASK_KERNEL_STACK_SIZE + PAGE_SIZE;
// Reserve the top-most page(s) for trampoline; place window region below KERNEL_VM_STACK
pub const KERNEL_KSTACK_REGION_END: usize = KERNEL_VM_STACK_START - 1;
pub const KERNEL_KSTACK_REGION_START: usize =
    KERNEL_KSTACK_REGION_END + 1 - (KERNEL_KSTACK_SLOTS * KERNEL_KSTACK_SLOT_SIZE);
