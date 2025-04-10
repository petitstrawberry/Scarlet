use slab_allocator_rs::LockedHeap;

use crate::early_println;
use crate::early_print;
use crate::vm::vmem::MemoryArea;

#[global_allocator]
pub static ALLOCATOR: LockedHeap = LockedHeap::empty();

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
