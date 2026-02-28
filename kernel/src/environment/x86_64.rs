// x86_64 environment constants

use super::common::PAGE_SIZE;

// Virtual memory maximum address (inclusive)
// x86_64 48-bit canonical: upper canonical end.
pub const VMMAX: usize = 0xffff_ffff_ffff_ffff;

// Trampoline-managed high-VA infrastructure anchor.
pub const TRAMPOLINE_VA_END: usize = VMMAX;

// Reserve the top page for trampoline
pub const TRAMPOLINE_VA_RESERVE: usize = PAGE_SIZE;

// User stack end address (exclusive)
pub const USER_STACK_END: usize =
    (TRAMPOLINE_VA_END - TRAMPOLINE_VA_RESERVE + 1) & !(PAGE_SIZE - 1);

// Kernel VM stack end address (inclusive)
pub const KERNEL_VM_STACK_END: usize = USER_STACK_END - 1;
