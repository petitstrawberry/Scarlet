//! Unique identifier for Elements

use core::sync::atomic::{AtomicU32, Ordering};

/// Unique identifier for Element instances
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct ElementId(u32);

impl ElementId {
    /// Create a new ElementId from a raw value
    pub const fn new(id: u32) -> Self {
        Self(id)
    }

    /// Get the raw ID value
    pub const fn get(self) -> u32 {
        self.0
    }

    /// Generate a new unique ElementId
    pub fn generate() -> Self {
        static ELEMENT_ID_COUNTER: AtomicU32 = AtomicU32::new(0);
        let id = ELEMENT_ID_COUNTER.fetch_add(1, Ordering::SeqCst);
        Self(id)
    }
}

impl Default for ElementId {
    fn default() -> Self {
        Self::generate()
    }
}
