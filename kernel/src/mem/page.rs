extern crate alloc;

use alloc::boxed::Box;

use crate::environment::PAGE_SIZE;

#[repr(C, align(4096))]
#[derive(Clone, Copy)]
pub struct Page {
    pub data: [u8; PAGE_SIZE],
}

impl Page {
    pub const fn new() -> Self {
        Page { data: [0; PAGE_SIZE] }
    }
}

/// Allocates a number of pages.
/// 
/// # Arguments
/// * `num_of_pages` - The number of pages to allocate
/// 
/// # Returns
/// A pointer to the allocated pages.
pub fn allocate_pages(num_of_pages: usize) -> *mut Page {
    let boxed_pages = alloc::vec![Page::new(); num_of_pages].into_boxed_slice();
    Box::into_raw(boxed_pages) as *mut Page
}

/// Frees a number of pages.
/// 
/// # Arguments
/// * `pages` - A pointer to the pages to free
/// * `num_of_pages` - The number of pages to free
pub fn free_pages(pages: *mut Page, num_of_pages: usize) {
    unsafe {
        let _ = Box::from_raw(core::slice::from_raw_parts_mut(pages, num_of_pages));
    }
}