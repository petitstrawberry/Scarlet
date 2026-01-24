//! PipelineOwner - Manages dirty flags and orchestrates render phases
//!
//! PipelineOwner tracks which elements need to be rebuilt, laid out, or repainted,
//! and orchestrates the flush of these dirty phases.
//!
//! PipelineOwner also owns the StateRegistry, ensuring there is only one
//! registry per application.

use alloc::collections::BTreeSet;
use crate::element::{ElementId, ElementTree, LayoutConstraints};
use crate::geometry::Size;
use crate::pipeline::StateRegistry;
use crate::state::{State, StateId};
use core::sync::atomic::{AtomicU32, Ordering};

/// Global dirty element ID for State change callbacks
///
/// This allows ComponentElement callbacks to notify the PipelineOwner
/// when State changes occur.
///
/// A value of 0 means "no dirty element", and any non-zero value is
/// an ElementId that needs to be marked dirty.
static GLOBAL_DIRTY_ID: AtomicU32 = AtomicU32::new(0);

/// Mark an element as dirty for rebuild (called from ComponentElement callbacks)
///
/// This function is called from State change callbacks in ComponentElement
/// to notify the PipelineOwner that an element needs to be rebuilt.
pub fn mark_element_dirty(id: ElementId) {
    // Store the element ID in the global atomic
    // Note: If multiple elements are marked dirty between flushes,
    // only the last one will be recorded. This is a simplified implementation
    // that works for the common case of single-element updates.
    // A full implementation would use a concurrent queue or set.
    GLOBAL_DIRTY_ID.store(id.get(), Ordering::SeqCst);
}

/// Take the globally dirty element ID (if any)
///
/// Returns None if no element is marked dirty.
pub(crate) fn take_global_dirty_id() -> Option<ElementId> {
    let id_val = GLOBAL_DIRTY_ID.swap(0, Ordering::SeqCst);
    if id_val != 0 {
        Some(ElementId::new(id_val))
    } else {
        None
    }
}

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
        // Collect any globally dirty elements from State change callbacks
        if let Some(dirty_id) = take_global_dirty_id() {
            self.mark_needs_build(dirty_id);
        }

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
            // Find the element in the tree
            if let Some(element) = element_tree.find_element_mut(id) {
                // Call rebuild() on the element
                // - ComponentElement: recreates child from stored View
                // - RenderElement: returns NoChange (properties updated via update())
                let _ = element.rebuild();
            }
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
            // Find the element and call render()
            if let Some(element) = element_tree.find_element_mut(id) {
                element.render();
            }
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

    /// Register a State instance
    ///
    /// This is a convenience method that forwards to the StateRegistry.
    /// Use this to register States when they are first created.
    pub fn register_state<T: 'static + Send + Sync>(&mut self, state: State<T>) -> StateId {
        self.state_registry.register(state)
    }

    /// Get a State from the registry by ID
    ///
    /// This is a convenience method that forwards to the StateRegistry.
    /// Returns a cloned State that shares data with the original.
    pub fn get_state<T: 'static + Clone>(&self, id: StateId) -> Option<State<T>> {
        self.state_registry.get(id)
    }

    /// Get a State reference from the registry by ID
    ///
    /// This is a convenience method that forwards to the StateRegistry.
    pub fn get_state_ref<T: 'static>(&self, id: StateId) -> Option<&State<T>> {
        self.state_registry.get_ref(id)
    }
}

impl Default for PipelineOwner {
    fn default() -> Self {
        Self::new()
    }
}
