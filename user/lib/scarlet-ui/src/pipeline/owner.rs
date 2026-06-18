//! PipelineOwner - Manages dirty flags and orchestrates render phases
//!
//! PipelineOwner tracks which elements need to be rebuilt, laid out, or repainted,
//! and orchestrates the flush of these dirty phases.
//!
//! PipelineOwner also owns the StateRegistry, ensuring there is only one
//! registry per application.

use crate::element::{ElementId, ElementTree, LayoutConstraints};
use crate::geometry::Size;
use crate::os::Mutex;
use crate::pipeline::StateRegistry;
use crate::state::{State, StateId};
use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet};
use core::sync::atomic::{AtomicU64, Ordering};
use std::println;

/// Stable owner ID for dirty queues belonging to one rendering pipeline.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct PipelineId(u64);

impl PipelineId {
    /// Generate a globally unique pipeline ID.
    pub fn generate() -> Self {
        static NEXT_PIPELINE_ID: AtomicU64 = AtomicU64::new(1);
        Self(NEXT_PIPELINE_ID.fetch_add(1, Ordering::SeqCst))
    }

    /// Create a pipeline ID from a raw numeric value.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Return the raw numeric value.
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl Default for PipelineId {
    fn default() -> Self {
        Self(0)
    }
}

/// Context passed while mounting an element tree.
#[derive(Clone, Copy, Debug)]
pub struct MountContext {
    pipeline_id: PipelineId,
}

impl MountContext {
    /// Create a mount context for a rendering pipeline.
    pub const fn new(pipeline_id: PipelineId) -> Self {
        Self { pipeline_id }
    }

    /// Return the owning pipeline ID.
    pub const fn pipeline_id(self) -> PipelineId {
        self.pipeline_id
    }
}

#[derive(Default)]
struct DirtyQueues {
    build: BTreeSet<ElementId>,
    layout: BTreeSet<ElementId>,
    paint: BTreeSet<ElementId>,
    self_paint: BTreeSet<ElementId>,
}

/// Global dirty element IDs for State change callbacks, partitioned by pipeline.
static GLOBAL_DIRTY: Mutex<BTreeMap<PipelineId, DirtyQueues>> = Mutex::new(BTreeMap::new());

/// Mark an element as dirty for rebuild (called from ComponentElement callbacks)
///
/// This function is called from State change callbacks in ComponentElement
/// to notify the PipelineOwner that an element needs to be rebuilt.
pub fn mark_element_dirty(owner: PipelineId, id: ElementId) {
    if crate::debug::is_enabled() {
        println!(
            "[PipelineOwner] mark_element_dirty: owner={}, id={}",
            owner.get(),
            id.get()
        );
    }
    let mut queues = GLOBAL_DIRTY.lock();
    let queue = queues.entry(owner).or_default();
    queue.build.insert(id);
    if crate::debug::is_enabled() {
        println!("[PipelineOwner] Dirty count: {}", queue.build.len());
    }
}

/// Mark an element as needing paint only (no build/layout).
pub fn mark_element_needs_paint(owner: PipelineId, id: ElementId) {
    let mut queues = GLOBAL_DIRTY.lock();
    queues.entry(owner).or_default().paint.insert(id);
}

/// Mark an element as needing layout and paint.
pub fn mark_element_needs_layout(owner: PipelineId, id: ElementId) {
    let mut queues = GLOBAL_DIRTY.lock();
    queues.entry(owner).or_default().layout.insert(id);
}

/// Mark an element as needing paint for its own buffer only.
///
/// This is for render objects whose internal state changed without changing
/// their descendants, such as a window titlebar hover state.
pub fn mark_element_needs_self_paint(owner: PipelineId, id: ElementId) {
    let mut queues = GLOBAL_DIRTY.lock();
    queues.entry(owner).or_default().self_paint.insert(id);
}

fn take_dirty_for(owner: PipelineId) -> DirtyQueues {
    GLOBAL_DIRTY.lock().remove(&owner).unwrap_or_default()
}

pub(crate) fn clear_global_dirty(owner: PipelineId) {
    GLOBAL_DIRTY.lock().remove(&owner);
}

pub(crate) fn has_global_dirty(owner: PipelineId) -> bool {
    GLOBAL_DIRTY.lock().get(&owner).is_some_and(|queue| {
        !queue.build.is_empty()
            || !queue.layout.is_empty()
            || !queue.paint.is_empty()
            || !queue.self_paint.is_empty()
    })
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
    /// Dirty queue owner ID.
    pipeline_id: PipelineId,
    /// Elements that need rebuilding (State changed)
    dirty_build: BTreeSet<ElementId>,
    /// Elements that need relayouting
    dirty_layout: BTreeSet<ElementId>,
    /// Elements that need repainting
    dirty_paint: BTreeSet<ElementId>,
    /// Elements whose own buffers need repainting without repainting descendants
    dirty_self_paint: BTreeSet<ElementId>,
    /// Elements repainted in the last flush
    last_paint_ids: alloc::vec::Vec<ElementId>,
    /// State registry for managing State instances
    state_registry: StateRegistry,
}

impl PipelineOwner {
    /// Create a new PipelineOwner
    pub fn new() -> Self {
        Self::with_pipeline_id(PipelineId::default())
    }

