//! Address translation utilities backed by the kernel memory layout.

use core::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

use spin::Once;

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KernelMemoryPhase {
    Uninitialized = 0,
    Bootloader = 1,
    BootKernel = 2,
    Runtime = 3,
}

impl KernelMemoryPhase {
    fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Bootloader,
            2 => Self::BootKernel,
            3 => Self::Runtime,
            _ => Self::Uninitialized,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct KernelImageLayout {
    phys_base: usize,
    virt_base: usize,
    size: usize,
}

impl KernelImageLayout {
    #[inline(always)]
    fn virt_end(&self) -> usize {
        self.virt_base + self.size
    }

    #[inline(always)]
    fn phys_end(&self) -> usize {
        self.phys_base + self.size
    }

    #[inline(always)]
    fn contains_virt(&self, vaddr: usize) -> bool {
        vaddr >= self.virt_base && vaddr < self.virt_end()
    }

    #[inline(always)]
    fn contains_phys(&self, paddr: usize) -> bool {
        paddr >= self.phys_base && paddr < self.phys_end()
    }
}

#[derive(Clone, Copy, Debug)]
struct DirectMapLayout {
    offset: usize,
    phys_start: usize,
    phys_end: usize,
}

impl DirectMapLayout {
    #[inline(always)]
    fn contains_phys(&self, paddr: usize) -> bool {
        paddr >= self.phys_start && paddr <= self.phys_end
    }

    #[inline(always)]
    fn virt_start(&self) -> usize {
        self.offset + self.phys_start
    }

    #[inline(always)]
    fn virt_end(&self) -> usize {
        self.offset + self.phys_end
    }

    #[inline(always)]
    fn contains_virt(&self, vaddr: usize) -> bool {
        vaddr >= self.virt_start() && vaddr <= self.virt_end()
    }
}

#[derive(Clone, Copy, Debug)]
struct HeapLayout {
    phys_base: usize,
    virt_base: usize,
    size: usize,
}

impl HeapLayout {
    #[inline(always)]
    fn virt_end(&self) -> usize {
        self.virt_base + self.size
    }

    #[inline(always)]
    fn contains_virt(&self, vaddr: usize) -> bool {
        vaddr >= self.virt_base && vaddr < self.virt_end()
    }
}

struct KernelMemoryLayout {
    phase: AtomicU8,
    kernel_image: Once<KernelImageLayout>,
    boot_direct_map_offset: AtomicUsize,
    current_direct_map_offset: AtomicUsize,
    boot_direct_map_phys_start: AtomicUsize,
    boot_direct_map_phys_end: AtomicUsize,
    current_direct_map_phys_start: AtomicUsize,
    current_direct_map_phys_end: AtomicUsize,
    heap_phys_base: AtomicUsize,
    heap_virt_base: AtomicUsize,
    heap_size: AtomicUsize,
}

impl KernelMemoryLayout {
    const fn new() -> Self {
        Self {
            phase: AtomicU8::new(KernelMemoryPhase::Uninitialized as u8),
            kernel_image: Once::new(),
            boot_direct_map_offset: AtomicUsize::new(0),
            current_direct_map_offset: AtomicUsize::new(0),
            boot_direct_map_phys_start: AtomicUsize::new(0),
            boot_direct_map_phys_end: AtomicUsize::new(0),
            current_direct_map_phys_start: AtomicUsize::new(0),
            current_direct_map_phys_end: AtomicUsize::new(0),
            heap_phys_base: AtomicUsize::new(0),
            heap_virt_base: AtomicUsize::new(0),
            heap_size: AtomicUsize::new(0),
        }
    }

    #[inline(always)]
    fn phase(&self) -> KernelMemoryPhase {
        KernelMemoryPhase::from_u8(self.phase.load(Ordering::Acquire))
    }

    fn init_from_limine(
        &self,
        hhdm_offset: usize,
        kernel_phys_base: usize,
        kernel_virt_base: usize,
        kernel_image_size: usize,
    ) {
        self.kernel_image.call_once(|| KernelImageLayout {
            phys_base: kernel_phys_base,
            virt_base: kernel_virt_base,
            size: kernel_image_size,
        });
        self.boot_direct_map_offset
            .store(hhdm_offset, Ordering::Release);
        self.current_direct_map_offset
            .store(hhdm_offset, Ordering::Release);
        self.phase
            .store(KernelMemoryPhase::Bootloader as u8, Ordering::Release);
    }

