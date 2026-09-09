pub const MAX_NUM_CPUS: usize = 16;

pub const STACK_SIZE: usize = 0x80000; // 512KiB
pub const PAGE_SIZE: usize = 0x1000; // 4KB

pub const KERNEL_VM_STACK_SIZE: usize = 0x10000; // 64KiB

pub const DEFAULT_TIME_SLICE: u32 = 1;

pub const DEAFAULT_MAX_TASK_STACK_SIZE: usize = usize::MAX; // Unlimited
pub const DEAFAULT_MAX_TASK_DATA_SIZE: usize = usize::MAX; // Unlimited
pub const DEAFAULT_MAX_TASK_TEXT_SIZE: usize = usize::MAX; // Unlimited

// Per-task kernel stack configuration
pub const TASK_KERNEL_STACK_SIZE: usize = 0x10000;

// Number of slots available for concurrent tasks
pub const KERNEL_KSTACK_SLOTS: usize = 256;
