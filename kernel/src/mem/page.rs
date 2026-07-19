extern crate alloc;

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::fmt;

use crate::environment::PAGE_SIZE;
use crate::vm::addr::{phys_to_virt, virt_to_phys};
use crate::vm::vmem::{MemoryArea, MemoryAttribute};

#[repr(C, align(4096))]
#[derive(Clone, Debug)]
pub struct Page {
    pub data: [u8; PAGE_SIZE],
}

impl Page {
    pub const fn new() -> Self {
        Page {
            data: [0; PAGE_SIZE],
        }
    }
}

/// Allocates a number of pages from PMM.
///
/// # Arguments
/// * `num_of_pages` - The number of pages to allocate
///
/// # Returns
/// A pointer to the allocated pages, or null if allocation failed.
pub fn allocate_raw_pages(num_of_pages: usize) -> *mut Page {
    if num_of_pages == 0 {
        return core::ptr::null_mut();
    }

    let paddr = match crate::mem::pmm::alloc_contiguous_pages(num_of_pages) {
        Some(addr) => addr,
        None => return core::ptr::null_mut(),
    };

    // Identity mapping: VA = PA
    let vaddr = phys_to_virt(paddr) as *mut Page;

    // Zero-initialize the pages
    // SAFETY: PMM returned `num_of_pages` contiguous direct-mapped pages at
    // `vaddr`, so the byte range is writable and exactly covers the allocation.
    unsafe {
        core::ptr::write_bytes(vaddr as *mut u8, 0, num_of_pages * PAGE_SIZE);
    }

    vaddr
}

/// Allocates a number of pages with custom alignment from PMM.
///
/// # Arguments
/// * `num_of_pages` - The number of pages to allocate
/// * `align` - The alignment in bytes (must be a power of 2 and >= PAGE_SIZE)
///
/// # Returns
/// A pointer to the allocated pages with the specified alignment.
pub fn allocate_raw_pages_aligned(num_of_pages: usize, align: usize) -> *mut Page {
    if num_of_pages == 0 {
        return core::ptr::null_mut();
    }

    let align_pages = align / PAGE_SIZE;
    let paddr = match crate::mem::pmm::alloc_contiguous_pages_aligned(num_of_pages, align_pages) {
        Some(addr) => addr,
        None => return core::ptr::null_mut(),
    };

    let vaddr = phys_to_virt(paddr) as *mut Page;

    // SAFETY: PMM returned `num_of_pages` contiguous direct-mapped pages at
    // `vaddr`, so the byte range is writable and exactly covers the allocation.
    unsafe {
        core::ptr::write_bytes(vaddr as *mut u8, 0, num_of_pages * PAGE_SIZE);
    }

    vaddr
}

/// Frees a number of pages back to PMM.
///
/// # Arguments
/// * `pages` - A pointer to the pages to free
/// * `num_of_pages` - The number of pages to free
pub fn free_raw_pages(pages: *mut Page, num_of_pages: usize) {
    if pages.is_null() || num_of_pages == 0 {
        return;
    }

    let paddr = virt_to_phys(pages as usize);
    crate::mem::pmm::free_contiguous_pages(paddr, num_of_pages);
}

/// Allocates a number of pages from the heap and returns them as a boxed slice.
/// Note: This uses the global heap allocator, not PMM.
/// For PMM-backed allocations, use `ContiguousPages::new()` instead.
///
/// # Arguments
/// * `num_of_pages` - The number of pages to allocate
///
/// # Returns
/// A boxed slice of the allocated pages.
///
/// # Panics
/// Panics if allocation fails.
#[deprecated(
    since = "0.1.0",
    note = "This function uses the global heap allocator. Use ContiguousPages::new() for PMM-backed allocations instead."
)]
pub fn allocate_boxed_pages(num_of_pages: usize) -> Box<[Page]> {
    use alloc::alloc::{Layout, alloc_zeroed};
    use core::ptr;

    let layout = Layout::array::<Page>(num_of_pages).expect("Layout calculation failed");

    // SAFETY: the allocation layout describes a `num_of_pages`-long Page array
    // returned by the global allocator immediately above.
    unsafe {
        let ptr = alloc_zeroed(layout) as *mut Page;
        if ptr.is_null() {
            alloc::alloc::handle_alloc_error(layout);
        }

        let slice = ptr::slice_from_raw_parts_mut(ptr, num_of_pages);
        Box::from_raw(slice)
    }
}

