use core::alloc::GlobalAlloc;
use core::mem;
use core::sync::atomic::{AtomicUsize, Ordering};

use slab_allocator_rs::LockedHeap;
#[cfg(debug_assertions)]
use spin::Mutex;

use crate::early_println;
use crate::vm::vmem::MemoryArea;

#[global_allocator]
static mut ALLOCATOR: Allocator = Allocator::new();

#[cfg(debug_assertions)]
mod debug_tracking {
    use super::*;

    // Best-effort leak tracker (fixed-size, non-allocating).
    // This is intentionally simple and safe to call from GlobalAlloc.
    pub const LEAK_TABLE_CAPACITY: usize = 8192; // power-of-two
    pub const LEAK_TABLE_TOMBSTONE: usize = 1;

    // Tracking every alloc/dealloc is expensive (lock per call). We focus on
    // "big" allocations since those dominate leak growth.
    pub const LEAK_TRACK_MIN_BYTES: usize = 64 * 1024; // 64KiB

    pub static LEAK_TABLES: Mutex<LeakTables> = Mutex::new(LeakTables::new());

    pub struct LeakTables {
        active: LeakTable,
        scratch: LeakTable,
        overflows: usize,
        collisions: usize,
        unknown_deallocs: usize,
        mismatched_deallocs: usize,
        rehashes: usize,
    }

    struct LeakTable {
        keys: [usize; LEAK_TABLE_CAPACITY],
        sizes: [usize; LEAK_TABLE_CAPACITY],
        len: usize,
        tombstones: usize,
    }

    #[derive(Copy, Clone)]
    pub struct LeakSnapshot {
        pub tracked: usize,
        pub overflows: usize,
        pub collisions: usize,
        pub unknown_deallocs: usize,
        pub mismatched_deallocs: usize,
        pub tombstones: usize,
        pub rehashes: usize,
        pub top_ptrs: [usize; 4],
        pub top_sizes: [usize; 4],
    }

    impl LeakTable {
        pub const fn new() -> Self {
            Self {
                keys: [0; LEAK_TABLE_CAPACITY],
                sizes: [0; LEAK_TABLE_CAPACITY],
                len: 0,
                tombstones: 0,
            }
        }

        fn clear(&mut self) {
            for i in 0..LEAK_TABLE_CAPACITY {
                self.keys[i] = 0;
                self.sizes[i] = 0;
            }
            self.len = 0;
            self.tombstones = 0;
        }

        #[inline]
        fn hash(ptr: usize) -> usize {
            // Mix address bits; capacity is power-of-two so we can mask.
            // Shift removes low alignment zeros to improve distribution.
            let x = ptr >> 4;
            x.wrapping_mul(0x9E37_79B9_7F4A_7C15usize)
        }

        #[inline]
        fn index(ptr: usize) -> usize {
            Self::hash(ptr) & (LEAK_TABLE_CAPACITY - 1)
        }

        fn insert(&mut self, ptr: usize, size: usize) -> (bool, usize) {
            if ptr == 0 || ptr == LEAK_TABLE_TOMBSTONE {
                return (true, 0);
            }

            let mut idx = Self::index(ptr);
            let mut first_tombstone: Option<usize> = None;
            let mut probes: usize = 0;

            for _ in 0..LEAK_TABLE_CAPACITY {
                let key = self.keys[idx];
                if key == 0 {
                    let insert_idx = first_tombstone.unwrap_or(idx);
                    if first_tombstone.is_some() {
                        self.tombstones = self.tombstones.saturating_sub(1);
                    }
                    self.keys[insert_idx] = ptr;
                    self.sizes[insert_idx] = size;
                    self.len = self.len.saturating_add(1);
                    return (true, probes);
                }

                if key == LEAK_TABLE_TOMBSTONE {
                    if first_tombstone.is_none() {
                        first_tombstone = Some(idx);
                    }
                } else if key == ptr {
                    // Duplicate pointer (shouldn't normally happen) – treat as update.
                    self.sizes[idx] = size;
                    return (true, probes);
                }

                idx = (idx + 1) & (LEAK_TABLE_CAPACITY - 1);
                probes = probes.saturating_add(1);
            }

            (false, probes)
        }

