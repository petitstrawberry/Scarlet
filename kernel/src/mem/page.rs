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

    let paddr = match crate::mem::pmm::alloc_contiguous_pages(num_of_pages) {
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
    let paddr = match crate::mem::pmm::alloc_contiguous_pages_aligned(num_of_pages, align_pages) {
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
    crate::mem::pmm::free_contiguous_pages(paddr, num_of_pages);
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

pub struct ContiguousPages {
    ptr: *mut Page,
    count: usize,
}

#[deprecated(note = "Use `ContiguousPages` instead")]
pub type PageAllocation = ContiguousPages;

impl ContiguousPages {
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

    pub fn as_vaddr(&self) -> usize {
        self.ptr as usize
    }

    pub fn contains_paddr_range(&self, paddr: usize, len: usize) -> bool {
        let self_paddr = self.as_paddr();
        let self_end = self_paddr + self.count * PAGE_SIZE;
        let range_end = paddr + len;

        paddr < self_end && range_end > self_paddr
    }
}

impl Drop for ContiguousPages {
    fn drop(&mut self) {
        if !self.ptr.is_null() && self.count > 0 {
            free_raw_pages(self.ptr, self.count);
        }
    }
}

unsafe impl Send for ContiguousPages {}
unsafe impl Sync for ContiguousPages {}

impl Clone for ContiguousPages {
    fn clone(&self) -> Self {
        let new_alloc = Self::new(self.count).expect("Failed to clone ContiguousPages");
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

impl fmt::Debug for ContiguousPages {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ContiguousPages")
            .field("ptr", &self.ptr)
            .field("count", &self.count)
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
}

impl Drop for TaskPages {
    fn drop(&mut self) {
        crate::mem::pmm::free_individual_pages(&self.pages);
    }
}
