//! Compositor - Composites RenderObject trees into window buffers
//!
//! The Compositor traverses the element tree and composites all buffers
//! into a single window buffer.

use crate::buffer::Buffer;
use crate::geometry::{Point, Size};
use crate::color::Color;
use crate::render::RenderObject;

/// Compositor for rendering element trees to buffers
pub struct Compositor {
    window_buffer: Buffer,
}

impl Compositor {
    /// Create a new compositor with the given window size
    pub fn new(window_size: Size) -> Self {
        Self {
            window_buffer: Buffer::new(window_size),
        }
    }

    /// Clear the window buffer with a color
    pub fn clear(&mut self, color: Color) {
        let pixel = color.to_bgra();
        for px in self.window_buffer.as_mut_slice() {
            *px = pixel;
        }
    }

    /// Composite a RenderObject tree into the window buffer
    ///
    /// This traverses the tree depth-first and composites all buffers.
    pub fn composite_tree(&mut self, root: &dyn RenderObject) {
        scarlet_std::println!("[Compositor] composite_tree: window_size={:?}x{:?}",
            self.window_buffer.width(), self.window_buffer.height());

        // Clear background
        self.clear(Color::WHITE);

        // Composite from root
        self.composite_node(root, Point::ZERO);

        scarlet_std::println!("[Compositor] composite_tree: complete");
    }

    /// Composite a single RenderObject node
    fn composite_node(&mut self, node: &dyn RenderObject, origin: Point) {
        let frame = node.frame();
        let absolute_origin = Point {
            x: origin.x + frame.origin.x,
            y: origin.y + frame.origin.y,
        };

        // Process children first (for proper z-ordering)
        // Children are composited before parent (painter's algorithm)
        for child in node.children() {
            self.composite_node(child.as_ref(), absolute_origin);
        }

        // Composite this node's buffer if it has one
        if let Some(buffer) = node.get_buffer() {
            let opacity = node.opacity();
            scarlet_std::println!("[Compositor] composite_node: origin={:?}, buffer_size={}x{}, opacity={}",
                absolute_origin, buffer.width(), buffer.height(), opacity);

            self.window_buffer.composite(
                buffer,
                absolute_origin.x as i32,
                absolute_origin.y as i32,
                opacity,
            );
        }
    }

    /// Resize the window buffer
    pub fn resize(&mut self, new_size: Size) {
        self.window_buffer = Buffer::new(new_size);
    }

    /// Get the window buffer
    pub fn window_buffer(&self) -> &Buffer {
        &self.window_buffer
    }

    /// Get mutable access to the window buffer
    pub fn window_buffer_mut(&mut self) -> &mut Buffer {
        &mut self.window_buffer
    }
}
