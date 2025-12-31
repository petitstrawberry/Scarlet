// AArch64 environment constants

use super::common::PAGE_SIZE;

// Virtual memory maximum address (inclusive)
// AArch64: 48-bit VA (T0SZ/T1SZ=16) requires canonical addresses.
// The lower canonical range is 0x0000_0000_0000_0000 ..= 0x0000_7fff_ffff_ffff.
pub const VMMAX: usize = 0x0000_7fff_ffff_ffff;

// Upper canonical end address (inclusive) for 48-bit VA.
// We place trampoline / kernel high-VA regions here for TTBR1.
pub const TRAMPOLINE_VA_END: usize = 0xffff_ffff_ffff_ffff;

// Reserve a high-VA window for the trampoline.
// The actual trampoline size is link-time defined, but we need a static gap so
// user/kernel stacks don't collide with the `VMMAX - trampoline_size ..= VMMAX` mapping.
pub const TRAMPOLINE_VA_RESERVE: usize = 0x0001_0000; // 64KiB

const VMMAX_EXCLUSIVE: usize = VMMAX + 1;

// User stack end address (exclusive)
pub const USER_STACK_END: usize = (VMMAX_EXCLUSIVE - TRAMPOLINE_VA_RESERVE) & !(PAGE_SIZE - 1);

// Kernel VM stack addresses (within 48-bit address space)
// Keep kernel stack window below the trampoline-reserved region at the top of the
// upper canonical address space so it can live in TTBR1.
pub const KERNEL_VM_STACK_END: usize = (((TRAMPOLINE_VA_END - TRAMPOLINE_VA_RESERVE)
	& !(PAGE_SIZE - 1))
	- 1);
