//! Compositor - Composites RenderTree into window buffers
//!
//! The Compositor traverses the RenderTree (derived from the Element tree)
//! and composites all buffers into a single window buffer.

use crate::buffer::Buffer;
use crate::geometry::{Point, Rect, Size};
use crate::color::Color;
use crate::render::{RenderNode, RenderTree};
use crate::element::ElementId;
use alloc::collections::BTreeSet;
use alloc::vec::Vec;

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

    /// Composite a RenderTree into the window buffer using dirty rectangles.
    pub fn composite_tree_with_dirty(&mut self, tree: &RenderTree, dirty_ids: &[ElementId]) {
        if dirty_ids.is_empty() {
            self.composite_tree(tree);
            return;
        }

        let dirty_set: BTreeSet<ElementId> = dirty_ids.iter().copied().collect();
        let mut rects = Vec::new();
        let mut fallback_full = false;
        self.collect_dirty_rects(tree.root(), Point::ZERO, &dirty_set, &mut rects, &mut fallback_full);

        if fallback_full || rects.is_empty() {
            self.composite_tree(tree);
            return;
        }

        self.merge_overlapping_rects(&mut rects);

        let window_area = (self.window_buffer.width() as f32) * (self.window_buffer.height() as f32);
        let dirty_area: f32 = rects.iter().map(|r| r.size.width * r.size.height).sum();
        if dirty_area >= window_area * 0.6 {
            self.composite_tree(tree);
            return;
        }

        // Clear only dirty regions.
        for rect in rects.iter() {
            self.clear_rect(*rect, Color::WHITE);
        }

        self.composite_node_clipped(tree.root(), Point::ZERO, &rects);
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

    fn composite_node_clipped(&mut self, node: &RenderNode, origin: Point, dirty_rects: &[Rect]) {
        let position = node.position();
        let absolute_origin = Point {
            x: origin.x + position.x,
            y: origin.y + position.y,
        };

        let render_object = node.render_object();
        if let Some(render_object) = render_object {
            let size = render_object.size();
            let bounds = Rect::from_xywh(absolute_origin.x, absolute_origin.y, size.width, size.height);
            if !self.overlaps_any(bounds, dirty_rects) {
                return;
            }

            if let Some(buffer) = render_object.get_buffer() {
                let opacity = 1.0;
                for rect in dirty_rects.iter() {
                    if bounds.overlaps(rect) {
                        let (x, y, w, h) = self.rect_to_i32(*rect);
                        self.window_buffer.composite_clipped(
                            buffer,
                            absolute_origin.x as i32,
                            absolute_origin.y as i32,
                            opacity,
                            x,
                            y,
                            w,
                            h,
                        );
                    }
                }
            }
        } else {
            // No render object (e.g., root), still visit children.
        }

        for child in node.children() {
            self.composite_node_clipped(child, absolute_origin, dirty_rects);
        }
    }

    fn collect_dirty_rects(
        &self,
        node: &RenderNode,
        origin: Point,
        dirty_ids: &BTreeSet<ElementId>,
        rects: &mut Vec<Rect>,
        fallback_full: &mut bool,
    ) {
        if *fallback_full {
            return;
        }

        let position = node.position();
        let absolute_origin = Point {
            x: origin.x + position.x,
            y: origin.y + position.y,
        };

        if dirty_ids.contains(&node.id()) {
            if let Some(render_object) = node.render_object() {
                if render_object.get_buffer().is_some() {
                    let size = render_object.size();
                    rects.push(Rect::from_xywh(absolute_origin.x, absolute_origin.y, size.width, size.height));
                } else {
                    *fallback_full = true;
                    return;
                }
            } else {
                *fallback_full = true;
                return;
            }
        }

        for child in node.children() {
            self.collect_dirty_rects(child, absolute_origin, dirty_ids, rects, fallback_full);
            if *fallback_full {
                return;
            }
        }
    }

    fn clear_rect(&mut self, rect: Rect, color: Color) {
        let (x, y, w, h) = self.rect_to_u32(rect);
        self.window_buffer.clear_rect(x, y, w, h, color);
    }

    fn rect_to_u32(&self, rect: Rect) -> (u32, u32, u32, u32) {
        let x0 = libm::floorf(rect.origin.x).max(0.0);
        let y0 = libm::floorf(rect.origin.y).max(0.0);
        let x1 = libm::ceilf(rect.origin.x + rect.size.width).min(self.window_buffer.width() as f32);
        let y1 = libm::ceilf(rect.origin.y + rect.size.height).min(self.window_buffer.height() as f32);
        let w = (x1 - x0).max(0.0);
        let h = (y1 - y0).max(0.0);
        (x0 as u32, y0 as u32, w as u32, h as u32)
    }

    fn rect_to_i32(&self, rect: Rect) -> (i32, i32, i32, i32) {
        let (x, y, w, h) = self.rect_to_u32(rect);
        (x as i32, y as i32, w as i32, h as i32)
    }

    fn overlaps_any(&self, rect: Rect, rects: &[Rect]) -> bool {
        rects.iter().any(|r| rect.overlaps(r))
    }

    fn merge_overlapping_rects(&self, rects: &mut Vec<Rect>) {
        let mut merged: Vec<Rect> = Vec::new();
        'outer: for rect in rects.drain(..) {
            for existing in merged.iter_mut() {
                if existing.overlaps(&rect) {
                    let left = existing.left().min(rect.left());
                    let top = existing.top().min(rect.top());
                    let right = existing.right().max(rect.right());
                    let bottom = existing.bottom().max(rect.bottom());
                    *existing = Rect::from_xywh(left, top, right - left, bottom - top);
                    continue 'outer;
                }
            }
            merged.push(rect);
        }
        *rects = merged;
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
