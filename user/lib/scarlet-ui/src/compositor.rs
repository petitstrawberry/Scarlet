//! Compositor - Composites RenderTree into window buffers
//!
//! The Compositor traverses the RenderTree (derived from the Element tree)
//! and composites all buffers into a single window buffer.

use crate::buffer::Buffer;
use crate::geometry::{Point, Rect, Size};
use crate::color::Color;
use crate::element::{Element, ElementId};
use crate::render::{RenderNode, RenderTree};
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;

#[derive(Clone, Copy, Debug)]
struct ClipRegion {
    rect: Rect,
    radius: f32,
}

/// Compositor for rendering element trees to buffers
pub struct Compositor {
    window_buffer: Buffer,
    scale_milli: u32,
    last_bounds: BTreeMap<ElementId, Rect>,
}

impl Compositor {
    /// Create a new compositor with the given window size
    pub fn new(window_size: Size, scale_milli: u32) -> Self {
        Self {
            window_buffer: Buffer::from_logical_dimensions_with_scale(
                libm::ceilf(window_size.width.max(1.0)) as u32,
                libm::ceilf(window_size.height.max(1.0)) as u32,
                scale_milli,
            ),
            scale_milli: scale_milli.max(1),
            last_bounds: BTreeMap::new(),
        }
    }

    fn scale_pos(&self, value: f32) -> i32 {
        libm::floorf(value * self.scale_milli as f32 / 1000.0) as i32
    }

    fn scale_len(&self, value: f32) -> i32 {
        libm::ceilf(value.max(0.0) * self.scale_milli as f32 / 1000.0) as i32
    }

    fn element_paint_bounds(&self, element: &dyn Element, absolute_origin: Point) -> Rect {
        let bounds = element.bounds();
        let mut width = bounds.size.width;
        let mut height = bounds.size.height;
        if let Some(buffer) = element.get_buffer() {
            width = width.max(buffer.logical_width() as f32);
            height = height.max(buffer.logical_height() as f32);
        }
        Rect::from_xywh(absolute_origin.x, absolute_origin.y, width, height)
    }

    fn is_expanded_select(&self, element: &dyn Element) -> bool {
        element
            .render_object()
            .and_then(|render_object| {
                render_object
                    .as_any()
                    .downcast_ref::<crate::views::SelectRenderObject>()
            })
            .is_some_and(|select| select.is_expanded())
    }

    /// Set the output scale and recreate the physical window buffer.
    pub fn set_scale_milli(&mut self, scale_milli: u32, logical_size: Size) {
        self.scale_milli = scale_milli.max(1);
        self.resize(logical_size);
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
            crate::logln!("[Compositor] composite_tree: window_size={:?}x{:?}",
                self.window_buffer.width(), self.window_buffer.height());
        }

        // Clear background
        self.clear(Color::WHITE);

        // Composite from root
        self.composite_node(tree.root(), Point::ZERO);