/// Allocates aligned pages from the heap and returns them as a boxed slice.
/// Note: This uses the global heap allocator, not PMM.
///
/// # Arguments
/// * `num_of_pages` - The number of pages to allocate
/// * `align` - The alignment in bytes
///
/// # Returns
/// A boxed slice of the allocated pages.
///
/// # Panics
/// Panics if allocation fails.
#[deprecated(
    since = "0.1.0",
    note = "This function uses the global heap allocator. Use allocate_raw_pages_aligned() with ContiguousPages for PMM-backed allocations instead."
)]
pub fn allocate_boxed_pages_aligned(num_of_pages: usize, align: usize) -> Box<[Page]> {
    use alloc::alloc::{Layout, alloc_zeroed};
    use core::ptr;

    let size = num_of_pages * PAGE_SIZE;
    let layout = Layout::from_size_align(size, align).expect("Layout calculation failed");

    // SAFETY: the allocation layout describes `size` bytes returned by the
    // global allocator immediately below.
    unsafe {
        let ptr = alloc_zeroed(layout) as *mut Page;
        if ptr.is_null() {
            alloc::alloc::handle_alloc_error(layout);
        }

        let slice = ptr::slice_from_raw_parts_mut(ptr, num_of_pages);
        Box::from_raw(slice)
    }
}

/// Frees a boxed slice of pages.
/// Note: The Box will be automatically freed to the heap when dropped.
pub fn free_boxed_pages(_pages: Box<[Page]>) {
    // The Box will be automatically freed when it goes out of scope
    drop(_pages);
}

/// Frees a boxed page.
/// Note: The Box will be automatically freed to the heap when dropped.
pub fn free_boxed_page(_page: Box<Page>) {
    // The Box will be automatically freed when it goes out of scope
    drop(_page);
}

/// PMM-backed contiguous pages with an owned direct-map memory attribute.
pub struct ContiguousPages {
    ptr: *mut Page,
    count: usize,
    memory_attribute: MemoryAttribute,
}

impl ContiguousPages {
    pub fn new(count: usize) -> Option<Self> {
        if count == 0 {
            return None;
        }

        let ptr = allocate_raw_pages(count);
        if ptr.is_null() {
            None
        } else {
            Some(Self {
                ptr,
                count,
                memory_attribute: MemoryAttribute::Normal,
            })
        }
    }

    /// Allocate contiguous pages with a minimum physical alignment.
    ///
    /// # Arguments
    ///
    /// * `count` - Number of pages to allocate.
    /// * `align` - Physical alignment in bytes. Values below `PAGE_SIZE` are
    ///   rounded up to `PAGE_SIZE`.
    ///
    /// # Returns
    ///
    /// Aligned contiguous pages, or `None` when allocation fails.
    pub fn new_aligned(count: usize, align: usize) -> Option<Self> {
        if count == 0 {
            return None;
        }

        let align = align.max(PAGE_SIZE);
        let ptr = allocate_raw_pages_aligned(count, align);
        if ptr.is_null() {
            None
        } else {
            Some(Self {
                ptr,
                count,
                memory_attribute: MemoryAttribute::Normal,
            })
        }
    }

    /// Get a pointer to the first page.
    pub fn as_ptr(&self) -> *mut Page {
        self.ptr
    }

    /// Get the physical address of the first page.
    pub fn as_paddr(&self) -> usize {
        virt_to_phys(self.ptr as usize)
    }

    /// Returns the current direct-map memory attribute for this allocation.
    ///
    /// # Returns
    ///
    /// The attribute installed for every page in this allocation's HHDM range.
    pub const fn memory_attribute(&self) -> MemoryAttribute {
        self.memory_attribute
    }

