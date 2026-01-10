//! Global allocator using the talc crate.
//!
//! This module provides a global allocator based on `talc`, a high-performance
//! no_std allocator. It uses a custom OOM handler that extends the heap via
//! the `sbrk` system call when memory runs out.

use crate::syscall;
use core::alloc::Layout;
use talc::{OomHandler, Span, Talc, Talck};

/// Minimum extension size (4 KiB).
const MIN_EXTEND_SIZE: usize = 4096;

/// Custom OOM handler that extends the heap using the sbrk syscall.
pub struct SbrkOomHandler {
    /// Current heap span (base to end).
    heap: Span,
}

impl SbrkOomHandler {
    /// Create a new uninitialized handler.
    pub const fn new() -> Self {
        Self { heap: Span::empty() }
    }
}

impl OomHandler for SbrkOomHandler {
    fn handle_oom(talc: &mut Talc<Self>, layout: Layout) -> Result<(), ()> {
        // Calculate how much memory we need
        let required = layout.size().max(layout.align() * 2) + 64;
        let extend_size = required.max(MIN_EXTEND_SIZE);
        // Round up to alignment of usize for simplicity
        let aligned_size = (extend_size + core::mem::size_of::<usize>() - 1)
            & !(core::mem::size_of::<usize>() - 1);

        // Call sbrk to get more memory
        let result = sbrk(aligned_size);
        if result == usize::MAX {
            // sbrk failed
            return Err(());
        }

        let new_base = result as *mut u8;
        let new_acme = unsafe { new_base.add(aligned_size) };

        // Check if this is the first allocation or if we can extend
        if let Some((_base, acme)) = talc.oom_handler.heap.get_base_acme() {
            // Check if the new memory is contiguous with the existing heap
            if acme == new_base {
                // Extend the existing heap
                let old_heap = talc.oom_handler.heap;
                let new_heap = old_heap.extend(0, aligned_size);
                talc.oom_handler.heap = unsafe { talc.extend(old_heap, new_heap) };
            } else {
                // Non-contiguous: claim as a new heap region
                // This can happen if something else called sbrk
                let new_span = Span::new(new_base, new_acme);
                if let Ok(claimed) = unsafe { talc.claim(new_span) } {
                    // Note: we lose track of the old heap span, but talc still manages it
                    talc.oom_handler.heap = claimed;
                } else {
                    return Err(());
                }
            }
        } else {
            // First allocation: claim the new memory
            let new_span = Span::new(new_base, new_acme);
            match unsafe { talc.claim(new_span) } {
                Ok(claimed) => talc.oom_handler.heap = claimed,
                Err(_) => return Err(()),
            }
        }

        Ok(())
    }
}

/// The global allocator type.
pub type GlobalAllocator = Talck<spin::Mutex<()>, SbrkOomHandler>;

/// The global allocator instance.
///
/// Note: The allocator starts with an empty heap and claims memory on first OOM
/// (which happens on first allocation).
#[global_allocator]
pub static ALLOCATOR: GlobalAllocator = {
    let talc = Talc::new(SbrkOomHandler::new());
    talc.lock::<spin::Mutex<()>>()
};

/// Increase the program break by `size` bytes.
/// Returns the previous break address, or `usize::MAX` on failure.
pub fn sbrk(size: usize) -> usize {
    syscall::syscall1(syscall::Syscall::Sbrk, size)
}

/// Set the program break to `addr`.
/// Returns the new break address, or `usize::MAX` on failure.
#[allow(dead_code)]
pub fn brk(addr: usize) -> usize {
    syscall::syscall1(syscall::Syscall::Brk, addr)
}
