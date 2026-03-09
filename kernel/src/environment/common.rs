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

// IOREMAP virtual address region for dynamic device MMIO mapping.
//
// Located in the gap between HHDM end (0xFFFF_BFFF_FFFF_FFFF) and the kernel
// image (0xFFFF_FFFF_8000_0000), providing 1 GiB of virtual address space for
// on-demand device memory mappings (Linux-style ioremap).
pub const IOREMAP_START: usize = 0xFFFF_C000_0000_0000;
pub const IOREMAP_END: usize = 0xFFFF_C000_3FFF_FFFF; // 1 GiB
