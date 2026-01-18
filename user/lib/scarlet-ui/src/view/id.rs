//! Unique view identifiers
//!
//! ViewId provides unique identifiers for all views in the view tree.
//! Each view instance gets a unique ViewId that can be used for:
//! - O(1) lookup in ViewRegistry
//! - Dirty tracking
//! - Parent-child relationships
//! - Event targeting

use core::sync::atomic::{AtomicU64, Ordering};
use scarlet_std::sync::Mutex;

/// Unique identifier for a view instance
///
/// ViewIds are generated sequentially and are guaranteed to be unique
/// within a running application.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ViewId(u64);

impl ViewId {
    /// Create a new unique ViewId
    ///
    /// This is typically called by ViewRegistry when inserting a new view.
    pub fn new() -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        Self(id)
    }

    /// Get the underlying numeric ID
    pub fn as_u64(self) -> u64 {
        self.0
    }

    /// Create a ViewId from a u64
    ///
    /// This is used by ViewRegistry to generate IDs from its own counter.
    pub fn from_u64(id: u64) -> Self {
        Self(id)
    }
}

/// Alternative ID generator using Mutex (if AtomicU64 is not available)
pub struct ViewIdGenerator {
    next_id: Mutex<u64>,
}

impl ViewIdGenerator {
    pub fn new() -> Self {
        Self {
            next_id: Mutex::new(1),
        }
    }

    pub fn generate(&self) -> ViewId {
        let mut next = self.next_id.lock();
        let id = *next;
        *next = next.wrapping_add(1);
        ViewId(id)
    }
}

impl Default for ViewIdGenerator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_view_id_uniqueness() {
        let id1 = ViewId::new();
        let id2 = ViewId::new();
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_view_id_clone() {
        let id1 = ViewId::new();
        let id2 = id1;
        assert_eq!(id1, id2);
    }

    #[test]
    fn test_view_id_from_u64() {
        let id = ViewId::from_u64(42);
        assert_eq!(id.as_u64(), 42);
    }

    #[test]
    fn test_view_id_generator() {
        let generator = ViewIdGenerator::new();
        let id1 = generator.generate();
        let id2 = generator.generate();
        assert_ne!(id1, id2);
    }
}
