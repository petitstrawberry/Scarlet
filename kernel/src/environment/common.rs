pub const MAX_NUM_CPUS: usize = 2;

pub const STACK_SIZE: usize = 0x80000; // 128KiB
pub const PAGE_SIZE: usize = 0x1000; // 4KB

pub const KERNEL_VM_STACK_SIZE: usize = 0x10000; // 64KiB

pub const DEAFAULT_MAX_TASK_STACK_SIZE: usize = 0xffff_ffff_ffff_ffff; // Unlimited
pub const DEAFAULT_MAX_TASK_DATA_SIZE: usize = 0xffff_ffff_ffff_ffff; // Unlimited
pub const DEAFAULT_MAX_TASK_TEXT_SIZE: usize = 0xffff_ffff_ffff_ffff; // Unlimited

// Per-task kernel stack configuration
pub const TASK_KERNEL_STACK_SIZE: usize = 0x10000;

// Number of slots available for concurrent tasks
pub const KERNEL_KSTACK_SLOTS: usize = 256;
