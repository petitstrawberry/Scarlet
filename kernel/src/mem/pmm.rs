use alloc::vec::Vec;
use spin::Mutex;

use crate::early_println;
use crate::environment::PAGE_SIZE;
use crate::vm::vmem::MemoryArea;

const MAX_ORDER: usize = 22;
const MAX_REGIONS: usize = 16;

struct ListHead {
    next: *mut ListHead,
    prev: *mut ListHead,
}

unsafe impl Send for ListHead {}

impl ListHead {
    const fn new() -> Self {
        Self {
            next: core::ptr::null_mut(),
            prev: core::ptr::null_mut(),
        }
    }

    unsafe fn init(&mut self) {
        self.next = self;
        self.prev = self;
    }

    fn is_empty(&self) -> bool {
        self.next as *const ListHead == self as *const ListHead
    }

    unsafe fn add(&mut self, new: *mut ListHead) {
        unsafe {
            (*new).next = self.next;
            (*new).prev = self as *mut ListHead;
            (*self.next).prev = new;
        }
        self.next = new;
    }

    unsafe fn remove(&mut self, entry: *mut ListHead) {
        unsafe {
            (*(*entry).next).prev = (*entry).prev;
            (*(*entry).prev).next = (*entry).next;
            (*entry).next = entry;
            (*entry).prev = entry;
        }
    }
}

const PAGE_FLAG_BUDDY: u8 = 1 << 0;

struct Page {
    lru: ListHead,
    order: u8,
    flags: u8,
}

unsafe impl Send for Page {}

impl Page {
    const fn new() -> Self {
        Self {
            lru: ListHead::new(),
            order: 0,
            flags: 0,
        }
    }
}

struct FreeArea {
    free_list: ListHead,
    nr_free: usize,
}

impl FreeArea {
    const fn new() -> Self {
        Self {
            free_list: ListHead::new(),
            nr_free: 0,
        }
    }
}

struct BuddyRegion {
    mem_start: usize,
    mem_size: usize,
    page_count: usize,
    pages: *mut Page,
    free_area: [FreeArea; MAX_ORDER + 1],
    active: bool,
}

unsafe impl Send for BuddyRegion {}

impl BuddyRegion {
    const fn new() -> Self {
        Self {
            mem_start: 0,
            mem_size: 0,
            page_count: 0,
            pages: core::ptr::null_mut(),
            free_area: [
                FreeArea::new(),
                FreeArea::new(),
                FreeArea::new(),
                FreeArea::new(),
                FreeArea::new(),
                FreeArea::new(),
                FreeArea::new(),
                FreeArea::new(),
                FreeArea::new(),
                FreeArea::new(),
                FreeArea::new(),
                FreeArea::new(),
                FreeArea::new(),
                FreeArea::new(),
                FreeArea::new(),
                FreeArea::new(),
                FreeArea::new(),
                FreeArea::new(),
                FreeArea::new(),
                FreeArea::new(),
                FreeArea::new(),
                FreeArea::new(),
                FreeArea::new(),
            ],
            active: false,
        }
    }

    fn init(&mut self, start: usize, size: usize) {
        self.mem_start = align_up(start, PAGE_SIZE);
        self.mem_size = align_down(size, PAGE_SIZE);
        self.active = false;

        if self.mem_size < PAGE_SIZE {
            return;
        }

        self.page_count = self.mem_size / PAGE_SIZE;

        let pages_size = self.page_count * core::mem::size_of::<Page>();
        let pages_size_aligned = align_up(pages_size, PAGE_SIZE);
        let pages_needed = pages_size_aligned / PAGE_SIZE;

        if pages_needed >= self.page_count {
            return;
        }

        self.pages = self.mem_start as *mut Page;

        unsafe {
            for i in 0..self.page_count {
                let page = self.pages.add(i);
                (*page).lru.init();
                (*page).order = 0;
                (*page).flags = 0;
            }

            for i in 0..=MAX_ORDER {
                self.free_area[i].free_list.init();
                self.free_area[i].nr_free = 0;
            }
        }

        let mut page_idx = pages_needed;
        while page_idx < self.page_count {
            let remaining = self.page_count - page_idx;
            let mut order = 0usize;

            while order < MAX_ORDER {
                let next_order = order + 1;
                let block_pages = 1usize << next_order;

                if page_idx % block_pages != 0 {
                    break;
                }
                if remaining < block_pages {
                    break;
                }
                order = next_order;
            }

            unsafe {
                self.add_to_free_list(page_idx, order);
            }
            page_idx += 1usize << order;
        }

        self.active = true;
    }

    unsafe fn add_to_free_list(&mut self, page_idx: usize, order: usize) {
        let page = self.pages.add(page_idx);
        (*page).order = order as u8;
        (*page).flags |= PAGE_FLAG_BUDDY;

        let free_list = &mut self.free_area[order].free_list as *mut ListHead;
        (*free_list).add(&mut (*page).lru as *mut ListHead);
        self.free_area[order].nr_free += 1;
    }

