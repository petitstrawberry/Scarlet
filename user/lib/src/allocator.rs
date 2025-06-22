use crate::syscall;
use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::ptr;
use core::sync::atomic::{AtomicUsize, Ordering};

// Global allocator instance
#[global_allocator]
pub static ALLOCATOR: BumpAllocator = BumpAllocator::new();

/// Simple bump allocator
/// Allocates memory using brk and sbrk system calls
pub struct BumpAllocator {
    heap_start: AtomicUsize,
    heap_end: AtomicUsize,
    next: UnsafeCell<usize>,
    allocations: AtomicUsize,
}

unsafe impl Sync for BumpAllocator {}

impl BumpAllocator {
    pub const fn new() -> Self {
        BumpAllocator {
            heap_start: AtomicUsize::new(0),
            heap_end: AtomicUsize::new(0),
            next: UnsafeCell::new(0),
            allocations: AtomicUsize::new(0),
        }
    }

    /// Initialization process
    /// 
    /// # Safety
    /// This method must be called only once.
    pub unsafe fn init(&self) {
        // Allocate initial heap area (starting with 4KB)
        let initial_size = 4096;
        let start = sbrk(initial_size);
        if start == usize::MAX {
            panic!("Failed to initialize heap");
        }

        self.heap_start.store(start, Ordering::SeqCst);
        self.heap_end.store(start + initial_size, Ordering::SeqCst);
        unsafe { *self.next.get() = start };
    }

    /// Extend the heap
    fn extend_heap(&self, additional: usize) -> bool {
        let aligned_size = (additional + 15) & !15; // 16-byte alignment
        let prev_end = self.heap_end.load(Ordering::SeqCst);
        
        // Extend the heap using sbrk
        let new_end = sbrk(aligned_size);
        if new_end == usize::MAX {
            return false;
        }

        self.heap_end.store(prev_end + aligned_size, Ordering::SeqCst);
        true
    }
}

unsafe impl GlobalAlloc for BumpAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let size = layout.size();
        let align = layout.align();

        // Initialize if not already initialized
        if self.heap_start.load(Ordering::SeqCst) == 0 {
            unsafe { self.init() };
        }

        // Get the current position
        let current = unsafe { *self.next.get() };
        
        // Adjust alignment
        let aligned = (current + align - 1) & !(align - 1);
        
        // If there is not enough memory, extend the heap
        if aligned + size > self.heap_end.load(Ordering::SeqCst) {
            let needed = aligned + size - self.heap_end.load(Ordering::SeqCst);
            if !self.extend_heap(needed) {
                return ptr::null_mut();
            }
        }

        // Update the next pointer
        unsafe { *self.next.get() = aligned + size };
        
        // Increment the allocation count
        self.allocations.fetch_add(1, Ordering::SeqCst);
        
        aligned as *mut u8
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {
        // This simple allocator does not free memory
        // Only decrement the allocation count
        if self.allocations.fetch_sub(1, Ordering::SeqCst) == 1 {
            // If all allocations are freed, reset the heap to the initial position
            unsafe { self.init(); }
        }
    }
}

pub fn brk(addr: usize) -> usize {
    syscall::syscall1(syscall::Syscall::Brk, addr)
}

pub fn sbrk(size: usize) -> usize {
    syscall::syscall1(syscall::Syscall::Sbrk, size)
}
