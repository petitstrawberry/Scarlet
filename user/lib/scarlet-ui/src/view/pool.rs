//! Buffer pool for efficient buffer reuse
//!
//! This module provides BufferPool, which manages a pool of ViewBuffers
//! for efficient reuse. Buffers are allocated on demand and recycled when
//! no longer needed.

extern crate alloc;
use alloc::vec::Vec;

use crate::layout::Size;
use crate::view::buffer::ViewBuffer;
use crate::view::id::ViewId;
use scarlet_std::collections::HashMap;
use scarlet_std::fmt;

/// Pool of view buffers for efficient allocation and reuse
///
/// BufferPool manages a set of buffers that can be reused across views.
/// This reduces allocation overhead and memory fragmentation.
pub struct BufferPool {
    /// Available buffers (not currently in use)
    available: Vec<ViewBuffer>,
    /// Buffers currently in use, indexed by ViewId
    in_use: HashMap<ViewId, ViewBuffer>,
    /// Maximum number of buffers to keep in the pool
    max_pool_size: usize,
    /// Total memory used by all buffers (in bytes)
    total_memory: usize,
}

impl BufferPool {
    /// Create a new buffer pool
    pub fn new() -> Self {
        Self {
            available: Vec::new(),
            in_use: HashMap::new(),
            max_pool_size: 10,
            total_memory: 0,
        }
    }

    /// Create a new buffer pool with a specific max pool size
    pub fn with_max_size(max_pool_size: usize) -> Self {
        Self {
            available: Vec::new(),
            in_use: HashMap::new(),
            max_pool_size,
            total_memory: 0,
        }
    }

    /// Acquire a buffer for the given view
    ///
    /// This will either reuse an available buffer or allocate a new one.
    pub fn acquire(&mut self, view_id: ViewId, size: Size) -> Option<ViewBuffer> {
        // First, try to find a reusable buffer that fits
        let buffer = self.find_reusable_buffer(size)
            .unwrap_or_else(|| ViewBuffer::new(size));

        self.total_memory += buffer.memory_usage();
        self.in_use.insert(view_id, buffer.clone());
        Some(buffer)
    }

    /// Release a buffer back to the pool
    ///
    /// The buffer will be returned to the available pool if there's space,
    /// otherwise it will be dropped.
    pub fn release(&mut self, view_id: ViewId) {
        if let Some(mut buffer) = self.in_use.remove(&view_id) {
            self.total_memory -= buffer.memory_usage();

            // Clear the buffer before returning to pool
            buffer.clear();

            // Return to pool if there's space
            if self.available.len() < self.max_pool_size {
                self.available.push(buffer);
            }
            // Otherwise, the buffer is dropped
        }
    }

    /// Get a buffer that's currently in use
    pub fn get_buffer(&self, view_id: ViewId) -> Option<&ViewBuffer> {
        self.in_use.get(&view_id)
    }

    /// Get a mutable reference to a buffer that's currently in use
    pub fn get_buffer_mut(&mut self, view_id: ViewId) -> Option<&mut ViewBuffer> {
        self.in_use.get_mut(&view_id)
    }

    /// Resize a buffer that's in use
    ///
    /// Returns true if the buffer was resized (grew), false otherwise.
    pub fn resize_buffer(&mut self, view_id: ViewId, new_size: Size) -> bool {
        if let Some(buffer) = self.in_use.get_mut(&view_id) {
            let _old_size = buffer.size();
            let old_memory = buffer.memory_usage();

            if buffer.resize(new_size) {
                // Buffer was resized, update memory tracking
                self.total_memory -= old_memory;
                self.total_memory += buffer.memory_usage();
                return true;
            }
        }
        false
    }

    /// Find a reusable buffer from the available pool
    fn find_reusable_buffer(&mut self, size: Size) -> Option<ViewBuffer> {
        // Find the smallest buffer that fits
        let best_index = self.available.iter().enumerate()
            .filter(|(_, b)| b.can_fit(size))
            .min_by_key(|(_, b)| b.memory_usage())
            .map(|(i, _)| i);

        if let Some(index) = best_index {
            let buffer = self.available.remove(index);
            return Some(buffer);
        }

        None
    }

    /// Get the number of available buffers
    pub fn available_count(&self) -> usize {
        self.available.len()
    }

    /// Get the number of buffers in use
    pub fn in_use_count(&self) -> usize {
        self.in_use.len()
    }

