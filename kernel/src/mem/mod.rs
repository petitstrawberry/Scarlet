//! Memory management module.
//!
//! This module provides functionality for memory allocation, stack management,
//! and other memory-related operations needed by the kernel.

pub mod address;
pub mod allocator;
pub mod page;
pub mod page_cache;
pub mod pmm;

use crate::environment::{MAX_NUM_CPUS, STACK_SIZE};

/// Page-aligned backing storage for all bootstrap CPU stacks.
#[repr(C, align(4096))]
pub struct Stack {
    pub data: [u32; (STACK_SIZE / 4) * MAX_NUM_CPUS],
}

impl Stack {
    /// Return the first kernel virtual address of the complete stack array.
    ///
    /// # Returns
    ///
    /// The inclusive start address, not an individual CPU's initial stack pointer.
    pub fn start(&self) -> usize {
        self.data.as_ptr() as usize
    }

    /// Return the address immediately after the complete stack array.
    ///
    /// # Returns
    ///
    /// The exclusive end address, unlike the inclusive end used by `MemoryArea`.
    pub fn end(&self) -> usize {
        self.start() + self.size()
    }

    /// Return the total backing-storage size for all bootstrap CPU stacks.
    ///
    /// # Returns
    ///
    /// `STACK_SIZE * MAX_NUM_CPUS` bytes, not the size of one CPU's stack.
    pub fn size(&self) -> usize {
        STACK_SIZE * MAX_NUM_CPUS
    }
}

#[unsafe(no_mangle)]
pub static mut KERNEL_STACK: Stack = Stack {
    data: [0xdeadbeef; STACK_SIZE / 4 * MAX_NUM_CPUS],
};

/// Zero the linker-defined BSS during early boot.
///
/// # Returns
///
/// No value. Overwrites the complete half-open range `__BSS_START..__BSS_END`.
///
/// # Boot ordering
///
/// This is an early-entry operation, not a runtime reset API. The boot path must
/// arrange writable mappings and call it before BSS-backed state is initialized
/// or accessed concurrently; calling it later can corrupt live kernel state.
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
