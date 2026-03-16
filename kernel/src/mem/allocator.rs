use core::alloc::Layout;

use spin::Mutex;
use talc::{OomHandler, Span, Talc, Talck};

use crate::environment::PAGE_SIZE;

const HEAP_EXPAND_PAGES: usize = 1024;

struct DynamicHeapHandler;

impl OomHandler for DynamicHeapHandler {
    fn handle_oom(talc: &mut Talc<Self>, layout: Layout) -> Result<(), ()> {
        let required_size = layout.size().max(HEAP_EXPAND_PAGES * PAGE_SIZE);
        let pages_needed = (required_size + PAGE_SIZE - 1) / PAGE_SIZE;

        let addr = crate::mem::pmm::alloc_contiguous_pages(pages_needed);
        match addr {
            Some(start_paddr) => {
                let size = pages_needed * PAGE_SIZE;
                let start = crate::vm::phys_to_virt(start_paddr);
                let span = Span::from_base_size(start as *mut u8, size);
                match unsafe { talc.claim(span) } {
                    Ok(_) => Ok(()),
                    Err(_) => {
                        crate::mem::pmm::free_contiguous_pages(start_paddr, pages_needed);
                        Err(())
                    }
                }
            }
            None => Err(()),
        }
    }
}

#[global_allocator]
#[unsafe(link_section = ".data")]
static ALLOCATOR: Talck<Mutex<()>, DynamicHeapHandler> = Talc::new(DynamicHeapHandler).lock();

/// Initialize heap with the given memory region
///
/// # Safety
/// The memory region [start, start + size) must be valid and not used elsewhere
pub unsafe fn init_heap(start: usize, size: usize) {
    let span = Span::from_base_size(start as *mut u8, size);
    ALLOCATOR.lock().claim(span).unwrap();
}

/// Add an additional heap region
///
/// # Safety
/// The memory region [start, start + size) must be valid and not used elsewhere
pub unsafe fn add_heap_region(start: usize, size: usize) -> Result<(), &'static str> {
    let span = Span::from_base_size(start as *mut u8, size);
    ALLOCATOR
        .lock()
        .claim(span)
        .map(|_| ())
        .map_err(|_| "Failed to claim region")
}

pub fn heap_stats() -> (usize, usize) {
    (0, 0)
}
