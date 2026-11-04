// Architecture-aware environment constants.
//
// This module is intentionally split by arch, but re-exported here so callers can keep using
// `crate::environment::FOO` without being aware of the split.

#[path = "environment/common.rs"]
mod common;

#[cfg(target_arch = "aarch64")]
#[path = "environment/aarch64.rs"]
mod arch;

#[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))]
#[path = "environment/riscv.rs"]
mod arch;

#[cfg(target_pointer_width = "64")]
#[path = "environment/layout64.rs"]
mod layout;
#[cfg(target_pointer_width = "32")]
#[path = "environment/layout32.rs"]
mod layout;
pub use layout::*;

pub use arch::*;
pub use common::*;

// Derived constants (depend on arch-specific end addresses)
pub const KERNEL_VM_STACK_START: usize = KERNEL_VM_STACK_END - KERNEL_VM_STACK_SIZE + 1;

// Kernel high-VA stack window region (per-task windows in shared kernel PT)
// One guard page + task kernel stack per slot
pub const KERNEL_KSTACK_SLOT_SIZE: usize = TASK_KERNEL_STACK_SIZE + PAGE_SIZE;
// Reserve the top-most page(s) for trampoline; place window region below KERNEL_VM_STACK
pub const KERNEL_KSTACK_REGION_END: usize = KERNEL_VM_STACK_START - 1;
// Reserve only virtual addresses here. Physical stack pages and allocator
// metadata are committed on demand under the runtime task-capacity policy.
#[cfg(target_pointer_width = "64")]
const KERNEL_KSTACK_FLOOR: usize = KERNEL_MODULE_BASE + KERNEL_MODULE_SIZE;
#[cfg(target_pointer_width = "32")]
const KERNEL_KSTACK_FLOOR: usize = SCARLET_HHDM_BASE + KERNEL_DIRECT_MAP_SIZE;
pub const KERNEL_KSTACK_SLOTS: usize =
    (KERNEL_VM_STACK_START - KERNEL_KSTACK_FLOOR) / KERNEL_KSTACK_SLOT_SIZE;
pub const KERNEL_KSTACK_REGION_START: usize =
    KERNEL_KSTACK_REGION_END + 1 - (KERNEL_KSTACK_SLOTS * KERNEL_KSTACK_SLOT_SIZE);
const _: () = {
    assert!(KERNEL_KSTACK_SLOTS > 1);
    assert!(KERNEL_KSTACK_REGION_START >= KERNEL_KSTACK_FLOOR);
    assert!(KERNEL_KSTACK_REGION_END < KERNEL_VM_STACK_START);
};
