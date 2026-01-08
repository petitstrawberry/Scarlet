use crate::syscall;
use crate::sync::Mutex;
#[cfg(any(debug_assertions, feature = "alloc_debug"))]
use crate::println;
use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicUsize, Ordering};

#[cfg(any(debug_assertions, feature = "alloc_debug"))]
const LARGE_ALLOC_THRESHOLD: usize = 1024 * 1024; // 1 MiB

const ALLOC_HEADER_MAGIC: usize = 0x5343_4152_4c45_5401; // "SCARLET" + 1

#[cfg(any(debug_assertions, feature = "alloc_debug"))]
#[derive(Clone, Copy)]
struct AllocRecord {
    start: usize,
    size: usize,
    payload: usize,
    active: bool,
}

#[cfg(any(debug_assertions, feature = "alloc_debug"))]
const EMPTY_RECORD: AllocRecord = AllocRecord {
    start: 0,
    size: 0,
    payload: 0,
    active: false,
};

#[cfg(any(debug_assertions, feature = "alloc_debug"))]
struct AllocLog {
    next: usize,
    records: [AllocRecord; 256],
    large: [AllocRecord; 8],
    table: AllocTable,
}

#[cfg(any(debug_assertions, feature = "alloc_debug"))]
impl AllocLog {
    pub const fn new() -> Self {
        Self {
            next: 0,
            records: [EMPTY_RECORD; 256],
            large: [EMPTY_RECORD; 8],
            table: AllocTable::new(),
        }
    }
}

/// Free-list memory allocator
#[global_allocator]
pub static ALLOCATOR: LockedFreeListAllocator = LockedFreeListAllocator::new();

#[repr(C)]
struct FreeBlock {
    size: usize,
    next: *mut FreeBlock,
}

#[repr(C)]
struct AllocHeader {
    magic: usize,
    /// Total bytes reserved from the underlying region (including header/padding).
    size: usize,
    /// Start address of the reserved region.
    start: usize,
}

#[cfg(any(debug_assertions, feature = "alloc_debug"))]
const ALLOC_TABLE_SIZE: usize = 4096;

#[cfg(any(debug_assertions, feature = "alloc_debug"))]
#[derive(Clone, Copy)]
struct AllocSlot {
    payload: usize,
    start: usize,
    size: usize,
    state: u8, // 0=empty, 1=filled, 2=tombstone
}

#[cfg(any(debug_assertions, feature = "alloc_debug"))]
const EMPTY_SLOT: AllocSlot = AllocSlot {
    payload: 0,
    start: 0,
    size: 0,
    state: 0,
};

#[cfg(any(debug_assertions, feature = "alloc_debug"))]
struct AllocTable {
    slots: [AllocSlot; ALLOC_TABLE_SIZE],
}

#[cfg(any(debug_assertions, feature = "alloc_debug"))]
impl AllocTable {
    pub const fn new() -> Self {
        Self {
            slots: [EMPTY_SLOT; ALLOC_TABLE_SIZE],
        }
    }

    #[inline]
    fn hash(payload: usize) -> usize {
        // Payloads are typically aligned; shift to mix, then multiply.
        let x = payload >> 4;
        x.wrapping_mul(0x9e37_79b9_7f4a_7c15) % ALLOC_TABLE_SIZE
    }

    fn insert(&mut self, payload: usize, start: usize, size: usize) {
        let mut idx = Self::hash(payload);
        let mut first_tomb: Option<usize> = None;

        for _ in 0..ALLOC_TABLE_SIZE {
            let slot = &mut self.slots[idx];
            match slot.state {
                0 => {
                    let use_idx = first_tomb.unwrap_or(idx);
                    self.slots[use_idx] = AllocSlot {
                        payload,
                        start,
                        size,
                        state: 1,
                    };
                    return;
                }
                1 => {
                    if slot.payload == payload {
                        println!(
                            "[ALLOC] ERROR: double-alloc of payload=0x{:x} (old start=0x{:x} size=0x{:x}, new start=0x{:x} size=0x{:x})",
                            payload, slot.start, slot.size, start, size
                        );
                        panic!("allocator tracking table: duplicate payload");
                    }
                }
                2 => {
                    if first_tomb.is_none() {
                        first_tomb = Some(idx);
                    }
                }
                _ => {
                    panic!("allocator tracking table: bad slot state");
                }
            }
            idx = (idx + 1) % ALLOC_TABLE_SIZE;
        }

        if let Some(use_idx) = first_tomb {
            self.slots[use_idx] = AllocSlot {
                payload,
                start,
                size,
                state: 1,
            };
            return;
        }

        println!(
            "[ALLOC] ERROR: allocation table full (payload=0x{:x} start=0x{:x} size=0x{:x})",
            payload, start, size
        );
        panic!("allocator tracking table overflow");
    }

