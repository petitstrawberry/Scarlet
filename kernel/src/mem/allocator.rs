use core::alloc::GlobalAlloc;
use core::sync::atomic::{AtomicUsize, Ordering};

use slab_allocator_rs::LockedHeap;

use crate::early_println;
use crate::vm::vmem::MemoryArea;

#[global_allocator]
static mut ALLOCATOR: Allocator = Allocator::new();

struct Allocator {
  // inner: Option<Talck<spin::Mutex<()>, ClaimOnOom>>,
  inner: Option<LockedHeap>,
  allocated_count: AtomicUsize,
  allocated_bytes: AtomicUsize,
}

unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: core::alloc::Layout) -> *mut u8 {
        if let Some(ref inner) = self.inner {
            // early_println!("Allocating {} bytes with alignment {}", layout.size(), layout.align());
            let ptr = unsafe { inner.alloc(layout) };
            // early_println!("Allocated {} bytes at {:?}", layout.size(), ptr);
            self.allocated_count.fetch_add(1, Ordering::SeqCst);
            self.allocated_bytes.fetch_add(layout.size(), Ordering::SeqCst);
            // early_println!("Total allocations: {}, Total bytes allocated: {}", self.allocated_count.load(Ordering::SeqCst), self.allocated_bytes.load(Ordering::SeqCst));
            ptr
        } else {
          panic!("Allocator not initialized, cannot allocate memory.");
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: core::alloc::Layout) {
        if let Some(ref inner) = self.inner {
            unsafe { inner.dealloc(ptr, layout) }
            // early_println!("Deallocated {} bytes at {:?}", layout.size(), ptr);
            self.allocated_count.fetch_sub(1, Ordering::SeqCst);
            self.allocated_bytes.fetch_sub(layout.size(), Ordering::SeqCst);
            // early_println!("Total allocations: {}, Total bytes allocated: {}", self.allocated_count.load(Ordering::SeqCst), self.allocated_bytes.load(Ordering::SeqCst));
        } else {
            panic!("Allocator not initialized, cannot deallocate memory.");
        }
    }
}

impl Allocator {
    pub const fn new() -> Self {
        Allocator { inner: None, allocated_count: AtomicUsize::new(0), allocated_bytes: AtomicUsize::new(0) }
    }

    pub unsafe fn init(&mut self, start: usize, size: usize) {
        if self.inner.is_some() {
            early_println!("Allocator already initialized.");
            return;
        }

        let heap = unsafe { LockedHeap::new(start, size) };
        self.inner = Some(heap);
    }
}

#[allow(static_mut_refs)]
pub fn init_heap(area: MemoryArea) {
    let size = area.size();
    if size == 0 {
        early_println!("Heap size is zero, skipping initialization.");
        return;
    }

    unsafe {
        ALLOCATOR.init(area.start, size);
    }

    early_println!("Heap initialized: {:#x} - {:#x}", area.start, area.end);
}
