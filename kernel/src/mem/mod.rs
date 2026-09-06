//! Memory management module.
//!
//! This module provides functionality for memory allocation, stack management,
//! and other memory-related operations needed by the kernel.

pub mod allocator;
pub mod page;
pub mod page_cache;
pub mod pmm;

use alloc::{boxed::Box, vec};

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

/// Allocates a block of memory of the specified size from the kernel heap.
///
/// The block is zero-initialized and aligned for bytes only. This is not a PMM
/// allocator and does not promise page alignment or physical contiguity. Prefer
/// an owning `Box` or `Vec` when a raw allocation is unnecessary.
///
/// # Arguments
///
/// * `size` - The size of the memory block to allocate.
///
/// # Returns
///
/// A kernel virtual pointer owned by the caller, to be released once with
/// [`kfree`] using the exact original size. For zero bytes the pointer is
/// non-null but must not be dereferenced. Allocation failure follows the global
/// allocation-error path rather than returning null.
///
pub fn kmalloc(size: usize) -> *mut u8 {
    Box::into_raw(vec![0u8; size].into_boxed_slice()) as *mut u8
}

/// Frees a block of memory previously allocated with `kmalloc`.
///
/// # Arguments
///
/// * `ptr` - The original live pointer returned by [`kmalloc`], including for zero bytes.
/// * `size` - The exact original byte count passed to [`kmalloc`].
///
/// # Returns
///
/// No value. Releases the allocation to the kernel heap.
///
/// # Caller requirements
///
/// The caller must exclusively own the allocation and end all accesses before
/// freeing it. Null, interior, already freed, and PMM pointers are not accepted.
/// This legacy safe signature does not enforce those requirements: invalid inputs
/// can cause undefined behavior. Documentation is not a substitute for an unsafe
/// or ownership-enforcing API; new code should retain an owning `Box` or `Vec`.
///
pub fn kfree(ptr: *mut u8, size: usize) {
    unsafe {
        let _ = Box::<[u8]>::from_raw(core::slice::from_raw_parts_mut(ptr, size));
    }
}

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