    /// Retags this allocation's complete physical and HHDM range.
    ///
    /// When transitioning away from Normal memory, prior CPU writes are
    /// published and stale Normal cache lines are invalidated while the old
    /// mapping is still valid. When restoring a device alias to Normal, cache
    /// invalidation happens only after the Normal alias has been installed again.
    ///
    /// # Arguments
    ///
    /// * `memory_attribute` - Attribute to install for every page in this allocation.
    ///
    /// # Returns
    ///
    /// `Ok(())` after the runtime direct-map metadata and active kernel HHDM
    /// mappings have been updated, or an error if the range cannot be retagged.
    pub fn retag_memory_attribute(
        &mut self,
        memory_attribute: MemoryAttribute,
    ) -> Result<(), &'static str> {
        if self.memory_attribute == memory_attribute {
            return Ok(());
        }

        let byte_len = self.byte_len()?;
        if self.memory_attribute == MemoryAttribute::Normal
            && memory_attribute != MemoryAttribute::Normal
        {
            crate::arch::clean_invalidate_dcache_to_poc_range(self.as_vaddr(), byte_len);
        }

        let original_attribute =
            crate::vm::retag_direct_map_memory_attribute(self.physical_area()?, memory_attribute)?;
        debug_assert_eq!(original_attribute, self.memory_attribute);
        self.memory_attribute = memory_attribute;

        if self.memory_attribute == MemoryAttribute::Normal
            && original_attribute != MemoryAttribute::Normal
        {
            crate::arch::invalidate_dcache_to_poc_range(self.as_vaddr(), byte_len);
        }

        Ok(())
    }

    /// Get the number of pages.
    pub fn len(&self) -> usize {
        self.count
    }

    /// Check if empty (always false for valid allocations).
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Get a pointer to a specific page.
    ///
    /// # Safety
    /// `index` must be less than `count`.
    pub unsafe fn page_ptr(&self, index: usize) -> *mut Page {
        debug_assert!(index < self.count);
        // SAFETY: the caller guarantees `index < self.count`, and this
        // allocation owns a contiguous `count`-long Page array.
        unsafe { self.ptr.add(index) }
    }

    /// Convert to raw parts (ptr, count) without freeing.
    ///
    /// After restoring the allocation's direct-map range to Normal, this
    /// transfers ownership to the caller, which is responsible for freeing the
    /// memory.
    pub fn into_raw(mut self) -> (*mut Page, usize) {
        self.restore_normal_before_release();
        let ptr = self.ptr;
        let count = self.count;
        core::mem::forget(self);
        (ptr, count)
    }

    /// Create from raw parts.
    ///
    /// # Safety
    /// `ptr` must point to a valid allocation of `count` pages
    /// that was previously obtained from PMM.
    pub unsafe fn from_raw(ptr: *mut Page, count: usize) -> Self {
        debug_assert!(!ptr.is_null());
        debug_assert!(count > 0);
        Self {
            ptr,
            count,
            memory_attribute: MemoryAttribute::Normal,
        }
    }

    pub fn as_vaddr(&self) -> usize {
        self.ptr as usize
    }

    pub fn contains_paddr_range(&self, paddr: usize, len: usize) -> bool {
        let self_paddr = self.as_paddr();
        let self_end = self_paddr + self.count * PAGE_SIZE;
        let range_end = paddr + len;

        paddr < self_end && range_end > self_paddr
    }

    fn byte_len(&self) -> Result<usize, &'static str> {
        self.count
            .checked_mul(PAGE_SIZE)
            .ok_or("PMM allocation byte length overflows")
    }

    fn physical_area(&self) -> Result<MemoryArea, &'static str> {
        let paddr = self.as_paddr();
        let end = paddr
            .checked_add(self.byte_len()?)
            .and_then(|end| end.checked_sub(1))
            .ok_or("PMM allocation physical range overflows")?;
        Ok(MemoryArea::new(paddr, end))
    }

    fn restore_normal_before_release(&mut self) {
        if self.memory_attribute != MemoryAttribute::Normal {
            self.retag_memory_attribute(MemoryAttribute::Normal)
                .expect("failed to restore PMM allocation to Normal before release");
        }
    }
}

