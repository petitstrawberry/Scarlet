//! Dirty tracking flags for element updates
//!
//! These flags are used to track what aspects of an element need to be updated
//! during the rendering pipeline.

use bitflags::bitflags;

bitflags! {
    pub struct DirtyFlags: u32 {
        /// View rebuild is required
        ///
        /// Set when the view's structure or properties have changed,
        /// requiring the element tree to be rebuilt.
        const BUILD      = 1 << 0;

        /// Layout recalculation is required
        ///
        /// Set when constraints or sizing information has changed,
        /// requiring a new layout pass.
        const LAYOUT     = 1 << 1;

        /// Repaint is required
        ///
        /// Set when visual properties have changed but layout hasn't,
        /// requiring only a paint pass.
        const PAINT      = 1 << 2;

        /// Children structure has changed
        ///
        /// Set when children have been added, removed, or reordered,
        /// requiring reconciliation of the child tree.
        const CHILDREN   = 1 << 3;
    }
}

impl Default for DirtyFlags {
    fn default() -> Self {
        Self::empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dirty_flags() {
        let mut flags = DirtyFlags::empty();

        // Test flag setting
        flags |= DirtyFlags::BUILD;
        assert!(flags.contains(DirtyFlags::BUILD));
        assert!(!flags.contains(DirtyFlags::LAYOUT));

        // Test multiple flags
        flags |= DirtyFlags::PAINT;
        assert!(flags.contains(DirtyFlags::BUILD));
        assert!(flags.contains(DirtyFlags::PAINT));

        // Test flag clearing
        flags -= DirtyFlags::BUILD;
        assert!(!flags.contains(DirtyFlags::BUILD));
        assert!(flags.contains(DirtyFlags::PAINT));

        // Test empty
        flags = DirtyFlags::empty();
        assert!(flags.is_empty());
    }

    #[test]
    fn test_default() {
        let flags: DirtyFlags = Default::default();
        assert!(flags.is_empty());
    }
}