    /// Create a PipelineOwner for a specific rendering pipeline.
    pub fn with_pipeline_id(pipeline_id: PipelineId) -> Self {
        Self {
            pipeline_id,
            dirty_build: BTreeSet::new(),
            dirty_layout: BTreeSet::new(),
            dirty_paint: BTreeSet::new(),
            dirty_self_paint: BTreeSet::new(),
            last_paint_ids: alloc::vec::Vec::new(),
            state_registry: StateRegistry::new(),
        }
    }

    /// Mark an element as dirty for a specific phase
    pub fn mark_dirty(&mut self, id: ElementId, phase: DirtyPhase) {
        match phase {
            DirtyPhase::Build => {
                self.dirty_build.insert(id);
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

    /// Mark an element as needing only its own buffer repainted.
    pub fn mark_needs_self_paint(&mut self, id: ElementId) {
        self.dirty_self_paint.insert(id);
    }

    /// Check if there's any dirty work
    pub fn has_dirty(&self) -> bool {
        has_global_dirty(self.pipeline_id)
            || !self.dirty_build.is_empty()
            || !self.dirty_layout.is_empty()
            || !self.dirty_paint.is_empty()
            || !self.dirty_self_paint.is_empty()
    }

    /// Flush all dirty phases
    ///
    /// This processes build, layout, and paint in order.
    pub fn flush(&mut self, element_tree: &mut ElementTree, window_size: Size) {
        self.collect_global_dirty();

        // 1. Build Phase: Rebuild Elements whose State changed
        self.flush_build(element_tree);
        self.collect_global_dirty();

        // 2. Layout Phase: Recalculate layout
        self.flush_layout(element_tree, window_size);

        // 3. Paint Phase: Repaint dirty elements
        self.flush_paint(element_tree);
    }

    fn collect_global_dirty(&mut self) {
        let dirty = take_dirty_for(self.pipeline_id);
        for id in dirty.build {
            self.mark_needs_build(id);
        }
        for id in dirty.layout {
            self.mark_needs_layout(id);
        }
        for id in dirty.paint {
            self.mark_needs_paint(id);
        }
        for id in dirty.self_paint {
            self.mark_needs_self_paint(id);
        }
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
                if matches!(element.rebuild(), crate::element::UpdateResult::Updated) {
                    self.mark_needs_layout(id);
                }
            }
        }
    }

    /// Flush the layout phase
    fn flush_layout(&mut self, element_tree: &mut ElementTree, window_size: Size) {
        let dirty_layout = core::mem::take(&mut self.dirty_layout);
        if dirty_layout.is_empty() {
            return;
        }

        // Create constraints from window size
        let constraints = LayoutConstraints::loose(window_size.width, window_size.height);

        let root_id = match element_tree.root() {
            Some(root) => root.id(),
            None => return,
        };

        let mut layout_roots = BTreeSet::new();
        let mut full_layout = false;

        for id in dirty_layout {
            if id == root_id {
                full_layout = true;
                break;
            }

            let path = match element_tree.find_path_ids(id) {
                Some(path) => path,
                None => {
                    full_layout = true;
                    break;
                }
            };

            if path.len() < 2 {
                full_layout = true;
                break;
            }

            let mut chosen_id = path[path.len() - 2];
            let mut has_constraints = false;

            for ancestor_id in path[..path.len() - 1].iter().rev() {
                let constraints_opt = element_tree
                    .find_element_mut(*ancestor_id)
                    .and_then(|element| element.last_layout_constraints());
                let Some(ancestor_constraints) = constraints_opt else {
                    has_constraints = false;
                    break;
                };

                chosen_id = *ancestor_id;
                has_constraints = true;

                if ancestor_constraints.is_tight() {
                    break;
                }
            }

            if !has_constraints {
                full_layout = true;
                break;
            }

            layout_roots.insert(chosen_id);
        }

        if full_layout || layout_roots.is_empty() {
            element_tree.layout(constraints);
            if let Some(root) = element_tree.root() {
                self.dirty_paint.insert(root.id());
            }
            return;
        }

        let mut fallback_full = false;
        for id in layout_roots.iter().copied() {
            if let Some(element) = element_tree.find_element_mut(id) {
                if let Some(local_constraints) = element.last_layout_constraints() {
                    element.layout(local_constraints);
                    self.dirty_paint.insert(id);
                } else {
                    fallback_full = true;
                    break;
                }
            } else {
                fallback_full = true;
                break;
            }
        }

        if fallback_full {
            element_tree.layout(constraints);
            if let Some(root) = element_tree.root() {
                self.dirty_paint.insert(root.id());
            }
        }
    }

    /// Flush the paint phase
    fn flush_paint(&mut self, element_tree: &mut ElementTree) {
        let dirty_paint = core::mem::take(&mut self.dirty_paint);
        let dirty_self_paint = core::mem::take(&mut self.dirty_self_paint);
        self.last_paint_ids.clear();
        self.last_paint_ids.extend(dirty_paint.iter().copied());
        self.last_paint_ids.extend(dirty_self_paint.iter().copied());

        if crate::debug::is_enabled() {
            crate::logln!(
                "[PipelineOwner] flush_paint: {} dirty elements",
                dirty_paint.len()
            );
        }

        for id in dirty_paint {
            if crate::debug::is_enabled() {
                crate::logln!(
                    "[PipelineOwner] flush_paint: rendering element id={}",
                    id.get()
                );
            }
            // Find the element and call render()
            if let Some(element) = element_tree.find_element_mut(id) {
                // Render this element and all its descendants
                // Containers like VStack/HStack don't have buffers, but their children do
                Self::render_recursive(element, 0);
            }
        }

        for id in dirty_self_paint {
            if crate::debug::is_enabled() {
                crate::logln!(
                    "[PipelineOwner] flush_paint: rendering self element id={}",
                    id.get()
                );
            }
            if let Some(element) = element_tree.find_element_mut(id) {
                element.render();
            }
        }
    }

    /// Recursively render an element and all its descendants
    fn render_recursive(element: &mut Box<dyn crate::element::Element>, depth: usize) {
        let indent = "  ".repeat(depth);
        let type_name = element.type_name_debug();
        let has_buffer = element.get_buffer().is_some();

        if crate::debug::is_enabled() {
            crate::logln!(
                "[PipelineOwner] {}render: {} (buffer={})",
                indent,
                type_name,
                has_buffer
            );
        }

        // Render children before the parent so parent renderers that composite
        // child buffers, such as WindowRenderElement, see fresh child content.
        let children = element.children();
        if crate::debug::is_enabled() {
            crate::logln!("[PipelineOwner] {}has {} children", indent, children.len());
        }

        for (i, child) in element.children_mut().iter_mut().enumerate() {
            if crate::debug::is_enabled() {
                crate::logln!(
                    "[PipelineOwner] {}child #{}: {}",
                    indent,
                    i,
                    child.type_name_debug()
                );
            }
            Self::render_recursive(child, depth + 1);
        }

        // Render this element
        element.render();
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
        !self.dirty_paint.is_empty() || !self.dirty_self_paint.is_empty()
    }

    /// Get the IDs repainted in the last flush.
    pub fn last_paint_ids(&self) -> &[ElementId] {
        &self.last_paint_ids
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
