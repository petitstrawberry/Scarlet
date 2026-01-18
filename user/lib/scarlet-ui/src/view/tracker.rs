//! Render tracker for dirty view aggregation
//!
//! This module provides RenderTracker, which aggregates dirty flags from views
//! using an AppKit-style notification-based approach.
//!
//! # Key Design Principles
//!
//! - **No recursion**: Views notify directly, no tree traversal
//! - **O(1) notification**: Each dirty notification is O(1)
//! - **Aggregated**: Render cycle just iterates over dirty view IDs
//!
//! # Architecture
//!
//! ```
//! ViewNode ──mark_dirty()──> RenderTracker
//! DataContext ──modify()────> RenderTracker
//! EventCtx ──request()──────> RenderTracker
//! ```

use crate::view::id::ViewId;
use scarlet_std::collections::HashSet;
use scarlet_std::fmt;

/// Render tracker for aggregating dirty views
///
/// RenderTracker maintains sets of dirty view IDs for layout and paint.
/// Views notify the tracker directly when they become dirty, eliminating
/// the need for recursive tree traversal.
///
/// # Example
///
/// ```ignore
/// let mut tracker = RenderTracker::new();
///
/// // View marks itself as dirty
/// tracker.mark_dirty_layout(view_id);
///
/// // Render cycle consumes dirty views
/// let dirty_layout = tracker.take_dirty_layout();
/// for id in dirty_layout {
///     // layout only dirty views
/// }
/// ```
pub struct RenderTracker {
    /// Views that need layout recalculation
    dirty_layout: HashSet<ViewId>,
    /// Views that need repaint
    dirty_paint: HashSet<ViewId>,
}

impl RenderTracker {
    /// Create a new empty tracker
    pub fn new() -> Self {
        Self {
            dirty_layout: HashSet::new(),
            dirty_paint: HashSet::new(),
        }
    }

    /// Mark a view as needing layout
    ///
    /// This is called when a view's size constraints change or
    /// when it explicitly requests layout.
    pub fn mark_dirty_layout(&mut self, view_id: ViewId) {
        self.dirty_layout.insert(view_id);
    }

    /// Mark a view as needing paint
    ///
    /// This is called when a view's appearance changes but size doesn't.
    pub fn mark_dirty_paint(&mut self, view_id: ViewId) {
        self.dirty_paint.insert(view_id);
    }

    /// Mark a view as needing both layout and paint
    ///
    /// Convenience method for marking both dirty flags.
    pub fn mark_dirty(&mut self, view_id: ViewId) {
        self.mark_dirty_layout(view_id);
        self.mark_dirty_paint(view_id);
    }

    /// Clear dirty flags for a specific view
    ///
    /// Called after a view has been updated.
    pub fn clear_dirty(&mut self, view_id: ViewId) {
        self.dirty_layout.remove(&view_id);
        self.dirty_paint.remove(&view_id);
    }

    /// Take all views needing layout
    ///
    /// This consumes the current set of dirty layout views and returns them.
    /// Called during the render cycle.
    ///
    /// # Returns
    ///
    /// The set of view IDs that need layout. The tracker's dirty_layout set is cleared.
    pub fn take_dirty_layout(&mut self) -> HashSet<ViewId> {
        core::mem::take(&mut self.dirty_layout)
    }

    /// Take all views needing paint
    ///
    /// This consumes the current set of dirty paint views and returns them.
    /// Called during the render cycle.
    ///
    /// # Returns
    ///
    /// The set of view IDs that need paint. The tracker's dirty_paint set is cleared.
    pub fn take_dirty_paint(&mut self) -> HashSet<ViewId> {
        core::mem::take(&mut self.dirty_paint)
    }

    /// Check if any views need layout
    pub fn needs_layout(&self) -> bool {
        !self.dirty_layout.is_empty()
    }

    /// Check if any views need paint
    pub fn needs_paint(&self) -> bool {
        !self.dirty_paint.is_empty()
    }

    /// Check if the tracker has any dirty views
    pub fn is_dirty(&self) -> bool {
        self.needs_layout() || self.needs_paint()
    }

    /// Get the number of views needing layout
    pub fn dirty_layout_count(&self) -> usize {
        self.dirty_layout.len()
    }

