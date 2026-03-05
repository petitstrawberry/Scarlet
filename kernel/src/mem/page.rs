extern crate alloc;

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::fmt;

use crate::environment::PAGE_SIZE;
use crate::vm::addr::{phys_to_virt, virt_to_phys};

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

    let paddr = match crate::mem::pmm::alloc_pages(num_of_pages) {
        Some(addr) => addr,
        None => return core::ptr::null_mut(),
    };

    // Identity mapping: VA = PA
    let vaddr = phys_to_virt(paddr) as *mut Page;

    // Zero-initialize the pages
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
    let paddr = match crate::mem::pmm::alloc_pages_aligned(num_of_pages, align_pages) {
        Some(addr) => addr,
        None => return core::ptr::null_mut(),
    };

    let vaddr = phys_to_virt(paddr) as *mut Page;

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
    crate::mem::pmm::free_pages(paddr, num_of_pages);
}

/// Allocates a number of pages from the heap and returns them as a boxed slice.
/// Note: This uses the global heap allocator, not PMM.
/// For PMM-backed allocations, use `PageAllocation::new()` instead.
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
    note = "This function uses the global heap allocator. Use PageAllocation::new() for PMM-backed allocations instead."
)]
pub fn allocate_boxed_pages(num_of_pages: usize) -> Box<[Page]> {
    use alloc::alloc::{Layout, alloc_zeroed};
    use core::ptr;

    let layout = Layout::array::<Page>(num_of_pages).expect("Layout calculation failed");

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
    note = "This function uses the global heap allocator. Use allocate_raw_pages_aligned() with PageAllocation for PMM-backed allocations instead."
)]
pub fn allocate_boxed_pages_aligned(num_of_pages: usize, align: usize) -> Box<[Page]> {
    use alloc::alloc::{Layout, alloc_zeroed};
    use core::ptr;

    let size = num_of_pages * PAGE_SIZE;
    let layout = Layout::from_size_align(size, align).expect("Layout calculation failed");

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

/// A RAII wrapper for contiguous page allocations directly from PMM.
///
/// This struct owns a contiguous block of pages and automatically frees them
/// when dropped. This prevents memory leaks and ensures safe cleanup.
pub struct PageAllocation {
    ptr: *mut Page,
    count: usize,
}

impl PageAllocation {
    /// Allocate a contiguous block of pages directly from PMM.
    ///
    /// # Arguments
    /// * `count` - Number of pages to allocate
    ///
    /// # Returns
    /// Some(PageAllocation) on success, None on failure
    pub fn new(count: usize) -> Option<Self> {
        if count == 0 {
            return None;
        }

        let ptr = allocate_raw_pages(count);
        if ptr.is_null() {
            None
        } else {
            Some(Self { ptr, count })
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
        self.ptr.add(index)
    }

    /// Convert to raw parts (ptr, count) without freeing.
    ///
    /// After calling this, the caller is responsible for freeing the memory.
    pub fn into_raw(self) -> (*mut Page, usize) {
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
        Self { ptr, count }
    }

    /// Split the allocation at the given page offset.
    ///
    /// Consumes self and returns two optional allocations:
    /// - Left: pages [0, at) - `at` pages total
    /// - Right: pages [at, count) - `count - at` pages total
    ///
    /// # Returns
    /// - `(None, Some(right))` if `at == 0`
    /// - `(Some(left), None)` if `at >= count`
    /// - `(Some(left), Some(right))` otherwise
    ///
    /// # Example
    /// ```
    /// let alloc = PageAllocation::new(10).unwrap();
    /// let (left, right) = alloc.split_at(3);
    /// // left has 3 pages, right has 7 pages
    /// ```
    pub fn split_at(self, at: usize) -> (Option<Self>, Option<Self>) {
        if at == 0 {
            return (None, Some(self));
        }

        if at >= self.count {
            return (Some(self), None);
        }

        let left_count = at;
        let right_count = self.count - at;

        let left_ptr = self.ptr;
        let right_ptr = unsafe { self.ptr.add(at) };

        let _ = self.into_raw();

        let left = unsafe { Self::from_raw(left_ptr, left_count) };
        let right = unsafe { Self::from_raw(right_ptr, right_count) };

        (Some(left), Some(right))
    }

    /// Get the virtual address of the first page.
    pub fn as_vaddr(&self) -> usize {
        self.ptr as usize
    }

    /// Check if a given physical address range overlaps with this allocation.
    ///
    /// # Arguments
    /// * `paddr` - Physical address to check
    /// * `len` - Length in bytes
    pub fn contains_paddr_range(&self, paddr: usize, len: usize) -> bool {
        let self_paddr = self.as_paddr();
        let self_end = self_paddr + self.count * PAGE_SIZE;
        let range_end = paddr + len;

        paddr < self_end && range_end > self_paddr
    }

    /// Remove a sub-range from this allocation and return the remaining parts.
    ///
    /// This is useful for munmap operations where a partial range of a larger
    /// allocation is being unmapped.
    ///
    /// # Arguments
    /// * `offset_pages` - Page offset within this allocation to start removing
    /// * `count_pages` - Number of pages to remove
    ///
    /// # Returns
    /// A tuple of:
    /// - Vector of remaining allocations (0, 1, or 2 allocations)
    /// - The removed allocation (if any)
    pub fn extract_range(
        self,
        offset_pages: usize,
        count_pages: usize,
    ) -> (Vec<Self>, Option<Self>) {
        if offset_pages >= self.count || count_pages == 0 {
            return (alloc::vec![self], None);
        }

        let end_offset = (offset_pages + count_pages).min(self.count);
        let actual_count = end_offset - offset_pages;

        if offset_pages == 0 && actual_count >= self.count {
            return (Vec::new(), Some(self));
        }

        let mut remaining = Vec::new();

        // Left: [0, offset_pages)
        // Middle (extracted): [offset_pages, end_offset)
        // Right: [end_offset, self.count)

        let left_count = offset_pages;
        let right_count = self.count - end_offset;

        let base_ptr = self.ptr;
        let extracted_ptr = unsafe { base_ptr.add(offset_pages) };
        let extracted = unsafe { Self::from_raw(extracted_ptr, actual_count) };

        let _ = self.into_raw();

        if left_count > 0 {
            let left = unsafe { Self::from_raw(base_ptr, left_count) };
            remaining.push(left);
        }

        if right_count > 0 {
            let right_ptr = unsafe { base_ptr.add(end_offset) };
            let right = unsafe { Self::from_raw(right_ptr, right_count) };
            remaining.push(right);
        }

        (remaining, Some(extracted))
    }
}

impl Drop for PageAllocation {
    fn drop(&mut self) {
        if !self.ptr.is_null() && self.count > 0 {
            free_raw_pages(self.ptr, self.count);
        }
    }
}

// SAFETY: PageAllocation owns the memory and frees it on drop
unsafe impl Send for PageAllocation {}
unsafe impl Sync for PageAllocation {}

impl Clone for PageAllocation {
    fn clone(&self) -> Self {
        let new_alloc = Self::new(self.count).expect("Failed to clone PageAllocation");
        unsafe {
            core::ptr::copy_nonoverlapping(
                self.ptr as *const u8,
                new_alloc.ptr as *mut u8,
                self.count * PAGE_SIZE,
            );
        }
        new_alloc
    }
}

impl fmt::Debug for PageAllocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PageAllocation")
            .field("ptr", &self.ptr)
            .field("count", &self.count)
            .finish()
    }
}
