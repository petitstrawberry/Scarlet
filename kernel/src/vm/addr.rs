//! Address translation utilities for virtual memory management.
//!
//! This module provides functions for converting between physical and virtual addresses.
//!
//! # Design
//!
//! The HHDM (Higher Half Direct Map) offset is stored in an `AtomicUsize` so it can be
//! updated after the kernel switches from the bootloader's page tables to its own.
//! During early boot (before the switch), `phys_to_virt` uses the Limine-provided offset.
//! After switching to Scarlet's own page tables, `set_hhdm_offset()` is called to point
//! translations at `SCARLET_HHDM_BASE`.
//!
//! For pre-switch page table construction code that must NOT go through the runtime
//! `phys_to_virt` path, use `boot_phys_to_virt` which reads the boot-time offset
//! directly from the immutable `BOOT_HHDM_OFFSET`.
//!
//! # Usage
//!
//! ```rust,ignore
//! use crate::vm::addr::{virt_to_phys, phys_to_virt};
//!
//! // Example virtual address
//! let vaddr: usize = 0x1000;
//!
//! // Convert virtual address to physical address
//! let paddr = virt_to_phys(vaddr);
//!
//! // Convert physical address back to virtual address
//! let vaddr_back = phys_to_virt(paddr);
//! ```

use core::sync::atomic::{AtomicUsize, Ordering};

use spin::Once;

/// Kernel image layout — immutable once set at boot.
#[derive(Clone, Copy, Debug)]
struct KernelLayout {
    kernel_phys_base: usize,
    kernel_virt_base: usize,
    kernel_image_size: usize,
}

impl KernelLayout {
    #[inline(always)]
    fn kernel_virt_end(&self) -> usize {
        self.kernel_virt_base + self.kernel_image_size
    }

    #[inline(always)]
    fn kernel_phys_end(&self) -> usize {
        self.kernel_phys_base + self.kernel_image_size
    }

    #[inline(always)]
    fn contains_kernel_virt(&self, vaddr: usize) -> bool {
        vaddr >= self.kernel_virt_base && vaddr < self.kernel_virt_end()
    }
}

/// Kernel image layout (linker-placed addresses). Set once at boot, never changes.
static KERNEL_LAYOUT: Once<KernelLayout> = Once::new();

/// Runtime HHDM offset.  Initially set to the bootloader's value, then updated to
/// `SCARLET_HHDM_BASE` after the kernel switches to its own page tables.
///
/// 0 = not yet initialized (pre-init_limine_addressing).
static HHDM_OFFSET: AtomicUsize = AtomicUsize::new(0);

/// Boot-time HHDM offset — the value Limine gave us.  Stored once and never modified.
/// Used by `boot_phys_to_virt` during pre-switch page table construction so that code
/// does not depend on the mutable `HHDM_OFFSET`.
static BOOT_HHDM_OFFSET: Once<usize> = Once::new();

#[inline(always)]
fn kernel_layout() -> Option<&'static KernelLayout> {
    KERNEL_LAYOUT.get()
}

/// Initialize address translation with the Limine-provided layout.
///
/// Must be called exactly once during early boot, before any `phys_to_virt` calls.
pub fn init_limine_addressing(
    hhdm_offset: usize,
    kernel_phys_base: usize,
    kernel_virt_base: usize,
    kernel_image_size: usize,
) {
    KERNEL_LAYOUT.call_once(|| KernelLayout {
        kernel_phys_base,
        kernel_virt_base,
        kernel_image_size,
    });
    BOOT_HHDM_OFFSET.call_once(|| hhdm_offset);
    HHDM_OFFSET.store(hhdm_offset, Ordering::Release);
}

/// Returns `true` once `init_limine_addressing` has been called.
#[inline(always)]
pub fn address_translation_ready() -> bool {
    HHDM_OFFSET.load(Ordering::Relaxed) != 0 && kernel_layout().is_some()
}