    fn set_boot_direct_map_range(&self, phys_start: usize, phys_end: usize) {
        self.boot_direct_map_phys_start
            .store(phys_start, Ordering::Release);
        self.boot_direct_map_phys_end
            .store(phys_end, Ordering::Release);

        if self.phase() == KernelMemoryPhase::Bootloader {
            self.current_direct_map_phys_start
                .store(phys_start, Ordering::Release);
            self.current_direct_map_phys_end
                .store(phys_end, Ordering::Release);
        }
    }

    fn transition_to_boot_kernel(
        &self,
        direct_map_offset: usize,
        direct_map_phys_start: usize,
        direct_map_phys_end: usize,
        heap_phys_base: usize,
        heap_virt_base: usize,
        heap_size: usize,
    ) {
        self.current_direct_map_offset
            .store(direct_map_offset, Ordering::Release);
        self.current_direct_map_phys_start
            .store(direct_map_phys_start, Ordering::Release);
        self.current_direct_map_phys_end
            .store(direct_map_phys_end, Ordering::Release);
        self.heap_phys_base.store(heap_phys_base, Ordering::Release);
        self.heap_virt_base.store(heap_virt_base, Ordering::Release);
        self.heap_size.store(heap_size, Ordering::Release);
        self.phase
            .store(KernelMemoryPhase::BootKernel as u8, Ordering::Release);
    }

    fn finalize_runtime(&self) {
        self.phase
            .store(KernelMemoryPhase::Runtime as u8, Ordering::Release);
    }

    #[inline(always)]
    fn kernel_image(&self) -> &KernelImageLayout {
        self.kernel_image
            .get()
            .expect("kernel image layout not initialized")
    }

    #[inline(always)]
    fn boot_direct_map(&self) -> DirectMapLayout {
        let offset = self.boot_direct_map_offset.load(Ordering::Acquire);
        let phys_start = self.boot_direct_map_phys_start.load(Ordering::Acquire);
        let phys_end = self.boot_direct_map_phys_end.load(Ordering::Acquire);
        assert!(offset != 0, "boot direct-map offset not initialized");
        assert!(
            phys_end >= phys_start,
            "boot direct-map range not initialized"
        );
        DirectMapLayout {
            offset,
            phys_start,
            phys_end,
        }
    }

    #[inline(always)]
    fn current_direct_map(&self) -> DirectMapLayout {
        let offset = self.current_direct_map_offset.load(Ordering::Acquire);
        let phys_start = self.current_direct_map_phys_start.load(Ordering::Acquire);
        let phys_end = self.current_direct_map_phys_end.load(Ordering::Acquire);
        assert!(offset != 0, "current direct-map offset not initialized");
        assert!(
            phys_end >= phys_start,
            "current direct-map range not initialized"
        );
        DirectMapLayout {
            offset,
            phys_start,
            phys_end,
        }
    }

    #[inline(always)]
    fn heap_layout(&self) -> Option<HeapLayout> {
        let size = self.heap_size.load(Ordering::Acquire);
        if size == 0 {
            return None;
        }
        Some(HeapLayout {
            phys_base: self.heap_phys_base.load(Ordering::Acquire),
            virt_base: self.heap_virt_base.load(Ordering::Acquire),
            size,
        })
    }

    fn phys_to_current_virt(&self, paddr: usize) -> usize {
        let direct_map = self.current_direct_map();
        assert!(
            direct_map.contains_phys(paddr),
            "phys_to_virt: physical address {:#x} is outside current direct-map range {:#x}..={:#x}",
            paddr,
            direct_map.phys_start,
            direct_map.phys_end
        );
        paddr
            .checked_add(direct_map.offset)
            .unwrap_or_else(|| panic!("phys_to_virt overflow: paddr={:#x}", paddr))
    }

    fn virt_to_current_phys(&self, vaddr: usize) -> Option<usize> {
        let kernel_image = self.kernel_image();
        if kernel_image.contains_virt(vaddr) {
            return Some(kernel_image.phys_base + (vaddr - kernel_image.virt_base));
        }

        if let Some(heap) = self.heap_layout() {
            if heap.contains_virt(vaddr) {
                return Some(heap.phys_base + (vaddr - heap.virt_base));
            }
        }

        let direct_map = self.current_direct_map();
        if direct_map.contains_virt(vaddr) {
            return Some(vaddr - direct_map.offset);
        }

        None
    }

