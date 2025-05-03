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
pub fn allocate_raw_pages(num_of_pages: usize) -> *mut Page {
    let boxed_pages = allocate_boxed_pages(num_of_pages);
    Box::into_raw(boxed_pages) as *mut Page
}

/// Frees a number of pages.
/// 
/// # Arguments
/// * `pages` - A pointer to the pages to free
/// * `num_of_pages` - The number of pages to free
pub fn free_raw_pages(pages: *mut Page, num_of_pages: usize) {
    unsafe {
        let boxed_pages = Box::from_raw(core::slice::from_raw_parts_mut(pages, num_of_pages));
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
    let boxed_pages = alloc::vec![Page::new(); num_of_pages].into_boxed_slice();
    boxed_pages
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