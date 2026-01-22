use std::vec::Vec;

use crate::buffer::Buffer;
use crate::geometry::{Point, Rect, Size};
use crate::traits::RenderObject;

/// Layer represents a drawable surface with position information
pub struct Layer {
    pub buffer: Buffer,
    pub frame: Rect,
    pub children: Vec<Layer>,
}

impl Layer {
    pub fn new(buffer: Buffer, frame: Rect) -> Self {
        Self {
            buffer,
            frame,
            children: Vec::new(),
        }
    }

    /// Composite this layer and all children into a single buffer
    pub fn composite(&self) -> Buffer {
        // Calculate total size needed
        let total_size = self.calculate_size();

        // Create final buffer
        let mut final_buffer = Buffer::new(total_size);

        // Composite self (blit at frame origin)
        let offset = Point::new(self.frame.origin.x, self.frame.origin.y);
        final_buffer.blit_from(&self.buffer, Rect::new(offset, self.buffer.size()));

        // Composite children
        self.composite_children(&mut final_buffer);

        final_buffer
    }

    fn calculate_size(&self) -> Size {
        let buffer_size = self.buffer.size();
        let mut max_width = buffer_size.width;
        let mut max_height = buffer_size.height;

        for child in &self.children {
            let child_size = child.buffer.size();
            let child_right = child.frame.origin.x + child_size.width;
            let child_bottom = child.frame.origin.y + child_size.height;
            max_width = max_width.max(child_right);
            max_height = max_height.max(child_bottom);
        }

        Size::new(max_width, max_height)
    }

    fn composite_children(&self, target_buffer: &mut Buffer) {
        for child in &self.children {
            // Blit child at its frame position
            target_buffer.blit_from(&child.buffer, child.frame);

            // Recursively composite child's children
            child.composite_children(target_buffer);
        }
    }
}

/// SceneBuilder collects buffers from RenderObjects and creates a Scene
pub struct SceneBuilder {
    root_layer: Option<Layer>,
}

impl SceneBuilder {
    pub fn new() -> Self {
        Self { root_layer: None }
    }

    /// Build a scene from a RenderObject tree
    pub fn build(&mut self, root: &dyn RenderObject) {
        self.root_layer = Some(self.build_layer(root));
    }

    fn build_layer(&self, render_object: &dyn RenderObject) -> Layer {
        // Get the buffer from the render object
        let buffer = if let Some(buf) = render_object.get_buffer() {
            Buffer::clone(buf)
        } else {
            // Create empty buffer if none exists
            Buffer::new(render_object.frame().size)
        };

        let frame = render_object.frame();

        let mut layer = Layer::new(buffer, frame);

        // Recursively build layers for children
        for child in render_object.children() {
            let child_layer = self.build_layer(child.as_ref());
            layer.children.push(child_layer);
        }

        layer
    }

    /// Get the final composited buffer
    pub fn finalize(&self) -> Option<Buffer> {
        self.root_layer.as_ref().map(|layer| layer.composite())
    }
}

impl Default for SceneBuilder {
    fn default() -> Self {
        Self::new()
    }
}
