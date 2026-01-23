use std::vec::Vec;

use crate::buffer::Buffer;
use crate::geometry::{Color, Point, Rect, Size};
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

        // Fill with opaque background (e.g., dark gray or black) to avoid transparency
        let bg_color: [u8; 4] = [32, 32, 32, 255]; // Dark gray with full opacity
        final_buffer.fill_rect(Rect::new(Point::ZERO, total_size), bg_color);

        // Composite self buffer at frame position (skip placeholder buffers)
        if self.buffer.width() > 1 || self.buffer.height() > 1 {
            self.composite_buffer(&mut final_buffer, &self.buffer, self.frame);
        }

        // Composite children
        self.composite_children(&mut final_buffer);

        final_buffer
    }

    /// Composite a single buffer into target at the specified position
    fn composite_buffer(&self, target: &mut Buffer, src: &Buffer, dest_frame: Rect) {
        let src_width = src.width();
        let src_height = src.height();
        let src_data = src.as_slice();

        // Get target dimensions before mutable borrow
        let target_width = target.width();
        let target_height = target.height();
        let target_stride = target.stride();

        let dest_data = target.as_mut_slice();

        let dest_x = libm::ceilf(dest_frame.origin.x) as usize;
        let dest_y = libm::ceilf(dest_frame.origin.y) as usize;
        let dest_width = libm::ceilf(dest_frame.size.width) as usize;
        let dest_height = libm::ceilf(dest_frame.size.height) as usize;

        // Clamp to buffer bounds
        let dest_x = dest_x.clamp(0, target_width);
        let dest_y = dest_y.clamp(0, target_height);

        let copy_width = dest_width
            .min(src_width)
            .min(target_width - dest_x);
        let copy_height = dest_height
            .min(src_height)
            .min(target_height - dest_y);

        for y in 0..copy_height {
            for x in 0..copy_width {
                let src_offset = y * src.stride() + x * 4;
                let dest_offset = (dest_y + y) * target_stride + (dest_x + x) * 4;

                let src_b = src_data[src_offset];
                let src_g = src_data[src_offset + 1];
                let src_r = src_data[src_offset + 2];
                let src_a = src_data[src_offset + 3];

                // Alpha blending
                if src_a == 255 {
                    // Opaque: copy directly
                    dest_data[dest_offset] = src_b;
                    dest_data[dest_offset + 1] = src_g;
                    dest_data[dest_offset + 2] = src_r;
                    dest_data[dest_offset + 3] = src_a;
                } else if src_a > 0 {
                    // Semi-transparent: blend with destination
                    let dst_a = dest_data[dest_offset + 3];

                    if dst_a == 0 {
                        // Destination is fully transparent, just copy source
                        dest_data[dest_offset] = src_b;
                        dest_data[dest_offset + 1] = src_g;
                        dest_data[dest_offset + 2] = src_r;
                        dest_data[dest_offset + 3] = src_a;
                    } else {
                        // Both have some alpha, proper over compositing
                        let src_a_f = src_a as f32 / 255.0;
                        let dst_a_f = dst_a as f32 / 255.0;

                        // Final alpha (over operator)
                        let out_a_f = src_a_f + dst_a_f * (1.0 - src_a_f);
                        let out_a = (out_a_f * 255.0).min(255.0) as u8;

                        // Blend colors
                        let src_b_f = src_b as f32;
                        let src_g_f = src_g as f32;
                        let src_r_f = src_r as f32;

                        let dst_b_f = dest_data[dest_offset] as f32;
                        let dst_g_f = dest_data[dest_offset + 1] as f32;
                        let dst_r_f = dest_data[dest_offset + 2] as f32;

                        let out_b = (src_b_f * src_a_f + dst_b_f * dst_a_f * (1.0 - src_a_f)) / out_a_f.max(0.01);
                        let out_g = (src_g_f * src_a_f + dst_g_f * dst_a_f * (1.0 - src_a_f)) / out_a_f.max(0.01);
                        let out_r = (src_r_f * src_a_f + dst_r_f * dst_a_f * (1.0 - src_a_f)) / out_a_f.max(0.01);

                        dest_data[dest_offset] = out_b.min(255.0) as u8;
                        dest_data[dest_offset + 1] = out_g.min(255.0) as u8;
                        dest_data[dest_offset + 2] = out_r.min(255.0) as u8;
                        dest_data[dest_offset + 3] = out_a;
                    }
                }
                // If src_a == 0, keep destination pixel unchanged
            }
        }
    }

    fn calculate_size(&self) -> Size {
        let mut max_width = 0.0;
        let mut max_height = 0.0;

        // Only include self if it's not a placeholder buffer
        if self.buffer.width() > 1 || self.buffer.height() > 1 {
            let buffer_size = self.buffer.size();
            max_width = self.frame.origin.x + buffer_size.width;
            max_height = self.frame.origin.y + buffer_size.height;
        }

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
            // For containers without their own buffer, composite children at correct position
            // child.frame is relative to parent (self.frame), so we need to add parent offset
            let child_frame = Rect::new(
                Point::new(
                    self.frame.origin.x + child.frame.origin.x,
                    self.frame.origin.y + child.frame.origin.y,
                ),
                child.frame.size,
            );
            self.composite_buffer(target_buffer, &child.buffer, child_frame);

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
        // Get buffer if exists, otherwise create minimal placeholder
        let buffer = if let Some(buf) = render_object.get_buffer() {
            Buffer::clone(buf)
        } else {
            // Container without buffer - create minimal placeholder
            // This will be skipped during compositing
            Buffer::new(crate::geometry::Size::new(1.0, 1.0))
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
