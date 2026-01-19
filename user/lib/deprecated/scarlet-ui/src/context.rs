//! Context objects for view methods
//!
//! This module provides the context objects passed to view methods during
//! different phases of the rendering pipeline.
//!
//! # Context Types
//!
//! - `EventCtx`: Passed to event handlers, allows requesting layout/paint
//! - `LayoutCtx`: Passed during layout, provides access to layout utilities
//! - `PaintCtx`: Passed during paint, provides access to canvas and drawing utilities
//! - `UpdateCtx`: Passed when data changes, allows requesting redraw

use crate::event::Event;
use crate::graphics::Canvas;
use crate::view::id::ViewId;
use crate::view::tracker::RenderTracker;
use scarlet_std::vec::Vec;

/// Control flow for event handling
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ControlFlow {
    /// Continue propagating the event
    Continue,
    /// Stop propagating (event was consumed)
    Stop,
}

/// Context passed to event handlers
///
/// EventCtx allows views to:
/// - Request layout recalculation
/// - Request redraw
/// - Access event information
pub struct EventCtx<'a, 'b> {
    view_id: ViewId,
    _event: &'a Event,
    tracker: &'b mut RenderTracker,
    needs_layout: bool,
    needs_paint: bool,
}

impl<'a, 'b> EventCtx<'a, 'b> {
    pub(crate) fn new(view_id: ViewId, event: &'a Event, tracker: &'b mut RenderTracker) -> Self {
        Self {
            view_id,
            _event: event,
            tracker,
            needs_layout: false,
            needs_paint: false,
        }
    }

    /// Request a layout pass for this view
    ///
    /// This will immediately notify the RenderTracker (O(1) operation).
    /// This will recalculate the size and position of this view
    /// and potentially its children.
    pub fn request_layout(&mut self) {
        self.needs_layout = true;
        self.tracker.mark_dirty_layout(self.view_id);
    }

    /// Request a paint pass for this view
    ///
    /// This will immediately notify the RenderTracker (O(1) operation).
    /// This will redraw this view in the next frame.
    pub fn request_paint(&mut self) {
        self.needs_paint = true;
        self.tracker.mark_dirty_paint(self.view_id);
    }

    /// Check if layout was requested
    pub fn needs_layout(&self) -> bool {
        self.needs_layout
    }

    /// Check if paint was requested
    pub fn needs_paint(&self) -> bool {
        self.needs_paint
    }

    /// Get the view ID
    pub fn view_id(&self) -> ViewId {
        self.view_id
    }

    /// Get the RenderTracker
    pub fn tracker(&mut self) -> &mut RenderTracker {
        self.tracker
    }
}

/// Context passed during layout phase
///
/// LayoutCtx provides utilities for layout calculations and
/// allows views to access information about their environment.
pub struct LayoutCtx<'a> {
    view_id: ViewId,
    // Add layout utilities here as needed
    // e.g., access to text measurement, etc.
    _phantom: core::marker::PhantomData<&'a ()>,
}

impl<'a> LayoutCtx<'a> {
    pub(crate) fn new(view_id: ViewId) -> Self {
        Self {
            view_id,
            _phantom: core::marker::PhantomData,
        }
    }

    /// Get the view ID
    pub fn view_id(&self) -> ViewId {
        self.view_id
    }
}

/// Context passed during paint phase
///
/// PaintCtx provides access to the canvas and drawing utilities.
/// Views use this to render themselves.
pub struct PaintCtx<'a> {
    /// The canvas to draw on
    pub canvas: &'a mut Canvas<'a>,
    /// Clipping region (if set)
    clip_rect: Option<crate::graphics::Rect>,
    /// Dirty regions that need redrawing
    dirty_regions: Vec<crate::graphics::Rect>,
    view_id: ViewId,
}

impl<'a> PaintCtx<'a> {
    pub(crate) fn new(
        canvas: &'a mut Canvas<'a>,
        view_id: ViewId,
    ) -> Self {
        Self {
            canvas,
            clip_rect: None,
            dirty_regions: Vec::new(),
            view_id,
        }
    }