    unsafe fn del_from_free_list(&mut self, page: *mut Page, order: usize) {
        let free_list = &mut self.free_area[order].free_list as *mut ListHead;
        (*free_list).remove(&mut (*page).lru as *mut ListHead);
        (*page).flags &= !PAGE_FLAG_BUDDY;
        self.free_area[order].nr_free -= 1;
    }

    fn find_buddy_pfn(&self, page_idx: usize, order: usize) -> usize {
        page_idx ^ (1usize << order)
    }

    fn page_to_pfn(&self, page: *const Page) -> usize {
        unsafe { page.offset_from(self.pages) as usize }
    }

    fn pfn_to_page(&self, pfn: usize) -> *mut Page {
        unsafe { self.pages.add(pfn) }
    }

    fn pfn_to_addr(&self, pfn: usize) -> usize {
        self.mem_start + pfn * PAGE_SIZE
    }

    fn addr_to_pfn(&self, addr: usize) -> usize {
        (addr - self.mem_start) / PAGE_SIZE
    }

    unsafe fn page_is_buddy(&self, page: *const Page, order: usize) -> bool {
        (*page).order == order as u8 && ((*page).flags & PAGE_FLAG_BUDDY) != 0
    }

    fn alloc(&mut self, pages: usize) -> Option<usize> {
        if pages == 0 || !self.active {
            return None;
        }

        let mut order = 0;
        while (1usize << order) < pages && order < MAX_ORDER {
            order += 1;
        }

        if (1usize << order) < pages {
            return None;
        }

        self.alloc_from_order(order)
    }

    fn alloc_from_order(&mut self, order: usize) -> Option<usize> {
        if order > MAX_ORDER || !self.active {
            return None;
        }

        let mut current_order = order;
        while current_order <= MAX_ORDER && self.free_area[current_order].free_list.is_empty() {
            current_order += 1;
        }

        if current_order > MAX_ORDER {
            return None;
        }

        unsafe {
            let free_list = &self.free_area[current_order].free_list;
            let page = free_list.next as *mut Page;

            if page as usize == 0
                || page == free_list as *const ListHead as *mut ListHead as *mut Page
            {
                return None;
            }

            let page_idx = self.page_to_pfn(page);

            self.del_from_free_list(page, current_order);
            (*page).order = 0;

            while current_order > order {
                current_order -= 1;
                let buddy_idx = page_idx + (1usize << current_order);
                let buddy = self.pfn_to_page(buddy_idx);
                (*buddy).order = current_order as u8;
                self.add_to_free_list(buddy_idx, current_order);
            }

            Some(self.pfn_to_addr(page_idx))
        }
    }

    fn free(&mut self, paddr: usize, pages: usize) {
        if !self.active || paddr < self.mem_start {
            return;
        }

        let mut order = 0;
        while (1usize << order) < pages && order < MAX_ORDER {
            order += 1;
        }

        if (1usize << order) < pages {
            return;
        }

        let mut page_idx = self.addr_to_pfn(paddr);
        if page_idx >= self.page_count {
            return;
        }

        unsafe {
            let mut page = self.pfn_to_page(page_idx);
            let mut current_order = order;

            while current_order < MAX_ORDER {
                let buddy_idx = self.find_buddy_pfn(page_idx, current_order);

                if buddy_idx >= self.page_count {
                    break;
                }

                let buddy = self.pfn_to_page(buddy_idx);

                if !self.page_is_buddy(buddy, current_order) {
                    break;
                }

                self.del_from_free_list(buddy, current_order);

                page_idx = page_idx.min(buddy_idx);
                page = self.pfn_to_page(page_idx);
                current_order += 1;
            }

            (*page).order = current_order as u8;
            self.add_to_free_list(page_idx, current_order);
        }
    }

    fn contains(&self, paddr: usize) -> bool {
        self.active && paddr >= self.mem_start && paddr < self.mem_start + self.mem_size
    }

    fn free_pages(&self) -> usize {
        let mut count = 0;
        for order in 0..=MAX_ORDER {
            count += self.free_area[order].nr_free * (1usize << order);
        }
        count
    }

    fn total_pages(&self) -> usize {
        self.page_count
    }
}

struct PmmInner {
    regions: [BuddyRegion; MAX_REGIONS],
}

impl PmmInner {
    const fn new() -> Self {
        Self {
            regions: [
                BuddyRegion::new(),
                BuddyRegion::new(),
                BuddyRegion::new(),
                BuddyRegion::new(),
                BuddyRegion::new(),
                BuddyRegion::new(),
                BuddyRegion::new(),
                BuddyRegion::new(),
                BuddyRegion::new(),
                BuddyRegion::new(),
                BuddyRegion::new(),
                BuddyRegion::new(),
                BuddyRegion::new(),
                BuddyRegion::new(),
                BuddyRegion::new(),
                BuddyRegion::new(),
            ],
        }
    }

