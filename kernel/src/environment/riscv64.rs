// RISC-V environment constants

pub const RISCV_STIMER_FREQ: u64 = 10000000; // 10MHz

// Virtual memory maximum address
// RISC-V SV48: upper limit
pub const VMMAX: usize = 0xffff_ffff_ffff_ffff;

pub const USER_STACK_END: usize = 0xffff_ffff_ffff_f000;

pub const KERNEL_VM_STACK_END: usize = 0xffff_ffff_ffff_efff;