        fn remove(&mut self, ptr: usize) -> (bool, usize, usize) {
            if ptr == 0 || ptr == LEAK_TABLE_TOMBSTONE {
                return (true, 0, 0);
            }

            let mut idx = Self::index(ptr);
            let mut probes: usize = 0;

            for _ in 0..LEAK_TABLE_CAPACITY {
                let key = self.keys[idx];
                if key == 0 {
                    return (false, 0, probes);
                }
                if key == ptr {
                    let old = self.sizes[idx];
                    self.keys[idx] = LEAK_TABLE_TOMBSTONE;
                    self.sizes[idx] = 0;
                    self.len = self.len.saturating_sub(1);
                    self.tombstones = self.tombstones.saturating_add(1);
                    return (true, old, probes);
                }
                idx = (idx + 1) & (LEAK_TABLE_CAPACITY - 1);
                probes = probes.saturating_add(1);
            }

            (false, 0, probes)
        }

        fn snapshot(&self) -> LeakSnapshot {
            let mut top_ptrs = [0usize; 4];
            let mut top_sizes = [0usize; 4];

            for i in 0..LEAK_TABLE_CAPACITY {
                let key = self.keys[i];
                if key == 0 || key == LEAK_TABLE_TOMBSTONE {
                    continue;
                }
                let sz = self.sizes[i];

                // Insert into top-4 (descending by size).
                for j in 0..4 {
                    if sz > top_sizes[j] {
                        for k in (j + 1..4).rev() {
                            top_sizes[k] = top_sizes[k - 1];
                            top_ptrs[k] = top_ptrs[k - 1];
                        }
                        top_sizes[j] = sz;
                        top_ptrs[j] = key;
                        break;
                    }
                }
            }

            // Note: global stats (overflows/unknown/etc) live in LeakTables.
            LeakSnapshot {
                tracked: self.len,
                overflows: 0,
                collisions: 0,
                unknown_deallocs: 0,
                mismatched_deallocs: 0,
                tombstones: self.tombstones,
                rehashes: 0,
                top_ptrs,
                top_sizes,
            }
        }
    }

    impl LeakTables {
        pub const fn new() -> Self {
            Self {
                active: LeakTable::new(),
                scratch: LeakTable::new(),
                overflows: 0,
                collisions: 0,
                unknown_deallocs: 0,
                mismatched_deallocs: 0,
                rehashes: 0,
            }
        }

        #[inline]
        fn should_rehash(&self) -> bool {
            // If tombstones accumulate, the table degrades and probes explode.
            self.active.tombstones > (LEAK_TABLE_CAPACITY / 2)
                || (self.active.len + self.active.tombstones) > (LEAK_TABLE_CAPACITY * 3 / 4)
        }

        fn rehash(&mut self) {
            self.scratch.clear();

            for i in 0..LEAK_TABLE_CAPACITY {
                let key = self.active.keys[i];
                if key == 0 || key == LEAK_TABLE_TOMBSTONE {
                    continue;
                }
                let size = self.active.sizes[i];
                let (ok, _probes) = self.scratch.insert(key, size);
                if !ok {
                    // Should be extremely rare unless capacity is too small.
                    self.overflows = self.overflows.saturating_add(1);
                }
            }

            mem::swap(&mut self.active, &mut self.scratch);
            self.rehashes = self.rehashes.saturating_add(1);
        }

        pub fn track_alloc(&mut self, ptr: usize, size: usize) {
            if self.should_rehash() {
                self.rehash();
            }

            let (ok, probes) = self.active.insert(ptr, size);
            self.collisions = self.collisions.saturating_add(probes);

            if !ok {
                self.overflows = self.overflows.saturating_add(1);
                self.rehash();
                let (ok2, probes2) = self.active.insert(ptr, size);
                self.collisions = self.collisions.saturating_add(probes2);
                if !ok2 {
                    self.overflows = self.overflows.saturating_add(1);
                }
            }
        }