    fn virt_to_boot_phys(&self, vaddr: usize) -> Option<usize> {
        let kernel_image = self.kernel_image();
        if kernel_image.contains_virt(vaddr) {
            return Some(kernel_image.phys_base + (vaddr - kernel_image.virt_base));
        }

        let direct_map = self.boot_direct_map();
        if direct_map.contains_virt(vaddr) {
            return Some(vaddr - direct_map.offset);
        }

        None
    }

    fn phys_to_boot_virt(&self, paddr: usize) -> usize {
        let direct_map = self.boot_direct_map();
        assert!(
            direct_map.contains_phys(paddr),
            "boot_phys_to_virt: physical address {:#x} is outside boot direct-map range {:#x}..={:#x}",
            paddr,
            direct_map.phys_start,
            direct_map.phys_end
        );
        paddr
            .checked_add(direct_map.offset)
            .unwrap_or_else(|| panic!("boot_phys_to_virt overflow: paddr={:#x}", paddr))
    }
}

static KERNEL_MEMORY_LAYOUT: KernelMemoryLayout = KernelMemoryLayout::new();

#[inline(always)]
fn layout() -> &'static KernelMemoryLayout {
    &KERNEL_MEMORY_LAYOUT
}

/// Initialize address translation from Limine bootloader information.
///
/// This must be called early in the boot process (e.g., in `limine_entry`)
/// before any address translation is performed. It records the HHDM offset
/// and kernel image layout provided by the bootloader.
///
/// # Arguments
///
/// * `hhdm_offset` - The HHDM (Higher Half Direct Map) offset from physical addresses
/// * `kernel_phys_base` - Physical base address of the kernel image
/// * `kernel_virt_base` - Virtual base address of the kernel image
/// * `kernel_image_size` - Size of the kernel image in bytes
pub fn init_limine_addressing(
    hhdm_offset: usize,
    kernel_phys_base: usize,
    kernel_virt_base: usize,
    kernel_image_size: usize,
) {
    layout().init_from_limine(
        hhdm_offset,
        kernel_phys_base,
        kernel_virt_base,
        kernel_image_size,
    );
}

/// Check if address translation is initialized and ready to use.
///
/// Returns `true` after `init_limine_addressing()` has been called,
/// indicating that address translation functions can be safely used.
#[inline(always)]
pub fn address_translation_ready() -> bool {
    layout().phase() != KernelMemoryPhase::Uninitialized
}

/// Set the boot-time direct-map physical address range.
///
/// This defines the physical memory range that is directly mapped
/// by the bootloader (via HHDM). Should be called during early boot
/// after `init_limine_addressing()`.
///
/// # Arguments
///
/// * `start` - Start of the direct-mapped physical address range
/// * `end` - End of the direct-mapped physical address range (inclusive)
pub fn init_boot_direct_map_range(start: usize, end: usize) {
    layout().set_boot_direct_map_range(start, end);
}

/// Transition to the kernel-owned memory layout.
///
/// This should be called after switching from the bootloader's page tables
/// to Scarlet's own page tables. It updates the direct-map and heap
/// layout information for runtime address translation.
///
/// # Arguments
///
/// * `direct_map_offset` - HHDM offset for the new page tables
/// * `direct_map_phys_start` - Start of direct-mapped physical range
/// * `direct_map_phys_end` - End of direct-mapped physical range (inclusive)
/// * `heap_phys_base` - Physical base address of the kernel heap
/// * `heap_virt_base` - Virtual base address of the kernel heap
/// * `heap_size` - Size of the kernel heap in bytes
pub fn transition_kernel_memory_layout(
    direct_map_offset: usize,
    direct_map_phys_start: usize,
    direct_map_phys_end: usize,
    heap_phys_base: usize,
    heap_virt_base: usize,
    heap_size: usize,
) {
    layout().transition_to_boot_kernel(
        direct_map_offset,
        direct_map_phys_start,
        direct_map_phys_end,
        heap_phys_base,
        heap_virt_base,
        heap_size,
    );
}

/// Finalize the memory layout for full runtime operation.
///
/// Marks the memory layout as fully initialized. After this call,
/// the system is in the Runtime phase and all address translation
/// functions operate in their final configuration.
pub fn finalize_runtime_memory_layout() {
    layout().finalize_runtime();
}