    /// Get the total memory usage in bytes
    pub fn total_memory(&self) -> usize {
        self.total_memory
    }

    /// Clear all available buffers
    pub fn clear_available(&mut self) {
        self.available.clear();
    }

    /// Clear all buffers (both available and in use)
    ///
    /// This is useful for cleanup or when resetting the entire UI.
    pub fn clear_all(&mut self) {
        self.available.clear();
        self.in_use.clear();
        self.total_memory = 0;
    }

    /// Prune the available pool to remove excess buffers
    ///
    /// Returns the number of buffers removed.
    pub fn prune(&mut self) -> usize {
        let initial_count = self.available.len();
        if self.available.len() > self.max_pool_size {
            self.available.truncate(self.max_pool_size);
        }
        initial_count - self.available.len()
    }
}

impl Default for BufferPool {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for BufferPool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BufferPool")
            .field("available_count", &self.available.len())
            .field("in_use_count", &self.in_use.len())
            .field("max_pool_size", &self.max_pool_size)
            .field("total_memory", &self.total_memory)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_buffer_pool_new() {
        let pool = BufferPool::new();
        assert_eq!(pool.available_count(), 0);
        assert_eq!(pool.in_use_count(), 0);
        assert_eq!(pool.total_memory(), 0);
    }

    #[test]
    fn test_buffer_pool_acquire_release() {
        let mut pool = BufferPool::new();
        let view_id = ViewId::new();
        let size = Size::new(100, 100);

        // Acquire a buffer
        pool.acquire(view_id, size);
        assert_eq!(pool.in_use_count(), 1);
        assert_eq!(pool.total_memory(), 100 * 100 * 4);

        // Release the buffer
        pool.release(view_id);
        assert_eq!(pool.in_use_count(), 0);
        assert_eq!(pool.available_count(), 1);
        assert_eq!(pool.total_memory(), 0); // Memory is tracked only for in-use
    }

    #[test]
    fn test_buffer_pool_reuse() {
        let mut pool = BufferPool::new();
        let view_id1 = ViewId::new();
        let view_id2 = ViewId::new();
        let size = Size::new(100, 100);

        // Acquire and release a buffer
        pool.acquire(view_id1, size);
        pool.release(view_id1);

        // Acquire another buffer with the same size
        // Should reuse the previously released buffer
        pool.acquire(view_id2, size);

        // Available pool should be empty (buffer was reused)
        assert_eq!(pool.available_count(), 0);
        assert_eq!(pool.in_use_count(), 1);
    }

    #[test]
    fn test_buffer_pool_resize() {
        let mut pool = BufferPool::new();
        let view_id = ViewId::new();

        pool.acquire(view_id, Size::new(100, 100));

        // Resize to larger size
        let resized = pool.resize_buffer(view_id, Size::new(200, 200));
        assert!(resized);

        let buffer = pool.get_buffer(view_id).unwrap();
        assert_eq!(buffer.size(), Size::new(200, 200));
    }

    #[test]
    fn test_buffer_pool_resize_shrink() {
        let mut pool = BufferPool::new();
        let view_id = ViewId::new();

        pool.acquire(view_id, Size::new(100, 100));

        // Resize to smaller size (should not shrink due to grow-only)
        let resized = pool.resize_buffer(view_id, Size::new(50, 50));
        assert!(!resized);

        let buffer = pool.get_buffer(view_id).unwrap();
        assert_eq!(buffer.size(), Size::new(100, 100)); // Size unchanged
    }

    #[test]
    fn test_buffer_pool_prune() {
        let mut pool = BufferPool::with_max_size(2);

        // Release more buffers than max_pool_size
        for _ in 0..5 {
            let view_id = ViewId::new();
            pool.acquire(view_id, Size::new(100, 100));
            pool.release(view_id);
        }

        assert_eq!(pool.available_count(), 5);

        // Prune should remove excess buffers
        let removed = pool.prune();
        assert_eq!(removed, 3);
        assert_eq!(pool.available_count(), 2);
    }

    #[test]
    fn test_buffer_pool_clear_all() {
        let mut pool = BufferPool::new();

        let view_id = ViewId::new();
        pool.acquire(view_id, Size::new(100, 100));

        pool.clear_all();

        assert_eq!(pool.available_count(), 0);
        assert_eq!(pool.in_use_count(), 0);
        assert_eq!(pool.total_memory(), 0);
    }
}
