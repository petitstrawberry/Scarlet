use slab_allocator_rs::LockedHeap;

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

unsafe extern "C" {
    static __HEAP_START: usize;
    static __HEAP_END: usize;
}

pub fn init_heap() {
    let heap_start = unsafe { &__HEAP_START as *const usize as usize };
    let heap_end = unsafe { &__HEAP_END as *const usize as usize };

    let heap_size = heap_end - heap_start;
    unsafe {
        ALLOCATOR.init(heap_start, heap_size);
    }
}
