use slab_allocator_rs::LockedHeap;

use crate::early_println;
use crate::early_print;

#[global_allocator]
pub static ALLOCATOR: LockedHeap = LockedHeap::empty();

unsafe extern "C" {
    static __HEAP_START: usize;
}

pub fn init_heap(size: usize) {
    if size == 0 {
        early_println!("Heap size is zero, skipping initialization.");
        return;
    }

    let heap_size = size;
    let heap_start = unsafe { &__HEAP_START as *const usize as usize };
    let heap_end = heap_start + heap_size - 1;

    unsafe {
        ALLOCATOR.init(heap_start, heap_size);
    }

    early_println!("Heap initialized: {:#x} - {:#x}", heap_start, heap_end);
}
