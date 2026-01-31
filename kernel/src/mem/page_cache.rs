//! Global Page Cache Manager
//!
//! Provides a unified page cache for all filesystem operations (read/write/mmap).
//! This cache is shared across all file objects pointing to the same file,
//! ensuring consistency and memory efficiency.
//!
//! # Phase 0 Implementation
//!
//! Current implementation includes:
//! - Page allocation and caching with CacheId-based indexing
//! - Pin-based protection against eviction during active use
//! - Dirty page tracking for write operations
//! - Object-level locking for mmap support
//! - Flush operations for writeback
//!
//! Not yet implemented:
//! - Page eviction (LRU or similar policy)
//! - LOADING state coordination for concurrent access
//! - Integration with filesystem read/write operations (Phase 1)
//! - Demand paging for mmap (Phase 2)
//!
//! # Usage
//!
//! ```rust,no_run
//! use crate::mem::page_cache::PageCacheManager;
//!
//! let paddr = PageCacheManager::global().get_or_create_pinned(cache_id, page_index, |paddr| {
//!     // Load page content from disk to paddr
//!     Ok(())
//! })?;
//! // Use the page...
//! PageCacheManager::global().unpin(cache_id, page_index);
//! ```

use alloc::collections::BTreeMap;
use core::sync::atomic::{AtomicUsize, Ordering};
use spin::RwLock;

use crate::fs::vfs_v2::cache::CacheId;
use crate::mem::page::allocate_boxed_pages;

/// Page index within a file (0, 1, 2, ...)
pub type PageIndex = u64;

/// Physical address of a page
pub type PhysicalAddress = usize;

/// Entry in the page cache representing a single cached page
pub struct PageCacheEntry {
    /// Physical address of the cached page
    paddr: PhysicalAddress,
    /// Pin count - number of active short-term accesses
    /// Pages with pin_count > 0 cannot be evicted
    pin_count: AtomicUsize,
    /// Dirty flag - true if page has been modified and needs writeback
    is_dirty: AtomicUsize, // Using AtomicUsize as AtomicBool
}

impl PageCacheEntry {
    /// Create a new page cache entry
    fn new(paddr: PhysicalAddress) -> Self {
        Self {
            paddr,
            pin_count: AtomicUsize::new(0),
            is_dirty: AtomicUsize::new(0),
        }
    }

    /// Get the physical address
    #[inline]
    pub fn paddr(&self) -> PhysicalAddress {
        self.paddr
    }

    /// Increment pin count
    #[inline]
    fn pin(&self) {
        self.pin_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Decrement pin count
    #[inline]
    fn unpin(&self) {
        self.pin_count.fetch_sub(1, Ordering::Relaxed);
    }

    /// Get current pin count
    #[inline]
    fn pin_count(&self) -> usize {
        self.pin_count.load(Ordering::Relaxed)
    }

    /// Mark page as dirty
    #[inline]
    fn mark_dirty(&self) {
        self.is_dirty.store(1, Ordering::Relaxed);
    }

    /// Check if page is dirty
    #[inline]
    fn is_dirty(&self) -> bool {
        self.is_dirty.load(Ordering::Relaxed) != 0
    }
}

/// Global page cache manager
///
/// Manages all cached pages for filesystem operations. Uses (CacheId, PageIndex)
/// as the key to uniquely identify pages across the entire system.
pub struct PageCacheManager {
    /// Map from (CacheId, PageIndex) to cached page entry
    entries: RwLock<BTreeMap<(CacheId, PageIndex), PageCacheEntry>>,
    /// Object-level lock counts for eviction prevention
    /// Maps CacheId to lock count (>0 means object is unevictable)
    object_locks: RwLock<BTreeMap<CacheId, usize>>,
}

impl PageCacheManager {
    /// Create a new empty page cache manager
    pub const fn new() -> Self {
        Self {
            entries: RwLock::new(BTreeMap::new()),
            object_locks: RwLock::new(BTreeMap::new()),
        }
    }

    /// Global singleton accessor
    #[inline]
    pub fn global() -> &'static PageCacheManager {
        &GLOBAL_PAGE_CACHE
    }

