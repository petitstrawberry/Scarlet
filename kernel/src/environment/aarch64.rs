// AArch64 environment constants

// Virtual memory maximum address
// AArch64: 48-bit user space limit (TTBR0)
pub const VMMAX: usize = 0x0000_ffff_ffff_ffff;

// User stack end address (top of 48-bit user space)
pub const USER_STACK_END: usize = 0x0000_ffff_ffff_f000;

// Kernel VM stack addresses (within 48-bit address space)
pub const KERNEL_VM_STACK_END: usize = 0x0000_ffff_ffff_efff;