impl Drop for ContiguousPages {
    fn drop(&mut self) {
        if !self.ptr.is_null() && self.count > 0 {
            self.restore_normal_before_release();
            free_raw_pages(self.ptr, self.count);
        }
    }
}

unsafe impl Send for ContiguousPages {}
unsafe impl Sync for ContiguousPages {}

impl Clone for ContiguousPages {
    fn clone(&self) -> Self {
        let mut new_alloc = Self::new(self.count).expect("Failed to clone ContiguousPages");
        let byte_len = self
            .byte_len()
            .expect("ContiguousPages clone byte length overflow");
        if self.memory_attribute == MemoryAttribute::Normal {
            // SAFETY: the two live PMM allocations are distinct, contiguous,
            // and each covers `byte_len` bytes.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    self.ptr as *const u8,
                    new_alloc.ptr as *mut u8,
                    byte_len,
                );
            }
        } else {
            for offset in 0..byte_len {
                // SAFETY: both byte addresses are inside their respective live
                // PMM allocations. Volatile access is used for a Device alias.
                unsafe {
                    let value = core::ptr::read_volatile((self.ptr as *const u8).add(offset));
                    core::ptr::write_volatile((new_alloc.ptr as *mut u8).add(offset), value);
                }
            }
        }
        if self.memory_attribute != MemoryAttribute::Normal {
            new_alloc
                .retag_memory_attribute(self.memory_attribute)
                .expect("Failed to retag cloned ContiguousPages");
        }
        new_alloc
    }
}

impl fmt::Debug for ContiguousPages {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ContiguousPages")
            .field("ptr", &self.ptr)
            .field("count", &self.count)
            .field("memory_attribute", &self.memory_attribute)
            .finish()
    }
}

pub struct TaskPages {
    pages: Vec<usize>,
}

impl TaskPages {
    pub fn new(count: usize) -> Option<Self> {
        crate::mem::pmm::alloc_individual_pages(count).map(|pages| Self { pages })
    }

    pub fn page_paddr(&self, index: usize) -> Option<usize> {
        self.pages.get(index).copied()
    }

    pub fn len(&self) -> usize {
        self.pages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pages.is_empty()
    }

    pub fn free_range(&mut self, offset: usize, count: usize) {
        if offset >= self.pages.len() {
            return;
        }
        let end = (offset + count).min(self.pages.len());
        let to_free: Vec<usize> = self.pages.drain(offset..end).collect();
        crate::mem::pmm::free_individual_pages(&to_free);
    }

    pub fn reclaim_paddr_range(&mut self, start: usize, end: usize) -> usize {
        if self.pages.is_empty() {
            return 0;
        }

        let mut to_free = Vec::new();
        self.pages.retain(|&paddr| {
            let page_end = paddr.saturating_add(PAGE_SIZE - 1);
            let in_range = paddr >= start && page_end <= end;
            if in_range {
                to_free.push(paddr);
                false
            } else {
                true
            }
        });

        let freed = to_free.len();
        if freed > 0 {
            crate::mem::pmm::free_individual_pages(&to_free);
        }
        freed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn task_pages_reclaim_paddr_range_reclaims_only_covered_pages() {
        let mut pages = TaskPages::new(4).expect("TaskPages allocation failed");
        let p0 = pages.page_paddr(0).unwrap();
        let p1 = pages.page_paddr(1).unwrap();

        let start = core::cmp::min(p0, p1);
        let end = core::cmp::max(p0, p1) + PAGE_SIZE - 1;

        let before = pages.len();
        let freed = pages.reclaim_paddr_range(start, end);
        assert!(freed >= 1);
        assert!(pages.len() < before);
    }
}

impl Drop for TaskPages {
    fn drop(&mut self) {
        crate::mem::pmm::free_individual_pages(&self.pages);
    }
}