/// Get the current direct-map physical address range.
///
/// Returns the physical memory range that is currently direct-mapped
/// (accessible via HHDM offset).
///
/// # Returns
///
/// A tuple of `(start, end)` physical addresses (inclusive).
pub fn get_current_direct_map_phys_range() -> (usize, usize) {
    let current = layout().current_direct_map();
    (current.phys_start, current.phys_end)
}

/// Get the kernel heap physical layout information.
///
/// Returns the heap layout if it has been initialized.
///
/// # Returns
///
/// `Some((phys_base, virt_base, size))` if heap is initialized,
/// `None` otherwise.
pub fn get_heap_phys_layout() -> Option<(usize, usize, usize)> {
    layout()
        .heap_layout()
        .map(|heap| (heap.phys_base, heap.virt_base, heap.size))
}

/// Set the HHDM (Higher Half Direct Map) offset for address translation.
///
/// This updates the offset used for converting between physical and virtual
/// addresses in the direct-mapped region.
///
/// # Arguments
///
/// * `new_offset` - The new HHDM offset value
pub fn set_hhdm_offset(new_offset: usize) {
    let current = layout().current_direct_map();
    transition_kernel_memory_layout(
        new_offset,
        current.phys_start,
        current.phys_end,
        layout()
            .heap_layout()
            .map(|heap| heap.phys_base)
            .unwrap_or(0),
        layout()
            .heap_layout()
            .map(|heap| heap.virt_base)
            .unwrap_or(0),
        layout().heap_layout().map(|heap| heap.size).unwrap_or(0),
    );
}

/// Get the current HHDM (Higher Half Direct Map) offset.
///
/// Returns the offset used for the current runtime address translation.
#[inline(always)]
pub fn get_hhdm_offset() -> usize {
    layout().current_direct_map().offset
}

/// Get the boot-time HHDM (Higher Half Direct Map) offset.
///
/// Returns the offset provided by the bootloader during early boot.
/// This is used for boot-phase address translation.
#[inline(always)]
pub fn get_boot_hhdm_offset() -> usize {
    layout().boot_direct_map().offset
}

/// Convert a boot-time physical address to virtual address.
///
/// Uses the bootloader-provided HHDM offset. This should only be called
/// during early boot before transitioning to kernel-owned page tables.
///
/// # Panics
///
/// Panics if the physical address is outside the boot direct-map range.
#[inline(always)]
pub fn boot_phys_to_virt(paddr: usize) -> usize {
    layout().phys_to_boot_virt(paddr)
}

/// Convert a virtual address to physical address (runtime).
///
/// Uses the current runtime memory layout. This should be called after
/// the kernel has transitioned to its own page tables.
///
/// # Panics
///
/// Panics with caller information if the virtual address cannot be mapped
/// to a physical address in the current layout.
#[inline(always)]
#[track_caller]
pub fn virt_to_phys(vaddr: usize) -> usize {
    layout().virt_to_current_phys(vaddr).unwrap_or_else(|| {
        let caller = core::panic::Location::caller();
        panic!(
            "virt_to_phys: unmapped kernel virtual address {:#x} (caller: {}:{})",
            vaddr,
            caller.file(),
            caller.line()
        )
    })
}

/// Convert a boot-time virtual address to physical address.
///
/// Uses the bootloader-provided memory layout. This should be called
/// during early boot for addresses provided by the bootloader.
///
/// # Panics
///
/// Panics with caller information if the virtual address cannot be mapped
/// to a physical address in the boot layout.
#[inline(always)]
#[track_caller]
pub fn boot_virt_to_phys(vaddr: usize) -> usize {
    layout().virt_to_boot_phys(vaddr).unwrap_or_else(|| {
        let caller = core::panic::Location::caller();
        panic!(
            "boot_virt_to_phys: unmapped boot virtual address {:#x} (caller: {}:{})",
            vaddr,
            caller.file(),
            caller.line()
        )
    })
}

/// Convert a physical address to virtual address (runtime).
///
/// Uses the current runtime memory layout (HHDM offset).
///
/// # Panics
///
/// Panics if the physical address is outside the current direct-map range.
#[inline(always)]
pub fn phys_to_virt(paddr: usize) -> usize {
    layout().phys_to_current_virt(paddr)
}