    /// Set clipping region
    ///
    /// After calling this, all drawing will be clipped to the given rectangle.
    pub fn clip(&mut self, rect: crate::graphics::Rect) {
        self.clip_rect = Some(rect);
    }

    /// Clear clipping region
    pub fn clear_clip(&mut self) {
        self.clip_rect = None;
    }

    /// Add a dirty region
    ///
    /// This marks a region as needing redraw.
    pub fn add_dirty_region(&mut self, rect: crate::graphics::Rect) {
        self.dirty_regions.push(rect);
    }

    /// Get the dirty regions
    pub fn dirty_regions(&self) -> &[crate::graphics::Rect] {
        &self.dirty_regions
    }

    /// Check if a region intersects any dirty region
    pub fn should_draw(&self, rect: crate::graphics::Rect) -> bool {
        if self.dirty_regions.is_empty() {
            return true; // No dirty regions, draw everything
        }

        self.dirty_regions.iter().any(|dirty| {
            // Check if rectangles intersect
            let x_overlap = rect.x < dirty.x + dirty.width as i32
                && dirty.x < rect.x + rect.width as i32;
            let y_overlap = rect.y < dirty.y + dirty.height as i32
                && dirty.y < rect.y + rect.height as i32;
            x_overlap && y_overlap
        })
    }

    /// Get the view ID
    pub fn view_id(&self) -> ViewId {
        self.view_id
    }
}

/// Context passed when data changes
///
/// UpdateCtx allows views to request layout or paint when their
/// observed data changes.
pub struct UpdateCtx<'a, 'b> {
    view_id: ViewId,
    data_version: u64,
    tracker: &'b mut RenderTracker,
    needs_layout: bool,
    needs_paint: bool,
    _phantom: core::marker::PhantomData<&'a ()>,
}

impl<'a, 'b> UpdateCtx<'a, 'b> {
    pub(crate) fn new(view_id: ViewId, data_version: u64, tracker: &'b mut RenderTracker) -> Self {
        Self {
            view_id,
            data_version,
            tracker,
            needs_layout: false,
            needs_paint: true, // Default to paint on data change
            _phantom: core::marker::PhantomData,
        }
    }

    /// Request a layout pass
    ///
    /// This will immediately notify the RenderTracker (O(1) operation).
    pub fn request_layout(&mut self) {
        self.needs_layout = true;
        self.tracker.mark_dirty_layout(self.view_id);
    }

    /// Request a paint pass
    ///
    /// This will immediately notify the RenderTracker (O(1) operation).
    pub fn request_paint(&mut self) {
        self.needs_paint = true;
        self.tracker.mark_dirty_paint(self.view_id);
    }

    /// Check if layout was requested
    pub fn needs_layout(&self) -> bool {
        self.needs_layout
    }

    /// Check if paint was requested
    pub fn needs_paint(&self) -> bool {
        self.needs_paint
    }

    /// Get the data version (for change detection)
    pub fn data_version(&self) -> u64 {
        self.data_version
    }

    /// Get the view ID
    pub fn view_id(&self) -> ViewId {
        self.view_id
    }

    /// Get the RenderTracker
    pub fn tracker(&mut self) -> &mut RenderTracker {
        self.tracker
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_ctx_requests() {
        let view_id = ViewId::new();
        let event = Event::mouse_move(10, 20);
        let mut ctx = EventCtx::new(view_id, &event);

        assert!(!ctx.needs_layout());
        assert!(!ctx.needs_paint());

        ctx.request_layout();
        assert!(ctx.needs_layout());

        ctx.request_paint();
        assert!(ctx.needs_paint());
    }

    #[test]
    fn test_control_flow() {
        let flow = ControlFlow::Continue;
        assert_eq!(flow, ControlFlow::Continue);

        let flow = ControlFlow::Stop;
        assert_eq!(flow, ControlFlow::Stop);
    }
}