        if crate::debug::is_enabled() {
            crate::logln!("[Compositor] composite_tree: complete");
        }
    }

    /// Composite an Element tree into the window buffer using dirty rectangles.
    pub fn composite_elements_with_dirty(&mut self, root: &dyn Element, dirty_ids: &[ElementId]) {
        if dirty_ids.is_empty() {
            self.composite_elements(root);
            return;
        }

        let dirty_set: BTreeSet<ElementId> = dirty_ids.iter().copied().collect();
        let mut rects = Vec::new();
        self.collect_dirty_rects_element(root, Point::ZERO, &dirty_set, &mut rects);

        if rects.is_empty() {
            return;
        }

        self.merge_overlapping_rects(&mut rects);

        let window_area = (self.window_buffer.logical_width() as f32)
            * (self.window_buffer.logical_height() as f32);
        let dirty_area: f32 = rects.iter().map(|r| r.size.width * r.size.height).sum();
        if dirty_area >= window_area * 0.6 {
            self.composite_elements(root);
            return;
        }

        for rect in rects.iter() {
            self.clear_rect(*rect, Color::WHITE);
        }

        self.composite_element_clipped(root, Point::ZERO, &rects);
        self.composite_select_overlays_clipped(root, Point::ZERO, &rects, None);
    }

    /// Composite an Element tree into the window buffer.
    pub fn composite_elements(&mut self, root: &dyn Element) {
        if crate::debug::is_enabled() {
            crate::logln!("[Compositor] composite_elements: window_size={:?}x{:?}",
                self.window_buffer.width(), self.window_buffer.height());
        }

        self.clear(Color::WHITE);
        self.composite_element_with_clip(root, Point::ZERO, None);
        self.composite_select_overlays(root, Point::ZERO, None);
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

        let window_area = (self.window_buffer.logical_width() as f32)
            * (self.window_buffer.logical_height() as f32);
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
            crate::logln!(
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
                    crate::logln!("[Compositor] composite_node: origin={:?}, buffer_size={}x{}, opacity={}",
                        absolute_origin, buffer.width(), buffer.height(), opacity);
                }

                self.window_buffer.composite(
                    buffer,
                    self.scale_pos(absolute_origin.x),
                    self.scale_pos(absolute_origin.y),
                    opacity,
                );
            }
        }

        // Composite children after parent so they appear on top
        for child in node.children() {
            self.composite_node(child, absolute_origin);
        }
    }

    fn composite_element(&mut self, element: &dyn Element, origin: Point) {
        self.composite_element_with_clip(element, origin, None);
    }

    fn composite_element_clipped(&mut self, element: &dyn Element, origin: Point, dirty_rects: &[Rect]) {
        self.composite_element_with_clip_dirty(element, origin, dirty_rects, None);
    }

    fn composite_element_with_clip(
        &mut self,
        element: &dyn Element,
        origin: Point,
        clip: Option<ClipRegion>,
    ) {
        let position = element.position();
        let absolute_origin = Point {
            x: origin.x + position.x,
            y: origin.y + position.y,
        };
        let bounds = element.bounds();
        let absolute_bounds = Rect::from_xywh(
            absolute_origin.x,
            absolute_origin.y,
            bounds.size.width,
            bounds.size.height,
        );
        let paint_bounds = self.element_paint_bounds(element, absolute_origin);
        self.last_bounds.insert(element.id(), paint_bounds);

        let mut next_clip = clip;
        if let Some(render_object) = element.render_object() {
            if let Some(clip_render) = render_object
                .as_any()
                .downcast_ref::<crate::views::modifiers::ClipRenderObject>()
            {
                let current = ClipRegion {
                    rect: absolute_bounds,
                    radius: clip_render.radius(),
                };
                next_clip = match next_clip {
                    Some(existing) => self.intersect_clip(existing, current),
                    None => Some(current),
                };
                if next_clip.is_none() {
                    return;
                }
            }
        }

        if let Some(buffer) = element.get_buffer() {
            let opacity = 1.0;
            if let Some(active_clip) = next_clip {
                let (x, y, w, h) = self.rect_to_i32(active_clip.rect);
                self.window_buffer.composite_clipped_rounded(
                    buffer,
                    self.scale_pos(absolute_origin.x),
                    self.scale_pos(absolute_origin.y),
                    opacity,
                    x,
                    y,
                    w,
                    h,
                    self.scale_len(active_clip.radius) as f32,
                );
            } else {
                self.window_buffer.composite(
                    buffer,
                    self.scale_pos(absolute_origin.x),
                    self.scale_pos(absolute_origin.y),
                    opacity,
                );
            }
        }

        for child in element.children() {
            self.composite_element_with_clip(child.as_ref(), absolute_origin, next_clip);
        }
    }

    fn composite_element_with_clip_dirty(
        &mut self,
        element: &dyn Element,
        origin: Point,
        dirty_rects: &[Rect],
        clip: Option<ClipRegion>,
    ) {
        let position = element.position();
        let absolute_origin = Point {
            x: origin.x + position.x,
            y: origin.y + position.y,
        };
        let bounds = element.bounds();
        let absolute_bounds = Rect::from_xywh(
            absolute_origin.x,
            absolute_origin.y,
            bounds.size.width,
            bounds.size.height,
        );
        let paint_bounds = self.element_paint_bounds(element, absolute_origin);
        self.last_bounds.insert(element.id(), paint_bounds);

        if !self.overlaps_any(paint_bounds, dirty_rects) {
            return;
        }

        let mut next_clip = clip;
        if let Some(render_object) = element.render_object() {
            if let Some(clip_render) = render_object
                .as_any()
                .downcast_ref::<crate::views::modifiers::ClipRenderObject>()
            {
                let current = ClipRegion {
                    rect: absolute_bounds,
                    radius: clip_render.radius(),
                };
                next_clip = match next_clip {
                    Some(existing) => self.intersect_clip(existing, current),
                    None => Some(current),
                };
                if next_clip.is_none() {
                    return;
                }
            }
        }

        if let Some(buffer) = element.get_buffer() {
            let opacity = 1.0;
            for rect in dirty_rects.iter() {
                if !paint_bounds.overlaps(rect) {
                    continue;
                }
                let mut clip_rect = *rect;
                let mut clip_radius = 0.0;
                if let Some(active_clip) = next_clip {
                    if let Some(intersection) = self.intersect_rect(clip_rect, active_clip.rect) {
                        clip_rect = intersection;
                        clip_radius = active_clip.radius;
                    } else {
                        continue;
                    }
                }
                let (x, y, w, h) = self.rect_to_i32(clip_rect);
                if clip_radius > 0.0 {
                    self.window_buffer.composite_clipped_rounded(
                        buffer,
                        self.scale_pos(absolute_origin.x),
                        self.scale_pos(absolute_origin.y),
                        opacity,
                        x,
                        y,
                        w,
                        h,
                        self.scale_len(clip_radius) as f32,
                    );
                } else {
                    self.window_buffer.composite_clipped(
                        buffer,
                        self.scale_pos(absolute_origin.x),
                        self.scale_pos(absolute_origin.y),
                        opacity,
                        x,
                        y,
                        w,
                        h,
                    );
                }
            }
        }

        for child in element.children() {
            self.composite_element_with_clip_dirty(child.as_ref(), absolute_origin, dirty_rects, next_clip);
        }
    }

    fn composite_select_overlays(
        &mut self,
        element: &dyn Element,
        origin: Point,
        clip: Option<ClipRegion>,
    ) {
        let position = element.position();
        let absolute_origin = Point {
            x: origin.x + position.x,
            y: origin.y + position.y,
        };
        let bounds = element.bounds();
        let absolute_bounds = Rect::from_xywh(
            absolute_origin.x,
            absolute_origin.y,
            bounds.size.width,
            bounds.size.height,
        );

        let mut next_clip = clip;
        if let Some(render_object) = element.render_object() {
            if let Some(clip_render) = render_object
                .as_any()
                .downcast_ref::<crate::views::modifiers::ClipRenderObject>()
            {
                let current = ClipRegion {
                    rect: absolute_bounds,
                    radius: clip_render.radius(),
                };
                next_clip = match next_clip {
                    Some(existing) => self.intersect_clip(existing, current),
                    None => Some(current),
                };
                if next_clip.is_none() {
                    return;
                }
            }
        }

        for child in element.children() {
            self.composite_select_overlays(child.as_ref(), absolute_origin, next_clip);
        }

        if self.is_expanded_select(element) {
            if let Some(buffer) = element.get_buffer() {
                let opacity = 1.0;
                if let Some(active_clip) = next_clip {
                    let paint_bounds = self.element_paint_bounds(element, absolute_origin);
                    if let Some(clip_rect) = self.intersect_rect(paint_bounds, active_clip.rect) {
                        let (x, y, w, h) = self.rect_to_i32(clip_rect);
                        self.window_buffer.composite_clipped_rounded(
                            buffer,
                            self.scale_pos(absolute_origin.x),
                            self.scale_pos(absolute_origin.y),
                            opacity,
                            x,
                            y,
                            w,
                            h,
                            self.scale_len(active_clip.radius) as f32,
                        );
                    }
                } else {
                    self.window_buffer.composite(
                        buffer,
                        self.scale_pos(absolute_origin.x),
                        self.scale_pos(absolute_origin.y),
                        opacity,
                    );
                }
            }
        }
    }

    fn composite_select_overlays_clipped(
        &mut self,
        element: &dyn Element,
        origin: Point,
        dirty_rects: &[Rect],
        clip: Option<ClipRegion>,
    ) {
        let position = element.position();
        let absolute_origin = Point {
            x: origin.x + position.x,
            y: origin.y + position.y,
        };
        let bounds = element.bounds();
        let absolute_bounds = Rect::from_xywh(
            absolute_origin.x,
            absolute_origin.y,
            bounds.size.width,
            bounds.size.height,
        );

        let mut next_clip = clip;
        if let Some(render_object) = element.render_object() {
            if let Some(clip_render) = render_object
                .as_any()
                .downcast_ref::<crate::views::modifiers::ClipRenderObject>()
            {
                let current = ClipRegion {
                    rect: absolute_bounds,
                    radius: clip_render.radius(),
                };
                next_clip = match next_clip {
                    Some(existing) => self.intersect_clip(existing, current),
                    None => Some(current),
                };
                if next_clip.is_none() {
                    return;
                }
            }
        }

        for child in element.children() {
            self.composite_select_overlays_clipped(
                child.as_ref(),
                absolute_origin,
                dirty_rects,
                next_clip,
            );
        }

        if self.is_expanded_select(element) {
            let paint_bounds = self.element_paint_bounds(element, absolute_origin);
            if !self.overlaps_any(paint_bounds, dirty_rects) {
                return;
            }

            if let Some(buffer) = element.get_buffer() {
                let opacity = 1.0;
                for rect in dirty_rects.iter() {
                    if !paint_bounds.overlaps(rect) {
                        continue;
                    }

                    let mut clip_rect = *rect;
                    let mut clip_radius = 0.0;
                    if let Some(active_clip) = next_clip {
                        if let Some(intersection) =
                            self.intersect_rect(clip_rect, active_clip.rect)
                        {
                            clip_rect = intersection;
                            clip_radius = active_clip.radius;
                        } else {
                            continue;
                        }
                    }
                    let (x, y, w, h) = self.rect_to_i32(clip_rect);
                    if clip_radius > 0.0 {
                        self.window_buffer.composite_clipped_rounded(
                            buffer,
                            self.scale_pos(absolute_origin.x),
                            self.scale_pos(absolute_origin.y),
                            opacity,
                            x,
                            y,
                            w,
                            h,
                            self.scale_len(clip_radius) as f32,
                        );
                    } else {
                        self.window_buffer.composite_clipped(
                            buffer,
                            self.scale_pos(absolute_origin.x),
                            self.scale_pos(absolute_origin.y),
                            opacity,
                            x,
                            y,
                            w,
                            h,
                        );
                    }
                }
            }
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
                            self.scale_pos(absolute_origin.x),
                            self.scale_pos(absolute_origin.y),
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

    fn intersect_clip(&self, a: ClipRegion, b: ClipRegion) -> Option<ClipRegion> {
        let rect = self.intersect_rect(a.rect, b.rect)?;
        let radius = if a.radius <= 0.0 {
            b.radius
        } else if b.radius <= 0.0 {
            a.radius
        } else {
            a.radius.min(b.radius)
        };
        Some(ClipRegion { rect, radius })
    }

    fn intersect_rect(&self, a: Rect, b: Rect) -> Option<Rect> {
        let left = a.left().max(b.left());
        let top = a.top().max(b.top());
        let right = a.right().min(b.right());
        let bottom = a.bottom().min(b.bottom());
        if right <= left || bottom <= top {
            return None;
        }
        Some(Rect::from_xywh(left, top, right - left, bottom - top))
    }

    fn collect_dirty_rects_element(
        &self,
        element: &dyn Element,
        origin: Point,
        dirty_ids: &BTreeSet<ElementId>,
        rects: &mut Vec<Rect>,
    ) {
        let position = element.position();
        let absolute_origin = Point {
            x: origin.x + position.x,
            y: origin.y + position.y,
        };

        if dirty_ids.contains(&element.id()) {
            rects.push(self.element_paint_bounds(element, absolute_origin));
            if let Some(old_bounds) = self.last_bounds.get(&element.id()) {
                rects.push(*old_bounds);
            }
        }

        for child in element.children() {
            self.collect_dirty_rects_element(child.as_ref(), absolute_origin, dirty_ids, rects);
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
        let x0 = libm::floorf(rect.origin.x * self.scale_milli as f32 / 1000.0).max(0.0);
        let y0 = libm::floorf(rect.origin.y * self.scale_milli as f32 / 1000.0).max(0.0);
        let x1 = libm::ceilf((rect.origin.x + rect.size.width) * self.scale_milli as f32 / 1000.0)
            .min(self.window_buffer.width() as f32);
        let y1 = libm::ceilf((rect.origin.y + rect.size.height) * self.scale_milli as f32 / 1000.0)
            .min(self.window_buffer.height() as f32);
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
        self.window_buffer = Buffer::from_logical_dimensions_with_scale(
            libm::ceilf(new_size.width.max(1.0)) as u32,
            libm::ceilf(new_size.height.max(1.0)) as u32,
            self.scale_milli,
        );
        self.last_bounds.clear();
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
