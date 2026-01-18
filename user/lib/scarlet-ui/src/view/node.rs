//! View node structure
//!
//! This module provides ViewNode, which combines a view with its
//! layout information and dirty flags. ViewNodes form the view tree.

extern crate alloc;
use alloc::boxed::Box;

use crate::layout::Size;
use crate::view::dirty::DirtyFlags;
use crate::view::id::ViewId;
use crate::view::traits::View;
use crate::view::tracker::RenderTracker;
use scarlet_std::fmt;

/// A node in the view tree
///
/// ViewNode combines:
/// - A boxed View trait object
/// - Parent-child relationships
/// - Cached layout information
/// - Dirty flags for incremental updates
///
/// ViewNodes form a tree structure that represents the UI hierarchy.
pub struct ViewNode {
    /// Unique identifier for this view
    pub id: ViewId,
    /// The view trait object
    pub view: Box<dyn View>,
    /// Parent view ID (None for root)
    pub parent: Option<ViewId>,
    /// Child view IDs
    pub children: scarlet_std::vec::Vec<ViewId>,
    /// Cached size from last layout
    pub cached_size: Size,
    /// Cached frame (position and size) from last layout
    pub cached_frame: Option<crate::graphics::Rect>,
    /// Layout constraints from last layout
    pub layout_constraints: Option<crate::layout::LayoutConstraints>,
    /// Dirty flags (what needs updating)
    pub dirty_flags: DirtyFlags,
    /// Last data version observed (for change detection)
    pub last_data_version: Option<u64>,
}

impl ViewNode {
    /// Create a new ViewNode
    pub fn new(view: Box<dyn View>) -> Self {
        let id = view.id();
        Self {
            id,
            view,
            parent: None,
            children: scarlet_std::vec::Vec::new(),
            cached_size: Size::ZERO,
            cached_frame: None,
            layout_constraints: None,
            dirty_flags: DirtyFlags::new(),
            last_data_version: None,
        }
    }

    /// Create a new ViewNode with an explicit parent
    pub fn with_parent(view: Box<dyn View>, parent: ViewId) -> Self {
        let mut node = Self::new(view);
        node.parent = Some(parent);
        node
    }

    /// Mark this view as dirty
    ///
    /// Sets the specified dirty flags. If paint is dirty, this also
    /// propagates to the parent (since the parent may need to composite
    /// the child's changes).
    pub fn mark_dirty(&mut self, flags: DirtyFlags) {
        self.dirty_flags.set(flags);
    }

    /// Mark this view as dirty and notify RenderTracker
    ///
    /// This is the AppKit-style notification method. It sets local dirty flags
    /// AND immediately notifies the RenderTracker, eliminating the need for
    /// recursive tree traversal during render cycles.
    ///
    /// # Arguments
    ///
    /// * `tracker` - The RenderTracker to notify
    /// * `flags` - The dirty flags to set
    pub fn mark_dirty_with_tracker(&mut self, tracker: &mut RenderTracker, flags: DirtyFlags) {
        // Set local flags first
        self.dirty_flags.set(flags);

        // Notify tracker immediately (O(1) operation)
        if flags.is_layout_dirty() {
            tracker.mark_dirty_layout(self.id);
        }
        if flags.is_paint_dirty() {
            tracker.mark_dirty_paint(self.id);
        }
    }

    /// Clear dirty flags
    pub fn clear_dirty(&mut self, flags: DirtyFlags) {
        self.dirty_flags.clear(flags);
    }

    /// Clear all dirty flags
    pub fn clear_all_dirty(&mut self) {
        self.dirty_flags.clear_all();
    }

    /// Check if layout is dirty
    pub fn needs_layout(&self) -> bool {
        self.dirty_flags.is_layout_dirty()
    }

    /// Check if paint is dirty
    pub fn needs_paint(&self) -> bool {
        self.dirty_flags.is_paint_dirty()
    }

    /// Check if this view needs any update
    pub fn is_dirty(&self) -> bool {
        self.dirty_flags.is_dirty()
    }

    /// Add a child view ID
    pub fn add_child(&mut self, child_id: ViewId) {
        self.children.push(child_id);
        self.mark_dirty(DirtyFlags::CHILDREN);
    }

