use crate::syscall;
use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicUsize, Ordering};

/// Free-list memory allocator
#[global_allocator]
pub static ALLOCATOR: FreeListAllocator = FreeListAllocator::new();

#[repr(C)]
struct FreeBlock {
    size: usize,
    next: *mut FreeBlock,
}

pub struct FreeListAllocator {
    head: UnsafeCell<*mut FreeBlock>,
    heap_start: AtomicUsize,
    heap_end: AtomicUsize,
}

unsafe impl Sync for FreeListAllocator {}

impl FreeListAllocator {
    pub const fn new() -> Self {
        FreeListAllocator {
            head: UnsafeCell::new(core::ptr::null_mut()),
            heap_start: AtomicUsize::new(0),
            heap_end: AtomicUsize::new(0),
        }
    }

    unsafe fn init(&self) {
        let initial_size = 4096;
        let start = sbrk(initial_size);
        if start == usize::MAX {
            panic!("Failed to initialize heap");
        }
        self.heap_start.store(start, Ordering::SeqCst);
        self.heap_end.store(start + initial_size, Ordering::SeqCst);
        unsafe { *self.head.get() = core::ptr::null_mut(); }
    }

    fn extend_heap(&self, size: usize) -> *mut u8 {
        let aligned_size = (size + 15) & !15;
        let new_block_addr = sbrk(aligned_size);
        if new_block_addr == usize::MAX {
            return core::ptr::null_mut();
        }
        self.heap_end.fetch_add(aligned_size, Ordering::SeqCst);
        // Add as a new free block to the list
        unsafe {
            let block = new_block_addr as *mut FreeBlock;
            (*block).size = aligned_size;
            (*block).next = *self.head.get();
            *self.head.get() = block;
            block as *mut u8
        }
    }

    unsafe fn find_fit(&self, size: usize, align: usize) -> (*mut FreeBlock, *mut FreeBlock) {
        let mut prev: *mut FreeBlock = core::ptr::null_mut();
        let mut curr = unsafe { *self.head.get() };
        while !curr.is_null() {
            let addr = curr as usize;
            let aligned_addr = (addr + core::cmp::max(align, core::mem::align_of::<FreeBlock>()) - 1) & !(core::cmp::max(align, core::mem::align_of::<FreeBlock>()) - 1);
            let offset = aligned_addr - addr;
            if unsafe { (*curr).size } >= size + offset {
                return (prev, curr);
            }
            prev = curr;
            curr = unsafe { (*curr).next };
        }
        (core::ptr::null_mut(), core::ptr::null_mut())
    }
}

unsafe impl GlobalAlloc for FreeListAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let size = layout.size().max(core::mem::size_of::<FreeBlock>());
        let align = layout.align();
        if self.heap_start.load(Ordering::SeqCst) == 0 {
            unsafe { self.init(); }
        }
        // Find a fitting free block
        let (mut prev, mut curr) = unsafe { self.find_fit(size, align) };
        if curr.is_null() {
            // Extend and try again
            self.extend_heap(size.max(4096));
            let (p, c) = unsafe { self.find_fit(size, align) };
            prev = p;
            curr = c;
            if curr.is_null() {
                return core::ptr::null_mut();
            }
        }
        // Alignment adjustment
        let min_align = core::cmp::max(align, core::mem::align_of::<FreeBlock>());
        let addr = curr as usize;
        let aligned_addr = (addr + min_align - 1) & !(min_align - 1);
        let offset = aligned_addr - addr;
        let total_size = size + offset;
        // Split block if possible
        if unsafe { (*curr).size } > total_size + core::mem::size_of::<FreeBlock>() {
            let next_block_addr = aligned_addr + size;
            // Align next_block_addr as well
            let next_block_addr = (next_block_addr + min_align - 1) & !(min_align - 1);
            let next_block = next_block_addr as *mut FreeBlock;
            unsafe {
                (*next_block).size = (*curr).size - (next_block_addr - addr);
                (*next_block).next = (*curr).next;
                if prev.is_null() {
                    *self.head.get() = next_block;
                } else {
                    (*prev).next = next_block;
                }
            }
        } else {
            // Exact fit or too small to split
            unsafe {
                if prev.is_null() {
                    *self.head.get() = (*curr).next;
                } else {
                    (*prev).next = (*curr).next;
                }
            }
        }
        aligned_addr as *mut u8
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let size = layout.size().max(core::mem::size_of::<FreeBlock>());
        let align = layout.align();
        let min_align = core::cmp::max(align, core::mem::align_of::<FreeBlock>());
        let addr = ptr as usize;
        // Reverse alignment adjustment to find the original block start address
        let block_start_addr = addr & !(min_align - 1);
        let block = block_start_addr as *mut FreeBlock;
        unsafe {
            (*block).size = size;
            // Return to the head of the free list
            (*block).next = *self.head.get();
            *self.head.get() = block;
        }
    }
}

pub fn brk(addr: usize) -> usize {
    syscall::syscall1(syscall::Syscall::Brk, addr)
}

pub fn sbrk(size: usize) -> usize {
    syscall::syscall1(syscall::Syscall::Sbrk, size)
}