    fn remove(&mut self, payload: usize) -> Option<AllocSlot> {
        let mut idx = Self::hash(payload);
        for _ in 0..ALLOC_TABLE_SIZE {
            let slot = &mut self.slots[idx];
            match slot.state {
                0 => return None,
                1 => {
                    if slot.payload == payload {
                        let found = *slot;
                        slot.state = 2;
                        // Keep payload/start/size for debugging; treated as tombstone.
                        return Some(found);
                    }
                }
                2 => {}
                _ => {
                    panic!("allocator tracking table: bad slot state");
                }
            }
            idx = (idx + 1) % ALLOC_TABLE_SIZE;
        }
        None
    }
}

#[inline]
const fn align_up(value: usize, align: usize) -> usize {
    // align must be power-of-two.
    (value + align - 1) & !(align - 1)
}

pub struct FreeListAllocator {
    head: UnsafeCell<*mut FreeBlock>,
    heap_start: AtomicUsize,
    heap_end: AtomicUsize,
}

/// Locked wrapper for FreeListAllocator.
///
/// The underlying allocator is not thread-safe; all access must be serialized.
pub struct LockedFreeListAllocator {
    lock: Mutex<()>,
    inner: FreeListAllocator,
    #[cfg(any(debug_assertions, feature = "alloc_debug"))]
    log: UnsafeCell<AllocLog>,
}

unsafe impl Sync for LockedFreeListAllocator {}

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
        unsafe {
            *self.head.get() = core::ptr::null_mut();
        }
        self.insert_free_block(start, initial_size);
    }

    fn extend_heap(&self, size: usize) -> *mut u8 {
        let aligned_size = align_up(size, core::mem::align_of::<FreeBlock>());
        let new_block_addr = sbrk(aligned_size);
        if new_block_addr == usize::MAX {
            return core::ptr::null_mut();
        }
        self.heap_end.fetch_add(aligned_size, Ordering::SeqCst);
        self.insert_free_block(new_block_addr, aligned_size);
        new_block_addr as *mut u8
    }

    fn insert_free_block(&self, addr: usize, size: usize) {
        // Push-front insertion.
        #[cfg(any(debug_assertions, feature = "alloc_debug"))]
        {
            assert_eq!(
                addr % core::mem::align_of::<FreeBlock>(),
                0,
                "free block address must be aligned"
            );
            assert!(
                size >= core::mem::size_of::<FreeBlock>(),
                "free block size too small"
            );
            assert_eq!(
                size % core::mem::align_of::<FreeBlock>(),
                0,
                "free block size must be aligned"
            );
        }
        unsafe {
            let block = addr as *mut FreeBlock;
            (*block).size = size;
            (*block).next = *self.head.get();
            *self.head.get() = block;
        }
    }

    unsafe fn find_fit(&self, layout: Layout) -> (*mut FreeBlock, *mut FreeBlock, usize, usize) {
        // Returns (prev, curr, header_addr, alloc_size)
        let header_size = core::mem::size_of::<AllocHeader>();
        let header_align = core::mem::align_of::<AllocHeader>();
        let payload_align = core::cmp::max(layout.align(), header_align);
        let block_align = core::mem::align_of::<FreeBlock>();

        let mut prev: *mut FreeBlock = core::ptr::null_mut();
        let mut curr = unsafe { *self.head.get() };
        while !curr.is_null() {
            #[cfg(any(debug_assertions, feature = "alloc_debug"))]
            {
                assert_eq!(
                    (curr as usize) % block_align,
                    0,
                    "free list corrupted: node misaligned"
                );
            }
            let block_addr = curr as usize;
            let block_size = unsafe { (*curr).size };

            // Place header immediately before the payload, but ensure header alignment
            // by aligning payload to at least header_align.
            let payload = align_up(block_addr + header_size, payload_align);
            let header = payload - header_size;
            let alloc_end = payload.saturating_add(layout.size());
            // IMPORTANT:
            // `reserved` must be aligned so that the remainder block header (`FreeBlock`)
            // is written at an aligned address. If not, we corrupt the free list and can
            // end up double-allocating live regions (observed as stack/backbuffer overlap).
            let needed_raw = alloc_end.saturating_sub(block_addr);
            let needed = align_up(needed_raw, block_align);

            if needed <= block_size {
                return (prev, curr, header, needed);
            }

            prev = curr;
            curr = unsafe { (*curr).next };
        }
        (core::ptr::null_mut(), core::ptr::null_mut(), 0, 0)
    }
}

