//! RenderingPipeline - Integration of PipelineOwner, ElementTree, and Compositor
//!
//! RenderingPipeline is the main entry point for the rendering system.
//! It orchestrates all phases of the rendering pipeline.

use alloc::boxed::Box;
use alloc::string::String;
use crate::element::{Element, ElementTree, LayoutConstraints};
use crate::geometry::{Size, Point};
use crate::compositor::Compositor;
use crate::pipeline::PipelineOwner;
use crate::buffer::Buffer;
use crate::views::Window;

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
    pub fn state_registry(&self) -> &crate::pipeline::StateRegistry {
        self.pipeline_owner.state_registry()
    }

    /// Get mutable reference to the StateRegistry
    pub fn state_registry_mut(&mut self) -> &mut crate::pipeline::StateRegistry {
        self.pipeline_owner.state_registry_mut()
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
        // Try to downcast ComponentElement to check if it contains a Window
        // Note: This is a simplified version. In a full implementation, we would need
        // to handle different child types of Window more generically.
        if let Some(component) = element.as_any().downcast_ref::<crate::element::ComponentElement<Window<crate::views::Rectangle>>>() {
            let view = component.view();
            return Some((
                String::from(view.get_app_id()),
                String::from(view.get_title()),
                view.get_window_size(),
            ));
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
        // Flush all dirty phases
        self.pipeline_owner.flush(&mut self.element_tree, self.window_size);

        // Collect elements for compositing (before borrowing compositor)
        let mut elements_to_composite = alloc::vec::Vec::new();
        if let Some(root) = self.element_tree.root() {
            self.collect_elements_for_composite(root, Point::ZERO, &mut elements_to_composite);
        }

        // Composite all elements into the window buffer
        if let Some(ref mut compositor) = self.compositor {
            // Clear background
            compositor.clear(crate::color::Color::WHITE);

            // Composite in reverse order (children first for painter's algorithm)
            for (element, origin) in elements_to_composite.into_iter().rev() {
                if let Some(buffer) = element.get_buffer() {
                    compositor.window_buffer_mut().composite(
                        buffer,
                        origin.x as i32,
                        origin.y as i32,
                        1.0,
                    );
                }
            }

            Some(compositor.window_buffer())
        } else {
            None
        }
    }

    /// Collect elements for compositing (depth-first, children first)
    fn collect_elements_for_composite<'a>(
        &self,
        element: &'a dyn Element,
        origin: Point,
        result: &mut alloc::vec::Vec<(&'a dyn Element, Point)>,
    ) {
        let position = element.position();
        let absolute_origin = Point {
            x: origin.x + position.x,
            y: origin.y + position.y,
        };

        // Add children first (painter's algorithm)
        for child in element.children() {
            self.collect_elements_for_composite(child.as_ref(), absolute_origin, result);
        }

        // Add this element
        result.push((element, absolute_origin));
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
        // The EventDispatcher will pass the Phase parameter when dispatching
        false
    }
}

impl Default for RenderingPipeline {
    fn default() -> Self {
        Self::new()
    }
}
