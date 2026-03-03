//! Address translation utilities for virtual memory management.
//!
//! This module provides functions for converting between physical and virtual addresses.
//! Currently, it assumes identity mapping (VA == PA), but is designed to be extended
//! for Higher Half Kernel + HHDM (Higher Half Direct Mapping) in the future.
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

/// Converts a virtual address to a physical address.
///
/// **Note:** This function is specifically intended for addresses in the
/// HHDM (Higher Half Direct Map) / direct-map region. It is **not** valid
/// for arbitrary kernel virtual addresses (e.g., kernel image mappings).
///
/// Currently assumes identity mapping (VA == PA).
/// When Higher Half Kernel is enabled, this will subtract HHDM_OFFSET.
///
/// # Arguments
///
/// * `vaddr` - Virtual address in the direct-map region to convert
///
/// # Returns
///
/// Physical address corresponding to the given direct-map virtual address
#[inline(always)]
pub const fn virt_to_phys(vaddr: usize) -> usize {
    // Identity mapping: VA == PA
    // Future: When Higher Half is enabled, this will be:
    // vaddr - HHDM_OFFSET
    vaddr
}

/// Converts a physical address to a virtual address.
///
/// **Note:** This function is specifically intended for producing addresses in
/// the HHDM (Higher Half Direct Map) / direct-map region. It is **not** valid
/// for obtaining arbitrary kernel virtual addresses (e.g., kernel image VAs).
///
/// Currently assumes identity mapping (VA == PA).
/// When Higher Half Kernel is enabled, this will add HHDM_OFFSET.
///
/// # Arguments
///
/// * `paddr` - Physical address to convert
///
/// # Returns
///
/// Direct-map virtual address corresponding to the given physical address
#[inline(always)]
pub const fn phys_to_virt(paddr: usize) -> usize {
    // Identity mapping: VA == PA
    // Future: When Higher Half is enabled, this will be:
    // paddr + HHDM_OFFSET
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
pub const fn phys_to_kernel_virt(paddr: usize) -> usize {
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
pub const fn kernel_virt_to_phys(vaddr: usize) -> usize {
    virt_to_phys(vaddr)
}

/// Checks if the given virtual address is in the direct mapping region.
///
/// With identity mapping, all addresses are in the direct mapping region.
/// When Higher Half is enabled, this will check if the address is in HHDM range.
///
/// # Arguments
///
/// * `vaddr` - Virtual address to check
///
/// # Returns
///
/// `true` if the address is in the direct mapping region
#[inline(always)]
pub const fn is_direct_mapped(_vaddr: usize) -> bool {
    // With identity mapping, all addresses are direct mapped
    // Future: When Higher Half is enabled:
    // vaddr >= HHDM_OFFSET && vaddr < HHDM_OFFSET + MAX_PHYSICAL_MEMORY
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
    pub const fn to_virt(&self) -> VirtAddr {
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
    pub const fn to_phys(&self) -> PhysAddr {
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
        let vaddr = 0x80000000usize;
        let paddr = virt_to_phys(vaddr);
        assert_eq!(paddr, vaddr);
    }

    #[test_case]
    fn test_identity_mapping_phys_to_virt() {
        let paddr = 0x80000000usize;
        let vaddr = phys_to_virt(paddr);
        assert_eq!(vaddr, paddr);
    }

    #[test_case]
    fn test_roundtrip_conversion() {
        let original = 0x80001234usize;
        let paddr = virt_to_phys(original);
        let vaddr = phys_to_virt(paddr);
        assert_eq!(vaddr, original);
    }

    #[test_case]
    fn test_phys_addr_type() {
        let paddr = PhysAddr::new(0x80000000);
        assert_eq!(paddr.as_usize(), 0x80000000);
        assert_eq!(paddr.to_virt().as_usize(), 0x80000000);
    }

    #[test_case]
    fn test_virt_addr_type() {
        let vaddr = VirtAddr::new(0x80000000);
        assert_eq!(vaddr.as_usize(), 0x80000000);
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
