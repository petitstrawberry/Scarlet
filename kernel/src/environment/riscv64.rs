// RISC-V environment constants

use super::common::PAGE_SIZE;

pub const RISCV_STIMER_FREQ: u64 = 10000000; // 10MHz

// Virtual memory maximum address (inclusive)
// RISC-V SV48: upper canonical end.
pub const VMMAX: usize = 0xffff_ffff_ffff_ffff;

// Trampoline-managed high-VA infrastructure anchor.
//
// We treat the upper-most high-VA region as "trampoline-managed" infrastructure space:
// - the trampoline mapping itself
// - the kernel VM stack
// - per-task kernel stack windows (kstack slots) mapped into the shared kernel PT
pub const TRAMPOLINE_VA_END: usize = VMMAX;

// Keep the existing RISC-V layout: user stack ends at the page right before the
// last (top-most) page used by the trampoline.
pub const TRAMPOLINE_VA_RESERVE: usize = PAGE_SIZE;

// User stack end address (exclusive)
// NOTE: avoid `TRAMPOLINE_VA_END + 1` because TRAMPOLINE_VA_END may be `usize::MAX`.
pub const USER_STACK_END: usize =
	(TRAMPOLINE_VA_END - TRAMPOLINE_VA_RESERVE + 1) & !(PAGE_SIZE - 1);

// Kernel VM stack end address (inclusive)
pub const KERNEL_VM_STACK_END: usize = USER_STACK_END - 1;
