//! Memory management module.
//!
//! This module provides functionality for memory allocation, stack management, 
//! and other memory-related operations needed by the kernel.

pub mod allocator;
pub mod page;

use alloc::{boxed::Box, vec};

use crate::environment::{NUM_OF_CPUS, STACK_SIZE};

#[repr(C, align(4096))]
pub struct Stack {
    pub data: [u32; (STACK_SIZE/4) * NUM_OF_CPUS],
}

impl Stack {
    pub fn start(&self) -> usize {
        self.data.as_ptr() as usize
    }
    
    pub fn end(&self) -> usize {
        self.start() + self.size()
    }

    pub fn size(&self) -> usize {
        STACK_SIZE * NUM_OF_CPUS
    }
}

#[unsafe(no_mangle)]
pub static mut KERNEL_STACK: Stack = Stack { data: [0xdeadbeef; STACK_SIZE/4 * NUM_OF_CPUS] };

/// Allocates a block of memory of the specified size from the kernel heap.
/// 
/// # Arguments
/// 
/// * `size` - The size of the memory block to allocate.
/// 
/// # Returns
/// 
/// * A pointer to the allocated memory block.
/// 
pub fn kmalloc(size: usize) -> *mut u8 {
    Box::into_raw(vec![0u8; size].into_boxed_slice()) as *mut u8
}

/// Frees a block of memory previously allocated with `kmalloc`.
/// 
/// # Arguments
/// 
/// * `ptr` - A pointer to the memory block to free.
/// * `size` - The size of the memory block to free.
/// 
pub fn kfree(ptr: *mut u8, size: usize) {
    unsafe {
        let _ = Box::<[u8]>::from_raw(core::slice::from_raw_parts_mut(ptr, size));
    }
}

pub fn init_bss() {
    unsafe extern "C" {
        static mut __BSS_START: u8;
        static mut __BSS_END: u8;
    }

    unsafe {
        let bss_start = &raw mut __BSS_START as *mut u8;
        let bss_end = &raw mut __BSS_END as *mut u8;
        let bss_size = bss_end as usize - bss_start as usize;
        core::ptr::write_bytes(bss_start, 0, bss_size);
    }
}

unsafe extern "C" {
    pub static __KERNEL_SPACE_START: usize;
    pub static __KERNEL_SPACE_END: usize;
    pub static __FDT_RESERVED_START: usize;
    pub static __FDT_RESERVED_END: usize;
}