        pub fn track_dealloc(&mut self, ptr: usize, layout_size: usize) {
            if self.should_rehash() {
                self.rehash();
            }

            let (found, old_size, probes) = self.active.remove(ptr);
            self.collisions = self.collisions.saturating_add(probes);

            if !found {
                self.unknown_deallocs = self.unknown_deallocs.saturating_add(1);
                return;
            }
            if old_size != 0 && old_size != layout_size {
                self.mismatched_deallocs = self.mismatched_deallocs.saturating_add(1);
            }
        }

        pub fn snapshot(&self) -> LeakSnapshot {
            let mut snap = self.active.snapshot();
            snap.overflows = self.overflows;
            snap.collisions = self.collisions;
            snap.unknown_deallocs = self.unknown_deallocs;
            snap.mismatched_deallocs = self.mismatched_deallocs;
            snap.rehashes = self.rehashes;
            snap
        }
    }
}

#[cfg(debug_assertions)]
use debug_tracking::{LEAK_TABLES, LEAK_TRACK_MIN_BYTES, LeakSnapshot};

#[cfg(debug_assertions)]
struct Allocator {
    inner: Option<LockedHeap>,
    heap_total_bytes: AtomicUsize,

    /// Live allocation count/bytes (best-effort; relies on correct dealloc layout).
    live_alloc_count: AtomicUsize,
    live_bytes: AtomicUsize,
    peak_live_bytes: AtomicUsize,

    alloc_calls: AtomicUsize,
    dealloc_calls: AtomicUsize,
}

#[cfg(not(debug_assertions))]
struct Allocator {
    inner: Option<LockedHeap>,
    heap_total_bytes: AtomicUsize,
}

unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: core::alloc::Layout) -> *mut u8 {
        if let Some(ref inner) = self.inner {
            let ptr = unsafe { inner.alloc(layout) };

            #[cfg(debug_assertions)]
            {
                self.alloc_calls.fetch_add(1, Ordering::Relaxed);
                if ptr.is_null() {
                    self.maybe_log_usage();
                    return ptr;
                }

                if layout.size() >= LEAK_TRACK_MIN_BYTES {
                    let mut tables = LEAK_TABLES.lock();
                    tables.track_alloc(ptr as usize, layout.size());
                }

                self.live_alloc_count.fetch_add(1, Ordering::Relaxed);

                let new_live =
                    self.live_bytes.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
                let _ =
                    self.peak_live_bytes
                        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |cur| {
                            if new_live > cur { Some(new_live) } else { None }
                        });

                self.maybe_log_usage();
            }

            ptr
        } else {
            panic!("Allocator not initialized, cannot allocate memory.");
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: core::alloc::Layout) {
        if let Some(ref inner) = self.inner {
            #[cfg(debug_assertions)]
            {
                if !ptr.is_null() && layout.size() >= LEAK_TRACK_MIN_BYTES {
                    let mut tables = LEAK_TABLES.lock();
                    tables.track_dealloc(ptr as usize, layout.size());
                }
            }

            unsafe { inner.dealloc(ptr, layout) }

            #[cfg(debug_assertions)]
            {
                self.dealloc_calls.fetch_add(1, Ordering::Relaxed);

                let _ = self
                    .live_alloc_count
                    .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |cur| {
                        Some(cur.saturating_sub(1))
                    });
                let _ = self
                    .live_bytes
                    .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |cur| {
                        Some(cur.saturating_sub(layout.size()))
                    });
            }
        } else {
            panic!("Allocator not initialized, cannot deallocate memory.");
        }
    }
}