/// Convert a physical address to kernel virtual address.
///
/// This is an alias for `phys_to_virt()`.
#[inline(always)]
pub fn phys_to_kernel_virt(paddr: usize) -> usize {
    phys_to_virt(paddr)
}

/// Convert a kernel virtual address to physical address.
///
/// This is an alias for `virt_to_phys()`.
#[inline(always)]
pub fn kernel_virt_to_phys(vaddr: usize) -> usize {
    virt_to_phys(vaddr)
}

/// Convert a physical address to kernel image virtual address.
///
/// Only works for addresses within the kernel image (code/data sections).
/// For general direct-mapped addresses, use `phys_to_virt()` instead.
///
/// # Panics
///
/// Panics if the physical address is outside the kernel image range.
#[inline(always)]
pub fn phys_to_kernel_image_virt(paddr: usize) -> usize {
    let kernel_image = layout().kernel_image();
    if kernel_image.contains_phys(paddr) {
        return kernel_image.virt_base + (paddr - kernel_image.phys_base);
    }
    panic!(
        "phys_to_kernel_image_virt: physical address {:#x} is outside kernel image range",
        paddr
    )
}

/// Check if a virtual address is in the direct-mapped region.
///
/// Returns `true` if the address can be translated to physical
/// via simple HHDM offset subtraction.
#[inline(always)]
pub fn is_direct_mapped(vaddr: usize) -> bool {
    layout().current_direct_map().contains_virt(vaddr)
}

/// A wrapper type representing a physical address.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PhysAddr(pub usize);

impl PhysAddr {
    /// Create a new `PhysAddr` from a raw address value.
    #[inline(always)]
    pub const fn new(addr: usize) -> Self {
        Self(addr)
    }

    /// Return the raw address value.
    #[inline(always)]
    pub const fn as_usize(&self) -> usize {
        self.0
    }

    /// Convert this physical address to a virtual address.
    ///
    /// Uses `phys_to_virt()` for the translation.
    #[inline(always)]
    pub fn to_virt(&self) -> VirtAddr {
        VirtAddr::new(phys_to_virt(self.0))
    }

    /// Check if the address is aligned to the given power-of-two alignment.
    #[inline(always)]
    pub const fn is_aligned(&self, align: usize) -> bool {
        assert!(align != 0 && align.is_power_of_two());
        self.0 & (align - 1) == 0
    }

    /// Align the address down to the given power-of-two boundary.
    #[inline(always)]
    pub const fn align_down(&self, align: usize) -> Self {
        assert!(align != 0 && align.is_power_of_two());
        Self::new(self.0 & !(align - 1))
    }

    /// Align the address up to the given power-of-two boundary.
    #[inline(always)]
    pub const fn align_up(&self, align: usize) -> Self {
        assert!(align != 0 && align.is_power_of_two());
        Self::new((self.0 + align - 1) & !(align - 1))
    }
}

/// A wrapper type representing a virtual address.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VirtAddr(pub usize);

impl VirtAddr {
    /// Create a new `VirtAddr` from a raw address value.
    #[inline(always)]
    pub const fn new(addr: usize) -> Self {
        Self(addr)
    }

    /// Return the raw address value.
    #[inline(always)]
    pub const fn as_usize(&self) -> usize {
        self.0
    }

    /// Convert this virtual address to a physical address.
    ///
    /// Uses `virt_to_phys()` for the translation.
    #[inline(always)]
    pub fn to_phys(&self) -> PhysAddr {
        PhysAddr::new(virt_to_phys(self.0))
    }

    /// Check if the address is aligned to the given power-of-two alignment.
    #[inline(always)]
    pub const fn is_aligned(&self, align: usize) -> bool {
        assert!(align != 0 && align.is_power_of_two());
        self.0 & (align - 1) == 0
    }

    /// Align the address down to the given power-of-two boundary.
    #[inline(always)]
    pub const fn align_down(&self, align: usize) -> Self {
        assert!(align != 0 && align.is_power_of_two());
        Self::new(self.0 & !(align - 1))
    }

    /// Align the address up to the given power-of-two boundary.
    #[inline(always)]
    pub const fn align_up(&self, align: usize) -> Self {
        assert!(align != 0 && align.is_power_of_two());
        Self::new((self.0 + align - 1) & !(align - 1))
    }
}
