pub const NUM_OF_CPUS: usize = 2;
pub const RISCV_STIMER_FREQ: u64 = 10000000; // 10MHz
pub const VMMAX: usize = 0xffffffffffffffff;
pub const STACK_SIZE: usize = 0x100000; // 512KB
pub const PAGE_SIZE: usize = 0x1000; // 4KB
pub const KERNEL_VM_STACK_SIZE: usize = 0x10000; // 64KiB
pub const KERNEL_VM_STACK_END: usize = 0xffffffffffffefff;
pub const KERNEL_VM_STACK_START: usize = KERNEL_VM_STACK_END - KERNEL_VM_STACK_SIZE + 1;
pub const DEAFAULT_MAX_TASK_STACK_SIZE: usize = 0xffff_ffff_ffff_ffff; // Unlimited
pub const DEAFAULT_MAX_TASK_DATA_SIZE: usize = 0xffff_ffff_ffff_ffff; // Unlimited
pub const DEAFAULT_MAX_TASK_TEXT_SIZE: usize = 0xffff_ffff_ffff_ffff; // Unlimited