impl LockedFreeListAllocator {
    pub const fn new() -> Self {
        Self {
            lock: Mutex::new(()),
            inner: FreeListAllocator::new(),
            #[cfg(any(debug_assertions, feature = "alloc_debug"))]
            log: UnsafeCell::new(AllocLog::new()),
        }
    }

    #[cfg(any(debug_assertions, feature = "alloc_debug"))]
    fn debug_record_alloc(&self, payload: usize, start: usize, size: usize, layout: Layout) {
        let _ = layout;
        let log = unsafe { &mut *self.log.get() };

        let new_lo = start;
        let new_hi = start.saturating_add(size);

        // Always check against the stable allocation table (not subject to ring overwrite).
        for slot in log.table.slots.iter() {
            if slot.state != 1 {
                continue;
            }
            let lo = slot.start;
            let hi = slot.start.saturating_add(slot.size);
            let overlaps = !(new_hi <= lo || hi <= new_lo);
            if overlaps {
                println!(
                    "[ALLOC] OVERLAP: new(start=0x{:x} size=0x{:x} payload=0x{:x}) overlaps old(start=0x{:x} size=0x{:x} payload=0x{:x})",
                    start, size, payload, slot.start, slot.size, slot.payload
                );
                panic!("allocator overlap detected");
            }
        }

        let idx = log.next % log.records.len();
        log.next = log.next.wrapping_add(1);
        log.records[idx] = AllocRecord {
            start,
            size,
            payload,
            active: true,
        };

        // Insert into stable table.
        log.table.insert(payload, start, size);

        // Pin large allocations so they cannot be overwritten by the ring.
        if size >= LARGE_ALLOC_THRESHOLD {
            // Find an empty slot; otherwise replace the oldest active slot.
            if let Some(slot) = log.large.iter_mut().find(|r| !r.active) {
                *slot = AllocRecord {
                    start,
                    size,
                    payload,
                    active: true,
                };
            } else {
                let replace_idx = log.next % log.large.len();
                log.large[replace_idx] = AllocRecord {
                    start,
                    size,
                    payload,
                    active: true,
                };
            }
        }
    }

    #[cfg(any(debug_assertions, feature = "alloc_debug"))]
    fn debug_record_dealloc(&self, payload: usize, start: usize, size: usize) {
        // Basic sanity: payload should fall within reserved region.
        if start == 0 || size == 0 {
            println!(
                "[ALLOC] bad dealloc header: payload=0x{:x} start=0x{:x} size=0x{:x}",
                payload, start, size
            );
            panic!("allocator header corrupted");
        }
        if payload < start || payload >= start.saturating_add(size) {
            println!(
                "[ALLOC] dealloc out of range: payload=0x{:x} start=0x{:x} size=0x{:x}",
                payload, start, size
            );
            panic!("allocator header corrupted");
        }

        let log = unsafe { &mut *self.log.get() };

        // Remove from stable table first.
        match log.table.remove(payload) {
            Some(slot) => {
                if slot.start != start || slot.size != size {
                    println!(
                        "[ALLOC] ERROR: header/table mismatch for payload=0x{:x}: header(start=0x{:x} size=0x{:x}) table(start=0x{:x} size=0x{:x})",
                        payload, start, size, slot.start, slot.size
                    );
                    panic!("allocator header corrupted");
                }
            }
            None => {
                println!(
                    "[ALLOC] ERROR: dealloc of unknown payload=0x{:x} start=0x{:x} size=0x{:x}",
                    payload, start, size
                );
                panic!("allocator dealloc of unknown pointer (double free/corruption)");
            }
        }

        for rec in log.records.iter_mut() {
            if rec.active && rec.payload == payload {
                rec.active = false;
                return;
            }
        }
        for rec in log.large.iter_mut() {
            if rec.active && rec.payload == payload {
                rec.active = false;
                return;
            }
        }
    }
}

