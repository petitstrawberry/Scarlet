pub const NUM_OF_CPUS: usize = 2;
pub const RISCV_STIMER_FREQ: u64 = 10000000; // 10MHz

// Virtual memory maximum address (architecture-specific)
// RISC-V SV48: 48-bit sign-extended addresses (upper half: 0xffff_8000_0000_0000 - 0xffff_ffff_ffff_ffff)
// AArch64: 48-bit addresses with TTBR1 (upper half: 0xffff_0000_0000_0000 - 0xffff_ffff_ffff_ffff)
#[cfg(target_arch = "riscv64")]
pub const VMMAX: usize = 0xffff_ffff_ffff_ffff; // SV48 upper limit
#[cfg(target_arch = "aarch64")]
pub const VMMAX: usize = 0x0000_ffff_ffff_ffff; // 48-bit user space limit (TTBR0)

pub const STACK_SIZE: usize = 0x80000; // 128KiB

// User stack end address (architecture-specific for 48-bit VA)
#[cfg(target_arch = "riscv64")]
pub const USER_STACK_END: usize = 0xffff_ffff_ffff_f000;
#[cfg(target_arch = "aarch64")]
pub const USER_STACK_END: usize = 0x0000_ffff_ffff_f000; // Top of 48-bit user space

pub const PAGE_SIZE: usize = 0x1000; // 4KB
pub const KERNEL_VM_STACK_SIZE: usize = 0x10000; // 64KiB

// Kernel VM stack addresses (architecture-specific)
#[cfg(target_arch = "riscv64")]
pub const KERNEL_VM_STACK_END: usize = 0xffffffffffffefff;
#[cfg(target_arch = "aarch64")]
pub const KERNEL_VM_STACK_END: usize = 0x0000_ffff_ffff_efff;
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
