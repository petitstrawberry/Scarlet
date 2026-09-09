// Native 32-bit virtual-address layout. Physical addresses are independent.
// User addresses occupy the lower 2 GiB; kernel image and allocator/MMIO
// windows occupy disjoint regions in the upper half.
pub const SCARLET_HHDM_BASE: usize = 0xc000_0000;
pub const KERNEL_DIRECT_MAP_SIZE: usize = 768 * 1024 * 1024;
pub const KERNEL_HEAP_BASE: usize = 0x9000_0000;
pub const KERNEL_HEAP_SIZE: usize = 128 * 1024 * 1024;
pub const IOREMAP_START: usize = 0xa000_0000;
pub const IOREMAP_END: usize = 0xafff_ffff;
pub const DEFAULT_USER_MMAP_BASE: usize = 0x4000_0000;
pub const USER_LOWER_CANONICAL_END: usize = 0x8000_0000;