/// Update the runtime HHDM offset.
///
/// Called exactly once after switching to Scarlet's own page tables, so that subsequent
/// `phys_to_virt` / `virt_to_phys` calls use the new direct-map base.
///
/// # Safety
///
/// The caller must ensure that the new page tables are active and the new offset is
/// correct before calling this.  All existing HHDM-derived pointers (PMM metadata,
/// etc.) must be fixed up before or after this call.
pub fn set_hhdm_offset(new_offset: usize) {
    HHDM_OFFSET.store(new_offset, Ordering::Release);
}

/// Returns the current runtime HHDM offset.
#[inline(always)]
pub fn get_hhdm_offset() -> usize {
    HHDM_OFFSET.load(Ordering::Acquire)
}

/// Returns the boot-time (Limine) HHDM offset.
///
/// This is the immutable value from the bootloader.  Use this in pre-switch code
/// (e.g., building Scarlet's own page tables) where you need the original mapping.
#[inline(always)]
pub fn get_boot_hhdm_offset() -> usize {
    *BOOT_HHDM_OFFSET
        .get()
        .expect("boot HHDM offset not initialized")
}

/// Convert physical address to virtual address using the **boot-time** HHDM offset.
///
/// This function is for use during pre-switch page table construction ONLY.
/// It always uses the Limine-provided offset regardless of whether `set_hhdm_offset`
/// has been called.
///
/// After the PT switch, use `phys_to_virt` instead.
#[inline(always)]
pub fn boot_phys_to_virt(paddr: usize) -> usize {
    let offset = get_boot_hhdm_offset();
    paddr.checked_add(offset).unwrap_or_else(|| {
        panic!(
            "boot_phys_to_virt overflow: paddr={:#x} boot_hhdm_offset={:#x}",
            paddr, offset
        )
    })
}

/// Converts a virtual address to a physical address.
#[inline(always)]
pub fn virt_to_phys(vaddr: usize) -> usize {
    // Kernel image region — fixed offset, never changes
    if let Some(kl) = kernel_layout() {
        if kl.contains_kernel_virt(vaddr) {
            return kl.kernel_phys_base + (vaddr - kl.kernel_virt_base);
        }
    }
    // HHDM region
    let offset = HHDM_OFFSET.load(Ordering::Acquire);
    if offset != 0 && vaddr >= offset {
        return vaddr - offset;
    }
    // Fallback: identity
    vaddr
}

/// Converts a physical address to a virtual address.
///
/// **Note:** This function is specifically intended for producing addresses in
/// the HHDM (Higher Half Direct Map) / direct-map region. It is **not** valid
/// for obtaining arbitrary kernel virtual addresses (e.g., kernel image VAs).
///
/// # Arguments
///
/// * `paddr` - Physical address to convert
///
/// # Returns
///
/// Direct-map virtual address corresponding to the given physical address
#[inline(always)]
pub fn phys_to_virt(paddr: usize) -> usize {
    let offset = HHDM_OFFSET.load(Ordering::Acquire);
    if offset != 0 {
        return paddr.checked_add(offset).unwrap_or_else(|| {
            panic!(
                "phys_to_virt overflow: paddr={:#x} hhdm_offset={:#x}",
                paddr, offset
            )
        });
    }
    // Not yet initialized — identity
    paddr
}

/// Converts a physical address to a virtual address for kernel use.
///
/// This is an alias for `phys_to_virt` but makes the intent clearer
/// when accessing kernel memory.
///
/// # Arguments
///
/// * `paddr` - Physical address to convert
///
/// # Returns
///
/// Virtual address that can be used to access the physical memory
#[inline(always)]
pub fn phys_to_kernel_virt(paddr: usize) -> usize {
    phys_to_virt(paddr)
}

