pub const NUM_OF_CPUS: usize = 2;
pub const RISCV_STIMER_FREQ: u64 = 10000000; // 10MHz
pub const VMMAX: usize = 0xffffffffffffffff;
pub const STACK_SIZE: usize = 0x10000; // 64KiB
pub const USER_STACK_END: usize = 0xffff_ffff_ffff_f000;
pub const PAGE_SIZE: usize = 0x1000; // 4KB
pub const KERNEL_VM_STACK_SIZE: usize = 0x10000; // 64KiB
pub const KERNEL_VM_STACK_END: usize = 0xffffffffffffefff;
pub const KERNEL_VM_STACK_START: usize = KERNEL_VM_STACK_END - KERNEL_VM_STACK_SIZE + 1;
pub const DEAFAULT_MAX_TASK_STACK_SIZE: usize = 0xffff_ffff_ffff_ffff; // Unlimited
pub const DEAFAULT_MAX_TASK_DATA_SIZE: usize = 0xffff_ffff_ffff_ffff; // Unlimited
pub const DEAFAULT_MAX_TASK_TEXT_SIZE: usize = 0xffff_ffff_ffff_ffff; // Unlimited
// Per-task kernel stack configuration
#[cfg(not(any(debug_assertions, test)))]
pub const TASK_KERNEL_STACK_SIZE: usize = 0x4000; // 16KiB per task
#[cfg(any(debug_assertions, test))]
pub const TASK_KERNEL_STACK_SIZE: usize = 0x8000; // 32KiB per task

// Kernel high-VA stack window region (per-task windows in shared kernel PT)
// One guard page + task kernel stack per slot
pub const KERNEL_KSTACK_SLOT_SIZE: usize = TASK_KERNEL_STACK_SIZE + PAGE_SIZE;
// Number of slots available for concurrent tasks
pub const KERNEL_KSTACK_SLOTS: usize = 256;
// Reserve the top-most page(s) for trampoline; place window region below KERNEL_VM_STACK
pub const KERNEL_KSTACK_REGION_END: usize = KERNEL_VM_STACK_START - 1;
pub const KERNEL_KSTACK_REGION_START: usize =
	KERNEL_KSTACK_REGION_END + 1 - (KERNEL_KSTACK_SLOTS * KERNEL_KSTACK_SLOT_SIZE);