    /// Get a page, pinning it to prevent eviction during access
    ///
    /// If the page is not in cache, calls the `loader` callback to load it.
    /// Returns the physical address of the page with pin_count incremented.
    ///
    /// # Arguments
    /// * `id` - Cache identifier (filesystem + file)
    /// * `index` - Page index within the file
    /// * `loader` - Callback to load the page if not cached. Receives the allocated
    ///              physical address and should fill it with page content.
    ///
    /// # Returns
    /// Physical address of the pinned page
    pub fn get_or_create_pinned<F>(
        &self,
        id: CacheId,
        index: PageIndex,
        loader: F,
    ) -> Result<PhysicalAddress, &'static str>
    where
        F: FnOnce(PhysicalAddress) -> Result<(), &'static str>,
    {
        let key = (id, index);

        // Fast path: page already cached (read lock)
        if let Some(entry) = self.entries.read().get(&key) {
            entry.pin();
            return Ok(entry.paddr());
        }

        // Slow path: allocate new page and load content (may race; acceptable)
        let mut boxed_pages = allocate_boxed_pages(1);
        let page_ptr = boxed_pages.as_mut_ptr();
        let paddr = page_ptr as PhysicalAddress;

        // Call loader to fill the page with content
        loader(paddr)?;

        // Acquire write lock and insert if still missing
        let mut map = self.entries.write();
        if let Some(existing) = map.get(&key) {
            // Someone else inserted meanwhile; reuse it and drop our allocation
            existing.pin();
            return Ok(existing.paddr());
        }

        // Create cache entry with pin_count = 1 and insert
        let entry = PageCacheEntry::new(paddr);
        entry.pin();
        map.insert(key, entry);

        // Leak the box to prevent deallocation - we manage it manually now
        core::mem::forget(boxed_pages);

        Ok(paddr)
    }

    /// Try to get a pinned page without triggering I/O
    ///
    /// Returns Some(paddr) if the page is already cached, None otherwise.
    /// If successful, increments pin_count.
    pub fn try_get_pinned(&self, id: CacheId, index: PageIndex) -> Option<PhysicalAddress> {
        let key = (id, index);
        self.entries.read().get(&key).map(|entry| {
            entry.pin();
            entry.paddr()
        })
    }

    /// Unpin a page, allowing it to be evicted
    ///
    /// Decrements the pin count. When pin_count reaches 0, the page
    /// becomes eligible for eviction (if not locked).
    pub fn unpin(&self, id: CacheId, index: PageIndex) {
        if let Some(entry) = self.entries.read().get(&(id, index)) {
            entry.unpin();
        }
    }

    /// Mark a page as dirty (modified)
    ///
    /// Dirty pages will be written back to storage during flush or eviction.
    pub fn mark_dirty(&self, id: CacheId, index: PageIndex) {
        if let Some(entry) = self.entries.read().get(&(id, index)) {
            entry.mark_dirty();
        }
    }

