//! PipelineOwner - Manages dirty flags and orchestrates render phases
//!
//! PipelineOwner tracks which elements need to be rebuilt, laid out, or repainted,
//! and orchestrates the flush of these dirty phases.

use alloc::collections::BTreeSet;
use crate::element::{ElementId, ElementTree, LayoutConstraints, StateRegistry};
use crate::geometry::Size;

/// Dirty flags for different render phases
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum DirtyPhase {
    Build,
    Layout,
    Paint,
}

/// PipelineOwner manages dirty flags and orchestrates render phases
///
/// This is inspired by Flutter's PipelineOwner and manages the three
/// main phases of the rendering pipeline:
/// - Build: Rebuild Elements from Views
/// - Layout: Recalculate positions and sizes
/// - Paint: Repaint to buffers
pub struct PipelineOwner {
    /// Elements that need rebuilding (State changed)
    dirty_build: BTreeSet<ElementId>,
    /// Elements that need relayouting
    dirty_layout: BTreeSet<ElementId>,
    /// Elements that need repainting
    dirty_paint: BTreeSet<ElementId>,
    /// State registry for managing State instances
    state_registry: StateRegistry,
}

impl PipelineOwner {
    /// Create a new PipelineOwner
    pub fn new() -> Self {
        Self {
            dirty_build: BTreeSet::new(),
            dirty_layout: BTreeSet::new(),
            dirty_paint: BTreeSet::new(),
            state_registry: StateRegistry::new(),
        }
    }

    /// Mark an element as dirty for a specific phase
    pub fn mark_dirty(&mut self, id: ElementId, phase: DirtyPhase) {
        match phase {
            DirtyPhase::Build => {
                self.dirty_build.insert(id);
                // Build implies layout and paint
                self.dirty_layout.insert(id);
                self.dirty_paint.insert(id);
            }
            DirtyPhase::Layout => {
                self.dirty_layout.insert(id);
                // Layout implies paint
                self.dirty_paint.insert(id);
            }
            DirtyPhase::Paint => {
                self.dirty_paint.insert(id);
            }
        }
    }

    /// Mark an element as needing a rebuild
    pub fn mark_needs_build(&mut self, id: ElementId) {
        self.mark_dirty(id, DirtyPhase::Build);
    }

    /// Mark an element as needing layout
    pub fn mark_needs_layout(&mut self, id: ElementId) {
        self.mark_dirty(id, DirtyPhase::Layout);
    }

    /// Mark an element as needing paint
    pub fn mark_needs_paint(&mut self, id: ElementId) {
        self.mark_dirty(id, DirtyPhase::Paint);
    }

    /// Check if there's any dirty work
    pub fn has_dirty(&self) -> bool {
        !self.dirty_build.is_empty() || !self.dirty_layout.is_empty() || !self.dirty_paint.is_empty()
    }

    /// Flush all dirty phases
    ///
    /// This processes build, layout, and paint in order.
    pub fn flush(&mut self, element_tree: &mut ElementTree, window_size: Size) {
        // 1. Build Phase: Rebuild Elements whose State changed
        self.flush_build(element_tree);

        // 2. Layout Phase: Recalculate layout
        self.flush_layout(element_tree, window_size);

        // 3. Paint Phase: Repaint dirty elements
        self.flush_paint(element_tree);
    }

    /// Flush the build phase
    fn flush_build(&mut self, element_tree: &mut ElementTree) {
        let dirty_build = core::mem::take(&mut self.dirty_build);

        for id in dirty_build {
            // Note: In a full implementation, we would:
            // 1. Find the element by ID
            // 2. Call element.update(new_view)
            // For now, this is a placeholder for the update logic
            let _ = id;
            let _ = element_tree;
        }
    }

    /// Flush the layout phase
    fn flush_layout(&mut self, element_tree: &mut ElementTree, window_size: Size) {
        let dirty_layout = core::mem::take(&mut self.dirty_layout);

        // Create constraints from window size
        let constraints = LayoutConstraints::loose(window_size.width, window_size.height);

        // Layout elements
        // Note: In a full implementation, we would:
        // 1. Topologically sort by dependencies
        // 2. Layout in parent-first order
        for id in dirty_layout {
            // Note: For now, we just layout the entire tree
            // In a full implementation, we would layout specific elements
            let _ = id;
            element_tree.layout(constraints);
        }
    }

    /// Flush the paint phase
    fn flush_paint(&mut self, element_tree: &mut ElementTree) {
        let dirty_paint = core::mem::take(&mut self.dirty_paint);

        for id in dirty_paint {
            // Note: In a full implementation, we would:
            // 1. Find the element
            // 2. Call element.render() if it's a RenderElement
            let _ = id;
            let _ = element_tree;
        }
    }

    /// Get the StateRegistry
    pub fn state_registry(&self) -> &StateRegistry {
        &self.state_registry
    }

    /// Get mutable reference to the StateRegistry
    pub fn state_registry_mut(&mut self) -> &mut StateRegistry {
        &mut self.state_registry
    }

    /// Check if there are any dirty build elements
    pub fn has_dirty_build(&self) -> bool {
        !self.dirty_build.is_empty()
    }

    /// Check if there are any dirty layout elements
    pub fn has_dirty_layout(&self) -> bool {
        !self.dirty_layout.is_empty()
    }

    /// Check if there are any dirty paint elements
    pub fn has_dirty_paint(&self) -> bool {
        !self.dirty_paint.is_empty()
    }
}

impl Default for PipelineOwner {
    fn default() -> Self {
        Self::new()
    }
}
