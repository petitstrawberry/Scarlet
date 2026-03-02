//! Address translation utilities for virtual memory management.
//!
//! This module provides functions for converting between physical and virtual addresses.
//! Currently, it assumes identity mapping (VA == PA), but is designed to be extended
//! for Higher Half Kernel + HHDM (Higher Half Direct Mapping) in the future.
//!
//! # Usage
//!
//! ```rust
//! use crate::vm::addr::{virt_to_phys, phys_to_virt};
//!
//! // Convert virtual address to physical address
//! let paddr = virt_to_phys(vaddr);
//!
//! // Convert physical address to virtual address
//! let vaddr = phys_to_virt(paddr);
//! ```

/// Converts a virtual address to a physical address.
///
/// Currently assumes identity mapping (VA == PA).
/// When Higher Half Kernel is enabled, this will subtract HHDM_OFFSET.
///
/// # Arguments
///
/// * `vaddr` - Virtual address to convert
///
/// # Returns
///
/// Physical address corresponding to the given virtual address
#[inline(always)]
pub const fn virt_to_phys(vaddr: usize) -> usize {
    // Identity mapping: VA == PA
    // Future: When Higher Half is enabled, this will be:
    // vaddr - HHDM_OFFSET
    vaddr
}

/// Converts a physical address to a virtual address.
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
/// Virtual address corresponding to the given physical address
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
    #[inline(always)]
    pub const fn is_aligned(&self, align: usize) -> bool {
        self.0 % align == 0
    }

    /// Returns the address aligned down to the given boundary.
    #[inline(always)]
    pub const fn align_down(&self, align: usize) -> Self {
        Self::new(self.0 & !(align - 1))
    }

    /// Returns the address aligned up to the given boundary.
    #[inline(always)]
    pub const fn align_up(&self, align: usize) -> Self {
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

    /// Returns true if the address is aligned to the given boundary.
    #[inline(always)]
    pub const fn is_aligned(&self, align: usize) -> bool {
        self.0 % align == 0
    }

    /// Returns the address aligned down to the given boundary.
    #[inline(always)]
    pub const fn align_down(&self, align: usize) -> Self {
        Self::new(self.0 & !(align - 1))
    }

    /// Returns the address aligned up to the given boundary.
    #[inline(always)]
    pub const fn align_up(&self, align: usize) -> Self {
        Self::new((self.0 + align - 1) & !(align - 1))
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
