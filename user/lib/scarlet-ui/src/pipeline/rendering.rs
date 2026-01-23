//! RenderingPipeline - Integration of PipelineOwner, ElementTree, and Compositor
//!
//! RenderingPipeline is the main entry point for the rendering system.
//! It orchestrates all phases of the rendering pipeline.

use alloc::boxed::Box;
use crate::element::{Element, ElementTree, LayoutConstraints};
use crate::geometry::Size;
use crate::compositor::Compositor;
use crate::pipeline::PipelineOwner;
use crate::buffer::Buffer;

/// RenderingPipeline integrates all components of the rendering system
///
/// This is the main orchestrator that combines:
/// - ElementTree: Manages the element hierarchy
/// - PipelineOwner: Manages dirty flags and flush phases
/// - Compositor: Composites buffers into the window buffer
pub struct RenderingPipeline {
    /// Element tree
    element_tree: ElementTree,
    /// Pipeline owner for dirty flag management
    pipeline_owner: PipelineOwner,
    /// Compositor for rendering (created after initial layout)
    compositor: Option<Compositor>,
    /// Current window size
    window_size: Size,
}

impl RenderingPipeline {
    /// Create a new RenderingPipeline
    pub fn new() -> Self {
        Self {
            element_tree: ElementTree::new(),
            pipeline_owner: PipelineOwner::new(),
            compositor: None,
            window_size: Size::new(800.0, 600.0),
        }
    }

    /// Set the root Element
    pub fn set_root(&mut self, root_element: Box<dyn Element>) {
        self.element_tree.set_root(root_element);
    }

    /// Get the ElementTree
    pub fn element_tree(&self) -> &ElementTree {
        &self.element_tree
    }

    /// Get mutable reference to the ElementTree
    pub fn element_tree_mut(&mut self) -> &mut ElementTree {
        &mut self.element_tree
    }

    /// Get the PipelineOwner
    pub fn pipeline_owner(&self) -> &PipelineOwner {
        &self.pipeline_owner
    }

    /// Get mutable reference to the PipelineOwner
    pub fn pipeline_owner_mut(&mut self) -> &mut PipelineOwner {
        &mut self.pipeline_owner
    }

    /// Get the StateRegistry
    pub fn state_registry(&self) -> &crate::element::StateRegistry {
        self.pipeline_owner.state_registry()
    }

    /// Get mutable reference to the StateRegistry
    pub fn state_registry_mut(&mut self) -> &mut crate::element::StateRegistry {
        self.pipeline_owner.state_registry_mut()
    }

    /// Perform initial layout
    ///
    /// This should be called once after setting the root element
    /// to determine the window size and create the compositor.
    pub fn layout_initial(&mut self) -> Size {
        // Perform initial layout with loose constraints
        let constraints = LayoutConstraints::loose(self.window_size.width, self.window_size.height);
        let size = self.element_tree.layout(constraints);

        // Create compositor with the calculated size
        self.compositor = Some(Compositor::new(size));
        self.window_size = size;

        size
    }

    /// Set window size and resize compositor
    pub fn resize(&mut self, new_size: Size) {
        self.window_size = new_size;
        if let Some(ref mut compositor) = self.compositor {
            compositor.resize(new_size);
        }

        // Mark entire tree for relayout
        // Note: In a full implementation, we would mark specific elements
        if let Some(root) = self.element_tree.root() {
            self.pipeline_owner.mark_needs_layout(root.id());
        }
    }

    /// Handle a render frame
    ///
    /// This flushes all dirty phases and renders to the window buffer.
    pub fn render(&mut self) -> Option<&Buffer> {
        // Flush all dirty phases
        self.pipeline_owner.flush(&mut self.element_tree, self.window_size);

        // Composite the tree
        if let Some(ref mut compositor) = self.compositor {
            // Note: In a full implementation, we would get the root RenderObject
            // and call composite_tree. For now, we just clear the buffer.
            compositor.clear(crate::color::Color::WHITE);

            Some(compositor.window_buffer())
        } else {
            None
        }
    }

    /// Get the window buffer (if available)
    pub fn window_buffer(&self) -> Option<&Buffer> {
        self.compositor.as_ref().map(|c| c.window_buffer())
    }

    /// Get mutable access to the window buffer
    pub fn window_buffer_mut(&mut self) -> Option<&mut Buffer> {
        self.compositor.as_mut().map(|c| c.window_buffer_mut())
    }

    /// Get the current window size
    pub fn window_size(&self) -> Size {
        self.window_size
    }

    /// Handle an event
    ///
    /// In a full implementation, this would route events through the
    /// EventDispatcher to the target elements.
    pub fn handle_event(&mut self, _event: &crate::event::Event) -> bool {
        // Note: Event handling will be implemented in a later phase
        false
    }
}

impl Default for RenderingPipeline {
    fn default() -> Self {
        Self::new()
    }
}