/// Converts a virtual address used by kernel to physical address.
///
/// This is an alias for `virt_to_phys` but makes the intent clearer
/// when dealing with kernel virtual addresses.
///
/// # Arguments
///
/// * `vaddr` - Kernel virtual address to convert
///
/// # Returns
///
/// Physical address corresponding to the given kernel virtual address
#[inline(always)]
pub fn kernel_virt_to_phys(vaddr: usize) -> usize {
    virt_to_phys(vaddr)
}

#[inline(always)]
pub fn phys_to_kernel_image_virt(paddr: usize) -> usize {
    if let Some(kl) = kernel_layout() {
        if paddr >= kl.kernel_phys_base && paddr < kl.kernel_phys_end() {
            return kl.kernel_virt_base + (paddr - kl.kernel_phys_base);
        }
    }
    paddr
}

/// Checks if the given virtual address is in the direct mapping region.
///
/// # Arguments
///
/// * `vaddr` - Virtual address to check
///
/// # Returns
///
/// `true` if the address is in the direct mapping region
#[inline(always)]
pub fn is_direct_mapped(vaddr: usize) -> bool {
    let offset = HHDM_OFFSET.load(Ordering::Relaxed);
    if offset != 0 {
        return vaddr >= offset;
    }
    true
}

// ============================================================================
// Type-safe address wrappers (for future use)
// ============================================================================

/// A physical memory address.
///
/// This type provides type safety when working with physical addresses,
/// preventing accidental mixing with virtual addresses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PhysAddr(pub usize);

impl PhysAddr {
    /// Creates a new physical address.
    #[inline(always)]
    pub const fn new(addr: usize) -> Self {
        Self(addr)
    }

    /// Returns the raw address value.
    #[inline(always)]
    pub const fn as_usize(&self) -> usize {
        self.0
    }

    /// Converts this physical address to a virtual address.
    #[inline(always)]
    pub fn to_virt(&self) -> VirtAddr {
        VirtAddr::new(phys_to_virt(self.0))
    }

    /// Returns true if the address is aligned to the given boundary.
    ///
    /// # Panics
    ///
    /// Panics if `align` is 0 or not a power of two.
    #[inline(always)]
    pub const fn is_aligned(&self, align: usize) -> bool {
        assert!(align != 0 && align.is_power_of_two());
        self.0 & (align - 1) == 0
    }

    /// Returns the address aligned down to the given boundary.
    ///
    /// # Panics
    ///
    /// Panics if `align` is 0 or not a power of two.
    #[inline(always)]
    pub const fn align_down(&self, align: usize) -> Self {
        assert!(align != 0 && align.is_power_of_two());
        Self::new(self.0 & !(align - 1))
    }

    /// Returns the address aligned up to the given boundary.
    ///
    /// # Panics
    ///
    /// Panics if `align` is 0 or not a power of two.
    #[inline(always)]
    pub const fn align_up(&self, align: usize) -> Self {
        assert!(align != 0 && align.is_power_of_two());
        Self::new((self.0 + align - 1) & !(align - 1))
    }
}

/// A virtual memory address.
///
/// This type provides type safety when working with virtual addresses,
/// preventing accidental mixing with physical addresses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VirtAddr(pub usize);

impl VirtAddr {
    /// Creates a new virtual address.
    #[inline(always)]
    pub const fn new(addr: usize) -> Self {
        Self(addr)
    }

    /// Returns the raw address value.
    #[inline(always)]
    pub const fn as_usize(&self) -> usize {
        self.0
    }

    /// Converts this virtual address to a physical address.
    #[inline(always)]
    pub fn to_phys(&self) -> PhysAddr {
        PhysAddr::new(virt_to_phys(self.0))
    }

    /// Returns `true` if the address is aligned to the given boundary.
    ///
    /// # Parameters
    ///
    /// * `align` - Alignment in bytes. Must be non-zero and a power of two.
    ///
    /// If `align` is zero or not a power of two, this function:
    ///
    /// * Returns `false` in release builds.
    /// * Triggers a debug assertion in debug builds and returns `false`.
    #[inline(always)]
    pub const fn is_aligned(&self, align: usize) -> bool {
        if !Self::is_valid_align(align) {
            debug_assert!(
                false,
                "VirtAddr::is_aligned called with invalid alignment (must be non-zero power of two)",
            );
            return false;
        }
        (self.0 & (align - 1)) == 0
    }