    /// Set object-level lock (prevents eviction of all pages for this object)
    ///
    /// Used during mmap to keep all mapped pages resident.
    /// Phase 0: Simple implementation - lock entire object during mmap.
    pub fn set_object_locked(&self, id: CacheId, locked: bool) {
        if locked {
            *self.object_locks.write().entry(id).or_insert(0) += 1;
        } else if let Some(count) = self.object_locks.write().get_mut(&id) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                self.object_locks.write().remove(&id);
            }
        }
    }

    /// Check if an object is locked (unevictable)
    fn is_object_locked(&self, id: CacheId) -> bool {
        self.object_locks
            .read()
            .get(&id)
            .map_or(false, |&count| count > 0)
    }

    /// Flush dirty pages for a specific cache object
    ///
    /// Writes all dirty pages back to storage using the provided writer callback.
    /// Only flushes pages with pin_count == 0 to avoid writing pages being modified.
    ///
    /// # Arguments
    /// * `id` - Cache identifier
    /// * `writer` - Callback to write a page. Receives (page_index, paddr)
    pub fn flush<F>(&self, id: CacheId, mut writer: F) -> Result<(), &'static str>
    where
        F: FnMut(PageIndex, PhysicalAddress) -> Result<(), &'static str>,
    {
        // Collect targets under read lock to minimize lock contention
        let mut targets: alloc::vec::Vec<(PageIndex, PhysicalAddress)> = alloc::vec::Vec::new();
        {
            let map = self.entries.read();
            for (&(cache_id, page_index), entry) in map.iter() {
                if cache_id == id && entry.is_dirty() && entry.pin_count() == 0 {
                    targets.push((page_index, entry.paddr()));
                }
            }
        }

        // Perform writes without holding the map lock; then clear dirty flags
        for (page_index, paddr) in targets.into_iter() {
            writer(page_index, paddr)?;
            if let Some(entry) = self.entries.read().get(&(id, page_index)) {
                entry.is_dirty.store(0, Ordering::SeqCst);
            }
        }
        Ok(())
    }

    /// Get or load a page and return an RAII guard that unpins on drop.
    #[inline]
    pub fn pin_or_load<F>(
        &self,
        id: CacheId,
        index: PageIndex,
        loader: F,
    ) -> Result<PinnedPage, &'static str>
    where
        F: FnOnce(PhysicalAddress) -> Result<(), &'static str>,
    {
        let paddr = self.get_or_create_pinned(id, index, loader)?;
        Ok(PinnedPage { id, index, paddr })
    }

    /// Try to pin an already cached page and return an RAII guard.
    #[inline]
    pub fn try_pin(&self, id: CacheId, index: PageIndex) -> Option<PinnedPage> {
        self.try_get_pinned(id, index)
            .map(|paddr| PinnedPage { id, index, paddr })
    }

    /// Invalidate (drop) all cached pages belonging to the given CacheId.
    ///
    /// This is used when a file is removed (unlink) so that subsequent
    /// lookups / recreations do not observe stale cached content that
    /// belonged to the old incarnation of the file.
    pub fn invalidate(&self, id: CacheId) {
        // Collect keys first under read lock to minimize time with write lock
        let mut to_remove: alloc::vec::Vec<(CacheId, PageIndex)> = alloc::vec::Vec::new();
        {
            let map = self.entries.read();
            for (&(cache_id, page_index), _entry) in map.iter() {
                if cache_id == id {
                    to_remove.push((cache_id, page_index));
                }
            }
        }
        if to_remove.is_empty() {
            return;
        }
        let mut map = self.entries.write();
        for key in to_remove.into_iter() {
            map.remove(&key);
        }
    }
}

/// RAII guard for a pinned page.
///
/// Unpins the page on Drop. Provides helpers to access the page and mark it dirty.
pub struct PinnedPage {
    id: CacheId,
    index: PageIndex,
    paddr: PhysicalAddress,
}

impl PinnedPage {
    /// Physical address of the pinned page
    #[inline]
    pub fn paddr(&self) -> PhysicalAddress {
        self.paddr
    }

    /// Cache identifier
    #[inline]
    pub fn id(&self) -> CacheId {
        self.id
    }

    /// Page index within the object
    #[inline]
    pub fn index(&self) -> PageIndex {
        self.index
    }

    /// Mark this page dirty
    #[inline]
    pub fn mark_dirty(&self) {
        PageCacheManager::global().mark_dirty(self.id, self.index);
    }
}

impl Drop for PinnedPage {
    fn drop(&mut self) {
        PageCacheManager::global().unpin(self.id, self.index);
    }
}

/// Global page cache instance
///
pub static GLOBAL_PAGE_CACHE: PageCacheManager = PageCacheManager::new();

// Note: Callers should access the cache via `PageCacheManager::global()`.

// TODO (Phase 2): Replace single global structure with sharded structure:
// - Hash (CacheId, PageIndex) -> shard index (e.g. 16 or 32 shards)
// - Each shard: Mutex/PageCacheShard { entries }
// - object_locks separated or distributed
// Instance methods remain the stable API; callers use PageCacheManager::global().
