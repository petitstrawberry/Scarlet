use buddy_system_allocator::Heap;
use core::alloc::Layout;
use core::ptr::NonNull;
use spin::Mutex;

use crate::early_println;
use crate::environment::PAGE_SIZE;
use crate::vm::vmem::MemoryArea;

const HEAP_ORDER: usize = 33;

static PMM: Mutex<Heap<HEAP_ORDER>> = Mutex::new(Heap::empty());

pub unsafe fn init(area: MemoryArea) {
    early_println!(
        "[PMM] Initializing with region: {:#x} - {:#x}",
        area.start,
        area.end
    );

    let start = area.start;
    let size = area.end - area.start + 1;

    PMM.lock().init(start, size);

    let total_bytes = PMM.lock().stats_total_bytes();
    early_println!(
        "[PMM] Initialized: {} MB available",
        total_bytes / 1024 / 1024
    );
}

pub fn alloc_pages(pages: usize) -> Option<usize> {
    let size = pages * PAGE_SIZE;
    let layout = Layout::from_size_align(size, PAGE_SIZE).ok()?;

    PMM.lock()
        .alloc(layout)
        .ok()
        .map(|ptr| ptr.as_ptr() as usize)
}

pub fn free_pages(paddr: usize, pages: usize) {
    let size = pages * PAGE_SIZE;
    let layout = Layout::from_size_align(size, PAGE_SIZE).unwrap();

    unsafe {
        PMM.lock()
            .dealloc(NonNull::new_unchecked(paddr as *mut u8), layout);
    }
}

pub fn alloc_frame() -> Option<usize> {
    alloc_pages(1)
}

pub fn free_frame(paddr: usize) {
    free_pages(paddr, 1);
}

pub fn stats() -> (usize, usize) {
    let heap = PMM.lock();
    let total = heap.stats_total_bytes();
    let used = heap.stats_alloc_actual();
    (total, used)
}
