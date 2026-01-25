//! Compositor - Composites RenderTree into window buffers
//!
//! The Compositor traverses the RenderTree (derived from the Element tree)
//! and composites all buffers into a single window buffer.

use crate::buffer::Buffer;
use crate::geometry::{Point, Size};
use crate::color::Color;
use crate::render::{RenderNode, RenderTree};

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

    /// Composite a RenderTree into the window buffer
    ///
    /// This traverses the tree depth-first and composites all buffers.
    pub fn composite_tree(&mut self, tree: &RenderTree) {
        if crate::debug::is_enabled() {
            scarlet_std::println!("[Compositor] composite_tree: window_size={:?}x{:?}",
                self.window_buffer.width(), self.window_buffer.height());
        }

        // Clear background
        self.clear(Color::WHITE);

        // Composite from root
        self.composite_node(tree.root(), Point::ZERO);

        if crate::debug::is_enabled() {
            scarlet_std::println!("[Compositor] composite_tree: complete");
        }
    }

    /// Composite a single RenderNode
    fn composite_node(&mut self, node: &RenderNode, origin: Point) {
        let position = node.position();
        let absolute_origin = Point {
            x: origin.x + position.x,
            y: origin.y + position.y,
        };

        let render_object = node.render_object();
        let has_buffer = render_object.and_then(|ro| ro.get_buffer()).is_some();
        if crate::debug::is_enabled() {
            scarlet_std::println!(
                "[Compositor] visiting node id={} origin=({}, {}) local=({}, {}) buffer={}",
                node.id().get(),
                absolute_origin.x,
                absolute_origin.y,
                position.x,
                position.y,
                has_buffer
            );
        }

        // Composite this node's buffer if it has one
        if let Some(render_object) = render_object {
            if let Some(buffer) = render_object.get_buffer() {
                let opacity = 1.0;
                if crate::debug::is_enabled() {
                    scarlet_std::println!("[Compositor] composite_node: origin={:?}, buffer_size={}x{}, opacity={}",
                        absolute_origin, buffer.width(), buffer.height(), opacity);
                }

                self.window_buffer.composite(
                    buffer,
                    absolute_origin.x as i32,
                    absolute_origin.y as i32,
                    opacity,
                );
            }
        }

        // Composite children after parent so they appear on top
        for child in node.children() {
            self.composite_node(child, absolute_origin);
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
