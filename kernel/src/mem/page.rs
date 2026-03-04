extern crate alloc;

use alloc::boxed::Box;
use core::fmt;

use crate::environment::PAGE_SIZE;

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

/// Allocates a number of pages.
///
/// # Arguments
/// * `num_of_pages` - The number of pages to allocate
///
/// # Returns
/// A pointer to the allocated pages.
pub fn allocate_raw_pages(num_of_pages: usize) -> *mut Page {
    let boxed_pages = allocate_boxed_pages(num_of_pages);
    Box::into_raw(boxed_pages) as *mut Page
}

/// Allocates a number of pages with custom alignment.
///
/// # Arguments
/// * `num_of_pages` - The number of pages to allocate
/// * `align` - The alignment in bytes (must be a power of 2 and >= PAGE_SIZE)
///
/// # Returns
/// A pointer to the allocated pages with the specified alignment.
pub fn allocate_raw_pages_aligned(num_of_pages: usize, align: usize) -> *mut Page {
    let boxed_pages = allocate_boxed_pages_aligned(num_of_pages, align);
    Box::into_raw(boxed_pages) as *mut Page
}

/// Frees a number of pages.
///
/// # Arguments
/// * `pages` - A pointer to the pages to free
/// * `num_of_pages` - The number of pages to free
pub fn free_raw_pages(pages: *mut Page, num_of_pages: usize) {
    unsafe {
        let boxed_pages = Box::from_raw(core::ptr::slice_from_raw_parts_mut(pages, num_of_pages));
        free_boxed_pages(boxed_pages);
    }
}

/// Allocates a number of pages and returns them as a boxed slice.
///
/// # Arguments
/// * `num_of_pages` - The number of pages to allocate
///  
/// # Returns
/// A boxed slice of the allocated pages.
///
pub fn allocate_boxed_pages(num_of_pages: usize) -> Box<[Page]> {
    // Allocate raw memory and initialize it
    use alloc::alloc::{Layout, alloc_zeroed};
    use core::ptr;

    let layout = Layout::array::<Page>(num_of_pages).expect("Layout calculation failed");

    unsafe {
        let ptr = alloc_zeroed(layout) as *mut Page;
        if ptr.is_null() {
            alloc::alloc::handle_alloc_error(layout);
        }

        // Convert raw pointer to Box<[Page]>
        let slice = ptr::slice_from_raw_parts_mut(ptr, num_of_pages);
        Box::from_raw(slice)
    }
}

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
///
/// # Arguments
/// * `pages` - A boxed slice of pages to free
///
pub fn free_boxed_pages(pages: Box<[Page]>) {
    // The Box will be automatically freed when it goes out of scope
    drop(pages);
}

/// Frees a boxed page.
///
/// # Arguments
/// * `page` - A boxed page to free
///
pub fn free_boxed_page(page: Box<Page>) {
    // The Box will be automatically freed when it goes out of scope
    drop(page);
}

/// A RAII wrapper for contiguous page allocations.
///
/// This struct owns a contiguous block of pages and automatically frees them
/// when dropped. This prevents memory leaks and ensures safe cleanup.
pub struct PageAllocation {
    ptr: *mut Page,
    count: usize,
}

impl PageAllocation {
    /// Allocate a contiguous block of pages.
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
    /// that was previously obtained from `allocate_raw_pages`.
    pub unsafe fn from_raw(ptr: *mut Page, count: usize) -> Self {
        debug_assert!(!ptr.is_null());
        debug_assert!(count > 0);
        Self { ptr, count }
    }
}

impl Drop for PageAllocation {
    fn drop(&mut self) {
        if !self.ptr.is_null() && self.count > 0 {
            unsafe {
                free_raw_pages(self.ptr, self.count);
            }
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