    fn add_region(&mut self, start: usize, size: usize) -> Result<(), &'static str> {
        for region in &mut self.regions {
            if !region.active {
                region.init(start, size);
                if region.active {
                    return Ok(());
                }
            }
        }
        Err("Maximum number of PMM regions reached or region too small")
    }

    fn alloc(&mut self, pages: usize) -> Option<usize> {
        for region in &mut self.regions {
            if region.active {
                if let Some(addr) = region.alloc(pages) {
                    return Some(addr);
                }
            }
        }
        None
    }

    fn alloc_from_order(&mut self, order: usize) -> Option<usize> {
        for region in &mut self.regions {
            if region.active {
                if let Some(addr) = region.alloc_from_order(order) {
                    return Some(addr);
                }
            }
        }
        None
    }

    fn free(&mut self, paddr: usize, pages: usize) {
        for region in &mut self.regions {
            if region.contains(paddr) {
                region.free(paddr, pages);
                return;
            }
        }
    }

    fn stats(&self) -> (usize, usize) {
        let mut total = 0;
        let mut free = 0;
        for region in &self.regions {
            if region.active {
                total += region.total_pages();
                free += region.free_pages();
            }
        }
        (total, free)
    }
}

static PMM: Mutex<PmmInner> = Mutex::new(PmmInner::new());

pub unsafe fn init(area: MemoryArea) {
    early_println!(
        "[PMM] Initializing buddy system with region: {:#x} - {:#x}",
        area.start,
        area.end
    );

    let start = align_up(area.start, PAGE_SIZE);
    let size = align_down(area.end + 1 - start, PAGE_SIZE);

    if size == 0 {
        early_println!("[PMM] Region too small, skipping");
        return;
    }

    if let Err(e) = PMM.lock().add_region(start, size) {
        early_println!("[PMM] Failed to add region: {}", e);
        return;
    }

    let (total_pages, free_pages) = PMM.lock().stats();
    early_println!(
        "[PMM] Buddy system initialized: {} pages ({} MB) available",
        free_pages,
        free_pages * PAGE_SIZE / 1024 / 1024
    );
    let _ = total_pages;
}

pub fn add_region(area: MemoryArea) -> Result<(), &'static str> {
    let start = align_up(area.start, PAGE_SIZE);
    let size = align_down(area.end + 1 - start, PAGE_SIZE);

    if size == 0 {
        return Err("Region too small");
    }

    PMM.lock().add_region(start, size)
}

/// Allocate contiguous physical pages.
/// Required for DMA, kernel stacks, and other hardware-visible buffers.
pub fn alloc_contiguous_pages(pages: usize) -> Option<usize> {
    PMM.lock().alloc(pages)
}

/// Allocate aligned contiguous physical pages.
pub fn alloc_contiguous_pages_aligned(pages: usize, align_pages: usize) -> Option<usize> {
    if align_pages == 0 || align_pages == 1 {
        return alloc_contiguous_pages(pages);
    }

    let order = if align_pages.is_power_of_two() {
        align_pages.trailing_zeros() as usize
    } else {
        align_pages.next_power_of_two().trailing_zeros() as usize
    };

    let needed_order = order.max(pages.next_power_of_two().trailing_zeros() as usize);

    PMM.lock().alloc_from_order(needed_order)
}

/// Allocate individual pages (may be non-contiguous).
/// Suitable for task memory where physical contiguity is not required.
pub fn alloc_individual_pages(count: usize) -> Option<Vec<usize>> {
    let mut pages = Vec::with_capacity(count);
    for _ in 0..count {
        match PMM.lock().alloc(1) {
            Some(paddr) => pages.push(paddr),
            None => {
                // Cleanup on failure
                for paddr in pages {
                    PMM.lock().free(paddr, 1);
                }
                return None;
            }
        }
    }
    Some(pages)
}

/// Free contiguous pages.
pub fn free_contiguous_pages(paddr: usize, pages: usize) {
    PMM.lock().free(paddr, pages);
}

/// Free individual pages.
pub fn free_individual_pages(pages: &[usize]) {
    for &paddr in pages {
        PMM.lock().free(paddr, 1);
    }
}

pub fn alloc_frame() -> Option<usize> {
    alloc_contiguous_pages(1)
}

pub fn free_frame(paddr: usize) {
    free_contiguous_pages(paddr, 1);
}

pub fn stats() -> (usize, usize) {
    PMM.lock().stats()
}

fn align_up(addr: usize, align: usize) -> usize {
    (addr + align - 1) & !(align - 1)
}

fn align_down(addr: usize, align: usize) -> usize {
    addr & !(align - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_alloc_free_single_page() {
        // Test basic PMM operations using already initialized PMM
        // PMM is initialized during kernel boot with actual memory

        // Test single page allocation
        let frame = alloc_frame();
        assert!(frame.is_some());
        let frame = frame.unwrap();
        // Verify it's a valid physical address (not null and page aligned)
        assert!(frame > 0);
        assert_eq!(frame % PAGE_SIZE, 0);

        // Test multi-page allocation
        let addr = alloc_contiguous_pages(4);
        assert!(addr.is_some());
        let addr = addr.unwrap();
        assert_eq!(addr % PAGE_SIZE, 0);

        // Test stats - should report available memory
        let (total, free) = stats();
        assert!(total > 0);
        assert!(free > 0);

        // Free allocations
        free_frame(frame);
        free_contiguous_pages(addr, 4);
    }
}