impl Allocator {
    pub const fn new() -> Self {
        #[cfg(debug_assertions)]
        {
            return Allocator {
                inner: None,
                heap_total_bytes: AtomicUsize::new(0),
                live_alloc_count: AtomicUsize::new(0),
                live_bytes: AtomicUsize::new(0),
                peak_live_bytes: AtomicUsize::new(0),
                alloc_calls: AtomicUsize::new(0),
                dealloc_calls: AtomicUsize::new(0),
            };
        }

        #[cfg(not(debug_assertions))]
        {
            return Allocator {
                inner: None,
                heap_total_bytes: AtomicUsize::new(0),
            };
        }
    }

    #[inline]
    #[cfg(debug_assertions)]
    fn maybe_log_usage(&self) {
        // Log allocator usage periodically. Keep this cheap and non-allocating.
        // NOTE: This runs inside GlobalAlloc::alloc, so do NOT call regular println.
        // early_println is slow; keep this relatively infrequent.
        const LOG_EVERY_ALLOC_CALLS: usize = 1 << 18; // 262144
        const DUMP_LEAKS_EVERY_ALLOC_CALLS: usize = 1 << 22; // 4194304

        let calls = self.alloc_calls.load(Ordering::Relaxed);
        if LOG_EVERY_ALLOC_CALLS == 0 {
            return;
        }
        if (calls & (LOG_EVERY_ALLOC_CALLS - 1)) != 0 {
            return;
        }

        let heap_total = self.heap_total_bytes.load(Ordering::Relaxed);
        let live = self.live_bytes.load(Ordering::Relaxed);
        let peak = self.peak_live_bytes.load(Ordering::Relaxed);
        let live_allocs = self.live_alloc_count.load(Ordering::Relaxed);
        let dealloc_calls = self.dealloc_calls.load(Ordering::Relaxed);

        let dump_leaks = (calls & (DUMP_LEAKS_EVERY_ALLOC_CALLS - 1)) == 0;
        let leak = if dump_leaks {
            let tables = LEAK_TABLES.lock();
            tables.snapshot()
        } else {
            LeakSnapshot {
                tracked: 0,
                overflows: 0,
                collisions: 0,
                unknown_deallocs: 0,
                mismatched_deallocs: 0,
                tombstones: 0,
                rehashes: 0,
                top_ptrs: [0; 4],
                top_sizes: [0; 4],
            }
        };

        if heap_total == 0 {
            early_println!(
                "[alloc] calls={} live={}B peak={}B live_allocs={} dealloc_calls={} (heap not initialized)",
                calls,
                live,
                peak,
                live_allocs,
                dealloc_calls
            );
            return;
        }

        let used_pct = (live.saturating_mul(100)).saturating_div(heap_total);
        early_println!(
            "[alloc] calls={} live={}KiB peak={}KiB heap={}KiB used={}%% live_allocs={} dealloc_calls={} tracked={} tomb={} ovf={} unk_free={} mismatch_free={} rehash={} coll={}",
            calls,
            live / 1024,
            peak / 1024,
            heap_total / 1024,
            used_pct,
            live_allocs,
            dealloc_calls,
            leak.tracked,
            leak.tombstones,
            leak.overflows,
            leak.unknown_deallocs,
            leak.mismatched_deallocs,
            leak.rehashes,
            leak.collisions
        );

        if dump_leaks {
            early_println!(
                "[alloc] top_big_leaks(>={}KiB): 1) {:#x} {}B 2) {:#x} {}B 3) {:#x} {}B 4) {:#x} {}B",
                LEAK_TRACK_MIN_BYTES / 1024,
                leak.top_ptrs[0],
                leak.top_sizes[0],
                leak.top_ptrs[1],
                leak.top_sizes[1],
                leak.top_ptrs[2],
                leak.top_sizes[2],
                leak.top_ptrs[3],
                leak.top_sizes[3]
            );
        }
    }

    pub unsafe fn init(&mut self, start: usize, size: usize) {
        if self.inner.is_some() {
            early_println!("Allocator already initialized.");
            return;
        }

        let heap = unsafe { LockedHeap::new(start, size) };
        self.inner = Some(heap);

        self.heap_total_bytes.store(size, Ordering::Relaxed);
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