    /// Get the number of views needing paint
    pub fn dirty_paint_count(&self) -> usize {
        self.dirty_paint.len()
    }

    /// Clear all dirty flags
    ///
    /// Called after render cycle completes.
    pub fn clear_all(&mut self) {
        self.dirty_layout.clear();
        self.dirty_paint.clear();
    }

    /// Mark multiple views as needing layout
    pub fn mark_dirty_layout_many(&mut self, view_ids: impl IntoIterator<Item = ViewId>) {
        self.dirty_layout.extend(view_ids);
    }

    /// Mark multiple views as needing paint
    pub fn mark_dirty_paint_many(&mut self, view_ids: impl IntoIterator<Item = ViewId>) {
        self.dirty_paint.extend(view_ids);
    }
}

impl Default for RenderTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for RenderTracker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RenderTracker")
            .field("dirty_layout_count", &self.dirty_layout_count())
            .field("dirty_paint_count", &self.dirty_paint_count())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tracker_new() {
        let tracker = RenderTracker::new();
        assert!(!tracker.needs_layout());
        assert!(!tracker.needs_paint());
        assert!(!tracker.is_dirty());
    }

    #[test]
    fn test_mark_dirty_layout() {
        let mut tracker = RenderTracker::new();
        let id = ViewId::new();

        tracker.mark_dirty_layout(id);
        assert!(tracker.needs_layout());
        assert!(!tracker.needs_paint());
        assert_eq!(tracker.dirty_layout_count(), 1);
    }

    #[test]
    fn test_mark_dirty_paint() {
        let mut tracker = RenderTracker::new();
        let id = ViewId::new();

        tracker.mark_dirty_paint(id);
        assert!(!tracker.needs_layout());
        assert!(tracker.needs_paint());
        assert_eq!(tracker.dirty_paint_count(), 1);
    }

    #[test]
    fn test_mark_dirty_both() {
        let mut tracker = RenderTracker::new();
        let id = ViewId::new();

        tracker.mark_dirty(id);
        assert!(tracker.needs_layout());
        assert!(tracker.needs_paint());
    }

    #[test]
    fn test_take_dirty_layout() {
        let mut tracker = RenderTracker::new();
        let id1 = ViewId::new();
        let id2 = ViewId::new();

        tracker.mark_dirty_layout(id1);
        tracker.mark_dirty_layout(id2);

        assert_eq!(tracker.dirty_layout_count(), 2);

        let dirty = tracker.take_dirty_layout();
        assert_eq!(dirty.len(), 2);
        assert!(dirty.contains(&id1));
        assert!(dirty.contains(&id2));

        // After take, tracker should be clean
        assert!(!tracker.needs_layout());
    }

    #[test]
    fn test_take_dirty_paint() {
        let mut tracker = RenderTracker::new();
        let id = ViewId::new();

        tracker.mark_dirty_paint(id);

        let dirty = tracker.take_dirty_paint();
        assert_eq!(dirty.len(), 1);
        assert!(dirty.contains(&id));

        assert!(!tracker.needs_paint());
    }

    #[test]
    fn test_clear_dirty() {
        let mut tracker = RenderTracker::new();
        let id = ViewId::new();

        tracker.mark_dirty(id);
        assert!(tracker.is_dirty());

        tracker.clear_dirty(id);
        assert!(!tracker.is_dirty());
    }

    #[test]
    fn test_clear_all() {
        let mut tracker = RenderTracker::new();
        let id1 = ViewId::new();
        let id2 = ViewId::new();

        tracker.mark_dirty_layout(id1);
        tracker.mark_dirty_paint(id2);
        assert!(tracker.is_dirty());

        tracker.clear_all();
        assert!(!tracker.is_dirty());
        assert!(!tracker.needs_layout());
        assert!(!tracker.needs_paint());
    }

    #[test]
    fn test_mark_many() {
        let mut tracker = RenderTracker::new();
        let ids = vec![ViewId::new(), ViewId::new(), ViewId::new()];

        tracker.mark_dirty_layout_many(ids.clone());
        assert_eq!(tracker.dirty_layout_count(), 3);
    }
}
