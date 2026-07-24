//! ioremap — Dynamic I/O memory mapping.
//!
//! This module provides Linux-style `ioremap`/`iounmap` functionality for
//! dynamically mapping physical device MMIO regions into the kernel virtual
//! address space.
//!
//! Instead of statically identity-mapping (VA == PA) the entire device address
//! range at boot, individual device drivers call [`ioremap`] at runtime to
//! obtain a kernel virtual address for their MMIO registers.  When the device
//! is no longer needed, [`iounmap`] releases the virtual address back to the
//! pool.
//!
//! # Virtual Address Region
//!
//! All ioremap mappings are placed in the `IOREMAP` region defined by
//! [`crate::environment::IOREMAP_START`] and [`crate::environment::IOREMAP_END`].
//! This region sits in the gap between the HHDM end and the kernel image, so it
//! never conflicts with other kernel mappings.
//!
//! # Example
//!
//! ```rust,ignore
//! // Map a 4 KiB MMIO region starting at physical address 0x1000_0000.
//! let vaddr = ioremap(0x1000_0000, 0x1000)?;
//!
//! // Use vaddr for register access …
//!
//! // Release the mapping when done.
//! iounmap(vaddr);
//! ```

extern crate alloc;

use crate::sync::{IrqSpinLock, Once};
use alloc::collections::BTreeMap;

use crate::environment::{IOREMAP_END, IOREMAP_START, PAGE_SIZE};
use crate::vm::addr::validate_direct_map_alias;
use crate::vm::get_kernel_vm_manager;
use crate::vm::vmem::{MemoryArea, MemoryAttribute, VirtualMemoryMap, VirtualMemoryPermission};

// ---------------------------------------------------------------------------
// Internal allocator
// ---------------------------------------------------------------------------

/// Inner state of the ioremap virtual-address allocator.
struct IoremapAllocatorInner {
    /// Next free virtual address (bump pointer).
    next: usize,
    /// Exclusive end of the ioremap region.
    end: usize,
    /// Free list: maps VA start → size for previously freed regions.
    free_list: BTreeMap<usize, usize>,
}

impl IoremapAllocatorInner {
    /// Allocate `size` bytes of page-aligned virtual address space.
    ///
    /// Tries a first-fit search in the free list before falling back to bump
    /// allocation.
    fn alloc(&mut self, size: usize) -> Option<usize> {
        debug_assert_eq!(size % PAGE_SIZE, 0, "alloc size must be page-aligned");

        // First-fit from free list.
        let found_va = self
            .free_list
            .iter()
            .find(|(_, s)| **s >= size)
            .map(|(&va, _)| va);

        if let Some(va) = found_va {
            let free_size = self.free_list.remove(&va).unwrap();
            // Return any excess tail back to the free list.
            if free_size > size {
                self.free_list.insert(va + size, free_size - size);
            }
            return Some(va);
        }

        // Bump allocation.
        let va = self.next;
        if va
            .checked_add(size)
            .map(|end| end > self.end)
            .unwrap_or(true)
        {
            return None; // Out of virtual address space.
        }
        self.next = va + size;
        Some(va)
    }

    /// Return a previously-allocated virtual address range to the free list.
    fn free(&mut self, va: usize, size: usize) {
        debug_assert_eq!(va % PAGE_SIZE, 0);
        debug_assert_eq!(size % PAGE_SIZE, 0);
        // Simple insertion; coalescing of adjacent blocks is a future improvement.
        self.free_list.insert(va, size);
    }
}

static IOREMAP_ALLOCATOR: Once<IrqSpinLock<IoremapAllocatorInner>> = Once::new();

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialize the ioremap subsystem.
///
/// Must be called exactly once during kernel virtual memory initialization,
/// after the kernel VM manager and heap are available.  Subsequent calls are
/// no-ops.
pub fn ioremap_init() {
    IOREMAP_ALLOCATOR.call_once(|| {
        IrqSpinLock::new(IoremapAllocatorInner {
            next: IOREMAP_START,
            end: IOREMAP_END.wrapping_add(1), // exclusive end
            free_list: BTreeMap::new(),
        })
    });
}

/// Map a physical device MMIO region into the kernel virtual address space.
///
/// Allocates a virtual address from the IOREMAP region and installs a
/// read/write mapping for `[paddr, paddr + size)` in the kernel page tables.
/// The physical address is aligned down to a page boundary; the returned
/// virtual address preserves the offset within that page.
///
/// # Arguments
///
/// * `paddr` - Physical base address of the device memory (need not be page-aligned).
/// * `size`  - Number of bytes to map (must be > 0).
///
/// # Returns
///
/// * `Ok(vaddr)` – Virtual address corresponding to `paddr`; the caller uses
///   this address for MMIO register accesses.
/// * `Err(&'static str)` – Descriptive error if mapping failed.
pub fn ioremap(paddr: usize, size: usize) -> Result<usize, &'static str> {
    map_physical_memory(paddr, size, MemoryAttribute::Device)
}

/// Map physical RAM into the kernel virtual address space as normal memory.
///
/// This is intended for firmware-owned RAM that is deliberately absent from
/// the sparse runtime direct map but still needs a narrow kernel mapping. It
/// must not be used for MMIO registers; use [`ioremap`] for device memory.
///
/// # Arguments
///
/// * `paddr` - Physical base address of the RAM range.
/// * `size` - Number of bytes to map; must be greater than zero.
///
/// # Returns
///
/// A kernel virtual address with normal cacheable memory attributes, or an
/// error when the range conflicts with the direct map or cannot be mapped.
pub fn memremap_normal(paddr: usize, size: usize) -> Result<usize, &'static str> {
    map_physical_memory(paddr, size, MemoryAttribute::Normal)
}

