pub const MAX_NUM_CPUS: usize = 16;

pub const STACK_SIZE: usize = 0x80000; // 128KiB
pub const PAGE_SIZE: usize = 0x1000; // 4KB

pub const KERNEL_VM_STACK_SIZE: usize = 0x10000; // 64KiB

pub const DEFAULT_TIME_SLICE: u32 = 1;

pub const DEAFAULT_MAX_TASK_STACK_SIZE: usize = 0xffff_ffff_ffff_ffff; // Unlimited
pub const DEAFAULT_MAX_TASK_DATA_SIZE: usize = 0xffff_ffff_ffff_ffff; // Unlimited
pub const DEAFAULT_MAX_TASK_TEXT_SIZE: usize = 0xffff_ffff_ffff_ffff; // Unlimited

// Per-task kernel stack configuration
pub const TASK_KERNEL_STACK_SIZE: usize = 0x10000;

// Number of slots available for concurrent tasks
pub const KERNEL_KSTACK_SLOTS: usize = 256;

// Scarlet-owned HHDM base address.
//
// After boot, Scarlet builds its own page tables and direct-maps all physical
// memory starting at this fixed virtual address, fully decoupled from whatever
// offset the bootloader (Limine) originally chose.
//
// Layout (upper canonical half):
//   0xffff_8000_0000_0000  SCARLET_HHDM_BASE   (direct map)
//   0xffff_c000_0000_0000  IOREMAP              (1 GiB)
//   0xffff_d000_0000_0000  KERNEL_HEAP_BASE     (512 MiB)
//   0xffff_ffff_8000_0000  Kernel image         (linker-placed)
//   top of VA space        Trampoline / kstack slots
pub const SCARLET_HHDM_BASE: usize = 0xffff_8000_0000_0000;

// Kernel heap virtual address base.
//
// The heap is mapped at a fixed VA independent of the HHDM, so it survives
// the HHDM offset change during the boot page-table switch.
pub const KERNEL_HEAP_BASE: usize = 0xffff_d000_0000_0000;

// Initial kernel heap size (512 MiB).
pub const KERNEL_HEAP_SIZE: usize = 512 * 1024 * 1024;

// IOREMAP virtual address region for dynamic device MMIO mapping.
//
// Located in the gap between HHDM end (0xFFFF_BFFF_FFFF_FFFF) and the kernel
// image (0xFFFF_FFFF_8000_0000), providing 1 GiB of virtual address space for
// on-demand device memory mappings (Linux-style ioremap).
pub const IOREMAP_START: usize = 0xFFFF_C000_0000_0000;
pub const IOREMAP_END: usize = 0xFFFF_C000_3FFF_FFFF; // 1 GiB
