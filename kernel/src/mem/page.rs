extern crate alloc;

use alloc::boxed::Box;

use crate::environment::PAGE_SIZE;

#[repr(C, align(4096))]
#[derive(Clone, Copy, Debug)]
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

/// Allocates a number of pages as a *single contiguous allocation*.
///
/// # Arguments
/// * `num_of_pages` - The number of pages to allocate
///
/// # Returns
/// A pointer to the allocated pages.
///
/// # Notes
/// - This is intended for cases that require contiguous backing (e.g. DMA buffers).
/// - Do NOT split this allocation into per-page `Box<Page>` values; that violates
///   the allocator contract.
pub fn allocate_contiguous_raw_pages(num_of_pages: usize) -> *mut Page {
    let boxed_pages = allocate_contiguous_boxed_pages(num_of_pages);
    Box::into_raw(boxed_pages) as *mut Page
}

/// Frees pages allocated by [`allocate_contiguous_raw_pages`].
///
/// # Arguments
/// * `pages` - A pointer to the pages to free
/// * `num_of_pages` - The number of pages to free
pub fn free_contiguous_raw_pages(pages: *mut Page, num_of_pages: usize) {
    unsafe {
        let boxed_pages = Box::from_raw(core::ptr::slice_from_raw_parts_mut(pages, num_of_pages));
        free_contiguous_boxed_pages(boxed_pages);
    }
}

/// Allocates a number of pages as a *single contiguous boxed slice*.
///
/// # Arguments
/// * `num_of_pages` - The number of pages to allocate
///  
/// # Returns
/// A boxed slice of the allocated pages.
///
pub fn allocate_contiguous_boxed_pages(num_of_pages: usize) -> Box<[Page]> {
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

/// Frees pages allocated by [`allocate_contiguous_boxed_pages`].
///
/// # Arguments
/// * `pages` - A boxed slice of pages to free
///
pub fn free_contiguous_boxed_pages(pages: Box<[Page]>) {
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

/// Allocates a single page and returns it as a boxed page.
///
/// Each page is independently allocated, so it can be freed individually.
/// Use this for ELF segments, user stacks, mmap, brk, etc.
///
/// # Returns
/// A boxed page, zero-initialized.
pub fn allocate_page() -> Box<Page> {
    use alloc::alloc::{Layout, alloc_zeroed};

    let layout = Layout::new::<Page>();

    unsafe {
        let ptr = alloc_zeroed(layout) as *mut Page;
        if ptr.is_null() {
            alloc::alloc::handle_alloc_error(layout);
        }
        Box::from_raw(ptr)
    }
}