unsafe impl GlobalAlloc for LockedFreeListAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _guard = self.lock.lock();
        let header_size = core::mem::size_of::<AllocHeader>();
        let header_align = core::mem::align_of::<AllocHeader>();
        let payload_align = core::cmp::max(layout.align(), header_align);

        let size = layout.size();
        if size == 0 {
            // Return a non-null, well-aligned dangling pointer.
            return payload_align as *mut u8;
        }
        if self.inner.heap_start.load(Ordering::SeqCst) == 0 {
            unsafe {
                self.inner.init();
            }
        }
        // Find a fitting free block
        let (mut prev, mut curr, _header_addr, mut alloc_size) =
            unsafe { self.inner.find_fit(layout) };
        if curr.is_null() {
            // Extend and try again
            let grow = (size + header_size).max(4096);
            self.inner.extend_heap(grow);
            let (p, c, _h, n) = unsafe { self.inner.find_fit(layout) };
            prev = p;
            curr = c;
            alloc_size = n;
            if curr.is_null() {
                return core::ptr::null_mut();
            }
        }

        let block_addr = curr as usize;
        let block_size = unsafe { (*curr).size };

        let payload = align_up(block_addr + header_size, payload_align);
        let header_addr = payload - header_size;
        let mut reserved = alloc_size;

        // If the remainder is too small to hold a FreeBlock, consume the whole block.
        let remaining = block_size.saturating_sub(reserved);
        if remaining < core::mem::size_of::<FreeBlock>() {
            reserved = block_size;
        }

        // Remove/replace free list node.
        if reserved < block_size {
            let next_addr = block_addr + reserved;
            unsafe {
                let next_block = next_addr as *mut FreeBlock;
                (*next_block).size = block_size - reserved;
                (*next_block).next = (*curr).next;
                if prev.is_null() {
                    *self.inner.head.get() = next_block;
                } else {
                    (*prev).next = next_block;
                }
            }
        } else {
            unsafe {
                if prev.is_null() {
                    *self.inner.head.get() = (*curr).next;
                } else {
                    (*prev).next = (*curr).next;
                }
            }
        }

        // Write header.
        unsafe {
            let hdr = header_addr as *mut AllocHeader;
            core::ptr::write_unaligned(
                hdr,
                AllocHeader {
                    magic: ALLOC_HEADER_MAGIC,
                    size: reserved,
                    start: block_addr,
                },
            );
        }

        #[cfg(any(debug_assertions, feature = "alloc_debug"))]
        self.debug_record_alloc(payload, block_addr, reserved, layout);

        payload as *mut u8
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let _guard = self.lock.lock();
        // For zero-sized allocations, `alloc` may return any non-null dangling pointer.
        // `dealloc` for such layouts must be a no-op.
        if layout.size() == 0 {
            return;
        }
        if ptr.is_null() {
            return;
        }

        let header_size = core::mem::size_of::<AllocHeader>();
        let header_addr = (ptr as usize).saturating_sub(header_size);
        let hdr_ptr = header_addr as *const AllocHeader;

        unsafe {
            let hdr = core::ptr::read_unaligned(hdr_ptr);

            if hdr.magic != ALLOC_HEADER_MAGIC {
                #[cfg(any(debug_assertions, feature = "alloc_debug"))]
                {
                    println!(
                        "[ALLOC] ERROR: bad header magic for ptr=0x{:x} header_addr=0x{:x} magic=0x{:x}",
                        ptr as usize,
                        header_addr,
                        hdr.magic
                    );
                    panic!("allocator header magic mismatch");
                }

                #[cfg(not(any(debug_assertions, feature = "alloc_debug")))]
                {
                    // In non-debug builds, ignore invalid frees to avoid corrupting the heap.
                    return;
                }
            }

            let start = hdr.start;
            let size = hdr.size;
            if start == 0 || size == 0 {
                return;
            }

            #[cfg(any(debug_assertions, feature = "alloc_debug"))]
            self.debug_record_dealloc(ptr as usize, start, size);

            self.inner.insert_free_block(start, size);
        }
    }
}

#[allow(dead_code)]
pub fn brk(addr: usize) -> usize {
    syscall::syscall1(syscall::Syscall::Brk, addr)
}

pub fn sbrk(size: usize) -> usize {
    syscall::syscall1(syscall::Syscall::Sbrk, size)
}
