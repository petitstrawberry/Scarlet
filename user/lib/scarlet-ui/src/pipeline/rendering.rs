//! RenderingPipeline - Integration of PipelineOwner, ElementTree, and Compositor
//!
//! RenderingPipeline is the main entry point for the rendering system.
//! It orchestrates all phases of the rendering pipeline.

use alloc::boxed::Box;
use alloc::string::String;
use crate::element::{Element, ElementTree, LayoutConstraints};
use crate::geometry::Size;
use crate::compositor::Compositor;
use crate::pipeline::PipelineOwner;
use crate::buffer::Buffer;
use crate::render::RenderTree;
use crate::event::EventDispatcher;

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
    /// Event dispatcher
    event_dispatcher: EventDispatcher,
}

impl RenderingPipeline {
    /// Create a new RenderingPipeline
    pub fn new() -> Self {
        Self {
            element_tree: ElementTree::new(),
            pipeline_owner: PipelineOwner::new(),
            compositor: None,
            window_size: Size::new(800.0, 600.0),
            event_dispatcher: EventDispatcher::new(),
        }
    }

    /// Set the root Element
    pub fn set_root(&mut self, root_element: Box<dyn Element>) {
        self.element_tree.set_root(root_element);
        if let Some(root) = self.element_tree.root() {
            self.event_dispatcher.set_root(root.id());
        }
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
    pub fn state_registry(&self) -> &crate::pipeline::StateRegistry {
        self.pipeline_owner.state_registry()
    }

    /// Get mutable reference to the StateRegistry
    pub fn state_registry_mut(&mut self) -> &mut crate::pipeline::StateRegistry {
        self.pipeline_owner.state_registry_mut()
    }

    /// Has any dirty elements?
    pub fn has_dirty(&self) -> bool {
        self.pipeline_owner.has_dirty()
    }

    /// Extract window information from the element tree
    ///
    /// This searches the element tree for a Window View and extracts
    /// the app_id, title, and size from it.
    ///
    /// Returns (app_id, title, size) or defaults if no Window is found.
    fn extract_window_info(&self) -> (String, String, Size) {
        // Default values
        let default_app_id = String::from("com.example.scarletui");
        let default_title = String::from("ScarletUI Application");
        let default_size = Size::new(800.0, 600.0);

        // Try to find a Window View in the element tree
        if let Some(root) = self.element_tree.root() {
            if let Some(window_info) = self.find_window_view(root) {
                return window_info;
            }
        }

        (default_app_id, default_title, default_size)
    }

    /// Recursively search for a Window View in the element tree
    fn find_window_view(&self, element: &dyn Element) -> Option<(String, String, Size)> {
        // Check if this element provides window info
        if let Some(info) = element.get_window_info() {
            return Some(info);
        }

        // Check children recursively
        for child in element.children() {
            if let Some(info) = self.find_window_view(child.as_ref()) {
                return Some(info);
            }
        }

        None
    }

    /// Perform initial layout
    ///
    /// This should be called once after setting the root element
    /// to determine the window size and create the compositor.
    ///
    /// Returns (app_id, title, size) extracted from the Window View
    pub fn layout_initial(&mut self) -> (String, String, Size) {
        // Extract window info first
        let (app_id, title, preferred_size) = self.extract_window_info();

        // Perform initial layout with loose constraints
        let constraints = LayoutConstraints::loose(preferred_size.width, preferred_size.height);
        let size = self.element_tree.layout(constraints);

        // Create compositor with the calculated size
        self.compositor = Some(Compositor::new(size));
        self.window_size = size;

        // Mark root as dirty for initial paint
        if let Some(root) = self.element_tree.root() {
            self.pipeline_owner.mark_needs_paint(root.id());
        }

        (app_id, title, size)
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
        if crate::debug::is_enabled() {
            scarlet_std::println!("[RenderingPipeline] render() starting...");
        }
        // Flush all dirty phases (build, layout, paint)
        self.pipeline_owner.flush(&mut self.element_tree, self.window_size);
        if crate::debug::is_enabled() {
            scarlet_std::println!("[RenderingPipeline] flush() completed");
        }

        // Composite all elements into the window buffer
        if let Some(ref mut compositor) = self.compositor {
            if let Some(root) = self.element_tree.root() {
                if crate::debug::is_enabled() {
                    scarlet_std::println!("[RenderingPipeline] building RenderTree...");
                }
                let render_tree = RenderTree::build(root);
                if crate::debug::is_enabled() {
                    scarlet_std::println!("[RenderingPipeline] RenderTree built, compositing...");
                }
                compositor.composite_tree(&render_tree);
            } else {
                if crate::debug::is_enabled() {
                    scarlet_std::println!("[RenderingPipeline] No root element to render");
                }
            }

            Some(compositor.window_buffer())
        } else {
            if crate::debug::is_enabled() {
                scarlet_std::println!("[RenderingPipeline] No compositor!");
            }
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
        self.event_dispatcher.dispatch(&mut self.element_tree, _event)
    }
}

impl Default for RenderingPipeline {
    fn default() -> Self {
        Self::new()
    }
}
