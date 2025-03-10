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

pub fn allocate_pages(num_of_pages: usize) -> *mut Page {
    let boxed_pages = alloc::vec![Page::new(); num_of_pages].into_boxed_slice();
    Box::into_raw(boxed_pages) as *mut Page
}