    /// Remove a child view ID
    pub fn remove_child(&mut self, child_id: ViewId) -> bool {
        if let Some(pos) = self.children.iter().position(|&id| id == child_id) {
            self.children.remove(pos);
            self.mark_dirty(DirtyFlags::CHILDREN);
            true
        } else {
            false
        }
    }

    /// Get the frame for this view
    ///
    /// Returns the frame if it has been set during layout.
    pub fn frame(&self) -> Option<crate::graphics::Rect> {
        self.cached_frame
    }

    /// Set the frame for this view
    pub fn set_frame(&mut self, frame: crate::graphics::Rect) {
        self.cached_frame = Some(frame);
    }

    /// Get the size for this view
    ///
    /// Returns the cached size from the last layout pass.
    pub fn size(&self) -> Size {
        self.cached_size
    }

    /// Set the size for this view
    pub fn set_size(&mut self, size: Size) {
        self.cached_size = size;
    }

    /// Check if data has changed since last observation
    ///
    /// Returns true if the current data version is different from
    /// the last observed version.
    pub fn has_data_changed(&self, current_version: u64) -> bool {
        match self.last_data_version {
            Some(last) => last != current_version,
            None => true,
        }
    }

    /// Update the last observed data version
    pub fn update_data_version(&mut self, version: u64) {
        self.last_data_version = Some(version);
    }
}

impl fmt::Debug for ViewNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ViewNode")
            .field("id", &self.id)
            .field("parent", &self.parent)
            .field("children", &self.children)
            .field("cached_size", &self.cached_size)
            .field("cached_frame", &self.cached_frame)
            .field("dirty_flags", &self.dirty_flags)
            .field("last_data_version", &self.last_data_version)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{LayoutCtx, PaintCtx, UpdateCtx};
    use crate::event::Event;
    use crate::layout::LayoutConstraints;

    struct TestView {
        id: ViewId,
    }

    impl View for TestView {
        fn id(&self) -> ViewId {
            self.id
        }

        fn layout(&mut self, _ctx: &mut LayoutCtx, _constraints: LayoutConstraints) -> Size {
            Size::new(100, 100)
        }

        fn draw(&self, _ctx: &mut PaintCtx, _frame: crate::graphics::Rect) {
            // Do nothing for test
        }

        fn event(&mut self, _ctx: &mut crate::context::EventCtx, _event: &Event) -> crate::context::ControlFlow {
            crate::context::ControlFlow::Continue
        }

        fn update(&mut self, _ctx: &mut UpdateCtx) {
            // Do nothing for test
        }
    }

    #[test]
    fn test_view_node_new() {
        let view = Box::new(TestView { id: ViewId::new() });
        let node = ViewNode::new(view);
        assert_eq!(node.children.len(), 0);
        assert!(!node.is_dirty());
    }

    #[test]
    fn test_view_node_dirty() {
        let view = Box::new(TestView { id: ViewId::new() });
        let mut node = ViewNode::new(view);

        node.mark_dirty(DirtyFlags::LAYOUT);
        assert!(node.needs_layout());
        assert!(!node.needs_paint());

        node.mark_dirty(DirtyFlags::PAINT);
        assert!(node.needs_paint());

        node.clear_all_dirty();
        assert!(!node.is_dirty());
    }

    #[test]
    fn test_view_node_children() {
        let view = Box::new(TestView { id: ViewId::new() });
        let mut node = ViewNode::new(view);

        let child_id = ViewId::new();
        node.add_child(child_id);

        assert_eq!(node.children.len(), 1);
        assert!(node.children.contains(&child_id));

        assert!(node.remove_child(child_id));
        assert_eq!(node.children.len(), 0);
    }

    #[test]
    fn test_view_node_frame() {
        let view = Box::new(TestView { id: ViewId::new() });
        let mut node = ViewNode::new(view);

        let frame = crate::graphics::Rect::new(10, 20, 100, 200);
        node.set_frame(frame);

        assert_eq!(node.frame(), Some(frame));
    }

    #[test]
    fn test_view_node_data_version() {
        let view = Box::new(TestView { id: ViewId::new() });
        let mut node = ViewNode::new(view);

        // Initially no version, so data has "changed"
        assert!(node.has_data_changed(1));

        node.update_data_version(1);
        assert!(!node.has_data_changed(1));
        assert!(node.has_data_changed(2));
    }
}
