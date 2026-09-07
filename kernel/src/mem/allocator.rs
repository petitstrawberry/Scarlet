use core::alloc::Layout;

use crate::sync::RawIrqSpinLock;
use talc::{OomHandler, Span, Talc, Talck};

use crate::environment::PAGE_SIZE;

const HEAP_EXPAND_PAGES: usize = 1024;

fn minimum_heap_expand_pages(layout: Layout) -> Option<usize> {
    // Each fallback is a separate Talc heap. Account for the heap-base tag and
    // the allocation tag, then leave enough slack for worst-case alignment.
    let tag_slack = core::mem::size_of::<usize>().checked_mul(2)?;
    let required_size = layout
        .size()
        .checked_add(tag_slack)?
        .max(layout.align().checked_mul(2)?);
    let pages = required_size
        .checked_add(PAGE_SIZE - 1)?
        .checked_div(PAGE_SIZE)?;

    // The PMM buddy allocator reserves a power-of-two block even for a
    // non-power-of-two request. Claim the whole block instead of leaking the
    // unreported tail pages.
    pages.checked_next_power_of_two()
}

fn next_heap_expand_pages(current_pages: usize, minimum_pages: usize) -> Option<usize> {
    if current_pages <= minimum_pages {
        return None;
    }

    Some((current_pages / 2).max(minimum_pages))
}

struct DynamicHeapHandler;

impl OomHandler for DynamicHeapHandler {
    fn handle_oom(talc: &mut Talc<Self>, layout: Layout) -> Result<(), ()> {
        let minimum_pages = minimum_heap_expand_pages(layout).ok_or(())?;
        let mut pages_needed = HEAP_EXPAND_PAGES.max(minimum_pages);

        loop {
            let size = pages_needed.checked_mul(PAGE_SIZE).ok_or(())?;
            if let Some(start_paddr) = crate::mem::pmm::alloc_contiguous_pages(pages_needed) {
                let start = crate::vm::phys_to_virt(start_paddr);
                let span = Span::from_base_size(start as *mut u8, size);
                // SAFETY: PMM returned this contiguous physical allocation and
                // `phys_to_virt` maps the whole range into the kernel address space.
                return match unsafe { talc.claim(span) } {
                    Ok(_) => Ok(()),
                    Err(_) => {
                        crate::mem::pmm::free_contiguous_pages(start_paddr, pages_needed);
                        Err(())
                    }
                };
            }

            let Some(next_pages) = next_heap_expand_pages(pages_needed, minimum_pages) else {
                return Err(());
            };
            pages_needed = next_pages;
        }
    }
}

#[global_allocator]
#[unsafe(link_section = ".data")]
static ALLOCATOR: Talck<RawIrqSpinLock, DynamicHeapHandler> = Talc::new(DynamicHeapHandler).lock();

/// Initialize heap with the given memory region
///
/// # Safety
/// The memory region [start, start + size) must be valid and not used elsewhere
pub unsafe fn init_heap(start: usize, size: usize) {
    let span = Span::from_base_size(start as *mut u8, size);
    // SAFETY: The caller guarantees that this range is valid, mapped, and not
    // already owned by another allocator region.
    unsafe { ALLOCATOR.lock().claim(span) }.unwrap();
}

/// Add an additional heap region
///
/// # Safety
/// The memory region [start, start + size) must be valid and not used elsewhere
pub unsafe fn add_heap_region(start: usize, size: usize) -> Result<(), &'static str> {
    let span = Span::from_base_size(start as *mut u8, size);
    // SAFETY: The caller guarantees that this range is valid, mapped, and not
    // already owned by another allocator region.
    unsafe { ALLOCATOR.lock().claim(span) }
        .map(|_| ())
        .map_err(|_| "Failed to claim region")
}

pub fn heap_stats() -> (usize, usize) {
    (0, 0)
}

#[cfg(test)]
mod tests {
    use super::{HEAP_EXPAND_PAGES, PAGE_SIZE, minimum_heap_expand_pages, next_heap_expand_pages};
    use core::alloc::Layout;

    #[test_case]
    fn heap_expansion_falls_back_below_the_preferred_chunk() {
        let layout = Layout::from_size_align(24, 8).expect("test layout should be valid");
        let minimum_pages = minimum_heap_expand_pages(layout).unwrap();
        assert_eq!(minimum_pages, 1);

        let mut pages = HEAP_EXPAND_PAGES;
        for expected in [512, 256, 128, 64, 32, 16, 8, 4, 2, 1] {
            pages = next_heap_expand_pages(pages, minimum_pages).unwrap();
            assert_eq!(pages, expected);
        }
        assert_eq!(next_heap_expand_pages(pages, minimum_pages), None);
    }

    #[test_case]
    fn heap_expansion_keeps_enough_space_for_alignment() {
        let layout = Layout::from_size_align(1, 8192).expect("test layout should be valid");
        let minimum_pages = minimum_heap_expand_pages(layout).unwrap();

        assert_eq!(minimum_pages, 4);
        assert_eq!(next_heap_expand_pages(8, minimum_pages), Some(4));
        assert_eq!(next_heap_expand_pages(4, minimum_pages), None);
    }

    #[test_case]
    fn heap_expansion_accounts_for_talc_tags_at_a_page_boundary() {
        let layout = Layout::from_size_align(PAGE_SIZE, 8).expect("test layout should be valid");

        assert_eq!(minimum_heap_expand_pages(layout), Some(2));
    }

    #[test_case]
    fn heap_expansion_matches_the_pmm_buddy_granularity() {
        let layout =
            Layout::from_size_align(5 * PAGE_SIZE, 8).expect("test layout should be valid");

        assert_eq!(minimum_heap_expand_pages(layout), Some(8));
    }

    #[test_case]
    fn heap_expansion_does_not_shrink_below_a_large_request() {
        let layout =
            Layout::from_size_align(6 * 1024 * 1024, 16).expect("test layout should be valid");
        let minimum_pages = minimum_heap_expand_pages(layout).unwrap();

        assert!(minimum_pages > HEAP_EXPAND_PAGES);
        assert_eq!(next_heap_expand_pages(minimum_pages, minimum_pages), None);
    }
}