fn map_physical_memory(
    paddr: usize,
    size: usize,
    memory_attribute: MemoryAttribute,
) -> Result<usize, &'static str> {
    if size == 0 {
        return Err("physical map: size must be > 0");
    }

    let alloc_guard = IOREMAP_ALLOCATOR
        .get()
        .ok_or("physical map: subsystem not initialized")?;

    // Align physical address down to a page boundary.
    let offset = paddr & (PAGE_SIZE - 1);
    let aligned_paddr = paddr - offset;
    let aligned_size = checked_align_up(
        size.checked_add(offset)
            .ok_or("ioremap: physical range size overflows")?,
        PAGE_SIZE,
    )?;
    let aligned_end = aligned_paddr
        .checked_add(aligned_size)
        .and_then(|end| end.checked_sub(1))
        .ok_or("ioremap: physical range overflows")?;
    let physical_area = MemoryArea::new(aligned_paddr, aligned_end);

    // Reject incompatible aliases before consuming IOREMAP virtual address space.
    validate_direct_map_alias(physical_area, memory_attribute)?;

    // Reserve virtual address space.
    let alloc_va = alloc_guard
        .lock()
        .alloc(aligned_size)
        .ok_or("physical map: virtual address space exhausted")?;

    // Build the VirtualMemoryMap descriptor.
    let vmmap = VirtualMemoryMap::new(
        physical_area,
        MemoryArea::new(alloc_va, alloc_va + aligned_size - 1),
        VirtualMemoryPermission::Read as usize | VirtualMemoryPermission::Write as usize,
        true, // shared: accessible from any address space using the kernel PT
        None,
    )
    .with_memory_attribute(memory_attribute);

    let km = get_kernel_vm_manager();

    // Register the mapping with the kernel VM manager.
    if let Err(e) = km.add_memory_map(vmmap.clone()) {
        alloc_guard.lock().free(alloc_va, aligned_size);
        return Err(e);
    }

    // Install the mapping into the kernel page tables.
    if let Some(mut pt) = km.get_root_page_table() {
        let map_result = pt.map_memory_area(vmmap, true, true);
        drop(pt);
        if let Err(e) = map_result {
            km.remove_memory_map_by_addr(alloc_va);
            alloc_guard.lock().free(alloc_va, aligned_size);
            return Err(e);
        }
    } else {
        km.remove_memory_map_by_addr(alloc_va);
        alloc_guard.lock().free(alloc_va, aligned_size);
        return Err("ioremap: no root page table available");
    }

    let vaddr = alloc_va + offset;

    crate::early_println!(
        "[ioremap] paddr={:#x} -> vaddr={:#x} (size={:#x})",
        paddr,
        vaddr,
        size
    );

    Ok(vaddr)
}

/// Unmap a device MMIO region previously mapped with [`ioremap`].
///
/// Removes the kernel page table entries for the mapping and returns the
/// virtual address range to the ioremap allocator.
///
/// # Arguments
///
/// * `vaddr` - Virtual address returned by a previous [`ioremap`] call.
pub fn iounmap(vaddr: usize) {
    let km = get_kernel_vm_manager();
    if let Some(map) = km.remove_memory_map_by_addr(vaddr) {
        if let Some(alloc_guard) = IOREMAP_ALLOCATOR.get() {
            alloc_guard.lock().free(map.vmarea.start, map.vmarea.size());
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn checked_align_up(val: usize, align: usize) -> Result<usize, &'static str> {
    val.checked_add(align - 1)
        .map(|value| value & !(align - 1))
        .ok_or("ioremap: range alignment overflows")
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_align_up_zero() {
        assert_eq!(checked_align_up(0, PAGE_SIZE).unwrap(), 0);
    }

    #[test_case]
    fn test_align_up_already_aligned() {
        assert_eq!(checked_align_up(PAGE_SIZE, PAGE_SIZE).unwrap(), PAGE_SIZE);
    }

    #[test_case]
    fn test_align_up_one_byte_over() {
        assert_eq!(
            checked_align_up(PAGE_SIZE + 1, PAGE_SIZE).unwrap(),
            2 * PAGE_SIZE
        );
    }

    #[test_case]
    fn test_allocator_basic_alloc_free() {
        let mut inner = IoremapAllocatorInner {
            next: IOREMAP_START,
            end: IOREMAP_START + 4 * PAGE_SIZE,
            free_list: BTreeMap::new(),
        };

        let va1 = inner.alloc(PAGE_SIZE).expect("first alloc should succeed");
        assert_eq!(va1, IOREMAP_START);

        let va2 = inner.alloc(PAGE_SIZE).expect("second alloc should succeed");
        assert_eq!(va2, IOREMAP_START + PAGE_SIZE);

        // Free va1, then reallocate it.
        inner.free(va1, PAGE_SIZE);
        let va3 = inner.alloc(PAGE_SIZE).expect("re-alloc from free list");
        assert_eq!(va3, va1, "should reuse freed virtual address");
    }

    #[test_case]
    fn test_allocator_exhaustion() {
        let mut inner = IoremapAllocatorInner {
            next: IOREMAP_START,
            end: IOREMAP_START + PAGE_SIZE, // only 1 page available
            free_list: BTreeMap::new(),
        };

        let _va = inner.alloc(PAGE_SIZE).expect("alloc should succeed");
        assert!(
            inner.alloc(PAGE_SIZE).is_none(),
            "second alloc should fail (exhausted)"
        );
    }
}