    /// Returns the address aligned down to the given boundary.
    ///
    /// # Parameters
    ///
    /// * `align` - Alignment in bytes. Must be non-zero and a power of two.
    ///
    /// If `align` is zero or not a power of two, this function:
    ///
    /// * Returns the original address in release builds.
    /// * Triggers a debug assertion in debug builds and returns the original address.
    #[inline(always)]
    pub const fn align_down(&self, align: usize) -> Self {
        if !Self::is_valid_align(align) {
            debug_assert!(
                false,
                "VirtAddr::align_down called with invalid alignment (must be non-zero power of two)",
            );
            return *self;
        }
        Self::new(self.0 & !(align - 1))
    }

    /// Returns the address aligned up to the given boundary.
    ///
    /// # Parameters
    ///
    /// * `align` - Alignment in bytes. Must be non-zero and a power of two.
    ///
    /// If `align` is zero or not a power of two, this function:
    ///
    /// * Returns the original address in release builds.
    /// * Triggers a debug assertion in debug builds and returns the original address.
    #[inline(always)]
    pub const fn align_up(&self, align: usize) -> Self {
        if !Self::is_valid_align(align) {
            debug_assert!(
                false,
                "VirtAddr::align_up called with invalid alignment (must be non-zero power of two)",
            );
            return *self;
        }
        Self::new((self.0 + align - 1) & !(align - 1))
    }

    /// Returns `true` if an alignment value is valid for use with
    /// [`VirtAddr::is_aligned`], [`VirtAddr::align_down`], and [`VirtAddr::align_up`].
    ///
    /// A valid alignment is non-zero and a power of two.
    #[inline(always)]
    const fn is_valid_align(align: usize) -> bool {
        align != 0 && align.is_power_of_two()
    }
}

// ============================================================================
// Unit tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_identity_mapping_virt_to_phys() {
        let vaddr = phys_to_virt(0x80000000usize);
        let paddr = virt_to_phys(vaddr);
        assert_eq!(paddr, 0x80000000usize);
    }

    #[test_case]
    fn test_identity_mapping_phys_to_virt() {
        let paddr = 0x80000000usize;
        let vaddr = phys_to_virt(paddr);
        assert_eq!(virt_to_phys(vaddr), paddr);
    }

    #[test_case]
    fn test_roundtrip_conversion() {
        let original_paddr = 0x80001234usize;
        let vaddr = phys_to_virt(original_paddr);
        let paddr = virt_to_phys(vaddr);
        assert_eq!(paddr, original_paddr);
    }

    #[test_case]
    fn test_phys_addr_type() {
        let paddr = PhysAddr::new(0x80000000);
        assert_eq!(paddr.as_usize(), 0x80000000);
        assert_eq!(paddr.to_virt().to_phys().as_usize(), 0x80000000);
    }

    #[test_case]
    fn test_virt_addr_type() {
        let vaddr = VirtAddr::new(phys_to_virt(0x80000000));
        assert_eq!(vaddr.to_phys().as_usize(), 0x80000000);
    }

    #[test_case]
    fn test_addr_alignment() {
        let addr = PhysAddr::new(0x80001234);
        assert!(!addr.is_aligned(0x1000));
        assert_eq!(addr.align_down(0x1000).as_usize(), 0x80001000);
        assert_eq!(addr.align_up(0x1000).as_usize(), 0x80002000);

        let aligned = PhysAddr::new(0x80002000);
        assert!(aligned.is_aligned(0x1000));
        assert_eq!(aligned.align_down(0x1000).as_usize(), 0x80002000);
        assert_eq!(aligned.align_up(0x1000).as_usize(), 0x80002000);
    }
}
