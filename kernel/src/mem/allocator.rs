use core::alloc::GlobalAlloc;
use core::sync::atomic::{AtomicUsize, Ordering};

use slab_allocator_rs::LockedHeap;

use crate::early_println;
use crate::vm::vmem::MemoryArea;

#[global_allocator]
// Keep the allocator state out of .bss to avoid relying on late-bss pages being
// accessible on all accelerators (e.g. HVF). The value is still initialized to
// the same zero state.
#[unsafe(link_section = ".data")]
static ALLOCATOR: Allocator = Allocator::new();

struct Allocator {
    // inner: Option<Talck<spin::Mutex<()>, ClaimOnOom>>,
    inner: spin::Once<LockedHeap>,
    allocated_count: AtomicUsize,
    allocated_bytes: AtomicUsize,
}

unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: core::alloc::Layout) -> *mut u8 {
        if let Some(inner) = self.inner.get() {
            // early_println!("Allocating {} bytes with alignment {}", layout.size(), layout.align());
            let ptr = unsafe { inner.alloc(layout) };
            // early_println!("Allocated {} bytes at {:?}", layout.size(), ptr);
            self.allocated_count.fetch_add(1, Ordering::SeqCst);
            self.allocated_bytes
                .fetch_add(layout.size(), Ordering::SeqCst);
            // early_println!("Total allocations: {}, Total bytes allocated: {}", self.allocated_count.load(Ordering::SeqCst), self.allocated_bytes.load(Ordering::SeqCst));
            return ptr;
        }
        panic!("Allocator not initialized, cannot allocate memory.");
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: core::alloc::Layout) {
        if let Some(inner) = self.inner.get() {
            unsafe { inner.dealloc(ptr, layout) }
            // early_println!("Deallocated {} bytes at {:?}", layout.size(), ptr);
            self.allocated_count.fetch_sub(1, Ordering::SeqCst);
            self.allocated_bytes
                .fetch_sub(layout.size(), Ordering::SeqCst);
            return;
        }
        panic!("Allocator not initialized, cannot deallocate memory.");
    }
}

impl Allocator {
    pub const fn new() -> Self {
        Allocator {
            inner: spin::Once::new(),
            allocated_count: AtomicUsize::new(0),
            allocated_bytes: AtomicUsize::new(0),
        }
    }

    pub unsafe fn init(&self, start: usize, size: usize) {
        let _ = self
            .inner
            .call_once(|| unsafe { LockedHeap::new(start, size) });
    }
}

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
