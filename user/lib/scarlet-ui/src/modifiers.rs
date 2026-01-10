//! View modifiers for styling and composition
//!
//! Modifiers enable SwiftUI-style method chaining for view customization.
//! Each modifier wraps a view and adds visual styling or behavior.
//!
//! # Example
//!
//! ```no_run
//! use scarlet_ui::{Label, Color, ViewModifier};
//!
//! let styled_label = Label::new("Hello")
//!     .corner_radius(8)
//!     .border(2, Color::WHITE)
//!     .background_color(Color::BLUE);
//! ```
//!
//! # Available Modifiers
//!
//! - [`CornerRadius`] - Adds rounded corners to a view
//! - [`Border`] - Adds a border around a view
//! - [`Background`] - Sets a background color for a view
//!
//! # Creating Custom Modifiers
//!
//! To create a custom modifier, implement the [`View`] trait and wrap
//! the child view in a struct that applies your custom rendering or layout.

use crate::graphics::{Canvas, Rect};
use crate::view::{Size, View};
use crate::Color;
use scarlet_std::boxed::Box;

/// Wrapper that adds rounded corners to a view
///
/// This modifier clips the view's content to rounded corners.
/// It works by masking the corners after drawing the child.
pub struct CornerRadius {
    child: Box<dyn View>,
    radius: u32,
    background_color: Option<Color>,
    cached_size: Size,
}

impl CornerRadius {
    pub fn new<V: View + 'static>(child: V, radius: u32) -> Self {
        Self {
            child: Box::new(child),
            radius,
            background_color: None,
            cached_size: Size::ZERO,
        }
    }

    /// Set a background color for the rounded rectangle
    /// 
    /// If not set, corners are clipped to transparent.
    pub fn background(mut self, color: Color) -> Self {
        self.background_color = Some(color);
        self
    }

    /// Check if a point is inside the rounded rectangle
    fn point_in_rounded_rect(&self, x: i32, y: i32, frame: Rect) -> bool {
        let r = self.radius as i32;
        let fx = frame.x;
        let fy = frame.y;
        let fw = frame.width as i32;
        let fh = frame.height as i32;

        // Check if point is in the main rect
        if x < fx || x >= fx + fw || y < fy || y >= fy + fh {
            return false;
        }

        // Check corners
        // Top-left corner
        if x < fx + r && y < fy + r {
            let dx = x - (fx + r);
            let dy = y - (fy + r);
            return dx * dx + dy * dy <= r * r;
        }

        // Top-right corner
        if x >= fx + fw - r && y < fy + r {
            let dx = x - (fx + fw - r - 1);
            let dy = y - (fy + r);
            return dx * dx + dy * dy <= r * r;
        }

        // Bottom-left corner
        if x < fx + r && y >= fy + fh - r {
            let dx = x - (fx + r);
            let dy = y - (fy + fh - r - 1);
            return dx * dx + dy * dy <= r * r;
        }

        // Bottom-right corner
        if x >= fx + fw - r && y >= fy + fh - r {
            let dx = x - (fx + fw - r - 1);
            let dy = y - (fy + fh - r - 1);
            return dx * dx + dy * dy <= r * r;
        }

        true
    }
}

impl View for CornerRadius {
    fn layout(&mut self, available: Size) -> Size {
        self.cached_size = self.child.layout(available);
        self.cached_size
    }

    fn draw(&self, canvas: &mut Canvas, frame: Rect) {
        // First, draw background with rounded corners if set
        if let Some(bg) = self.background_color {
            canvas.fill_rounded_rect(frame.x, frame.y, frame.width, frame.height, self.radius, bg);
        }

        // Draw child
        self.child.draw(canvas, frame);

        // Mask out corners by drawing over them
        // This is a simple approach - drawing transparent or background color in corners
        let r = self.radius;
        let clear_color = self.background_color.unwrap_or(Color::TRANSPARENT);

        // Only need to mask if we don't have a background (background already handles rounding)
        if self.background_color.is_none() {
            // Mask the four corners
            for py in 0..r {
                for px in 0..r {
                    // Top-left
                    let dx = r as i32 - 1 - px as i32;
                    let dy = r as i32 - 1 - py as i32;
                    if dx * dx + dy * dy > (r * r) as i32 {
                        canvas.put_pixel(frame.x + px as i32, frame.y + py as i32, clear_color);
                    }

                    // Top-right
                    let tx = frame.width - r + px;
                    let dx = px as i32;
                    if dx * dx + dy * dy > (r * r) as i32 {
                        canvas.put_pixel(frame.x + tx as i32, frame.y + py as i32, clear_color);
                    }

                    // Bottom-left
                    let by = frame.height - r + py;
                    let dy = py as i32;
                    let dx = r as i32 - 1 - px as i32;
                    if dx * dx + dy * dy > (r * r) as i32 {
                        canvas.put_pixel(frame.x + px as i32, frame.y + by as i32, clear_color);
                    }

                    // Bottom-right
                    let dx = px as i32;
                    if dx * dx + dy * dy > (r * r) as i32 {
                        canvas.put_pixel(frame.x + tx as i32, frame.y + by as i32, clear_color);
                    }
                }
            }
        }
    }

    fn on_event_capture(&mut self, event: &mut crate::event::Event, frame: Rect) -> bool {
        // Only pass events inside the rounded rect
        if !self.point_in_rounded_rect(event.x(), event.y(), frame) {
            return false;
        }
        self.child.on_event_capture(event, frame)
    }

    fn on_event(&mut self, event: &mut crate::event::Event, frame: Rect) -> bool {
        // Only handle events inside the rounded rect
        if !self.point_in_rounded_rect(event.x(), event.y(), frame) {
            return false;
        }
        self.child.on_event(event, frame)
    }

    fn needs_draw(&self) -> bool {
        self.child.needs_draw()
    }

    fn set_needs_draw(&mut self) {
        self.child.set_needs_draw()
    }
}

/// Wrapper that adds a rounded border to a view
pub struct RoundedBorder {
    child: Box<dyn View>,
    width: u32,
    radius: u32,
    color: Color,
    cached_size: Size,
}

impl RoundedBorder {
    pub fn new<V: View + 'static>(child: V, width: u32, radius: u32, color: Color) -> Self {
        Self {
            child: Box::new(child),
            width,
            radius,
            color,
            cached_size: Size::ZERO,
        }
    }
}

impl View for RoundedBorder {
    fn layout(&mut self, available: Size) -> Size {
        let inner_available = Size::new(
            available.width.saturating_sub(self.width * 2),
            available.height.saturating_sub(self.width * 2),
        );
        let child_size = self.child.layout(inner_available);
        self.cached_size = child_size;
        
        Size::new(
            child_size.width.saturating_add(self.width * 2),
            child_size.height.saturating_add(self.width * 2),
        )
    }

    fn draw(&self, canvas: &mut Canvas, frame: Rect) {
        // Draw child
        let child_frame = Rect::new(
            frame.x + self.width as i32,
            frame.y + self.width as i32,
            self.cached_size.width,
            self.cached_size.height,
        );
        self.child.draw(canvas, child_frame);

        // Draw rounded border
        canvas.stroke_rounded_rect(
            frame.x,
            frame.y,
            frame.width,
            frame.height,
            self.radius,
            self.width,
            self.color,
        );
    }

    fn on_event_capture(&mut self, event: &mut crate::event::Event, frame: Rect) -> bool {
        let child_frame = Rect::new(
            frame.x + self.width as i32,
            frame.y + self.width as i32,
            self.cached_size.width,
            self.cached_size.height,
        );
        self.child.on_event_capture(event, child_frame)
    }

    fn on_event(&mut self, event: &mut crate::event::Event, frame: Rect) -> bool {
        let child_frame = Rect::new(
            frame.x + self.width as i32,
            frame.y + self.width as i32,
            self.cached_size.width,
            self.cached_size.height,
        );
        if child_frame.contains(event.x(), event.y()) {
            self.child.on_event(event, child_frame)
        } else {
            false
        }
    }

    fn needs_draw(&self) -> bool {
        self.child.needs_draw()
    }

    fn set_needs_draw(&mut self) {
        self.child.set_needs_draw()
    }
}

/// Wrapper that adds a border to a view
pub struct Border {
    child: Box<dyn View>,
    width: u32,
    color: Color,
    cached_size: Size,
}

impl Border {
    pub fn new<V: View + 'static>(child: V, width: u32, color: Color) -> Self {
        Self {
            child: Box::new(child),
            width,
            color,
            cached_size: Size::ZERO,
        }
    }
}

impl View for Border {
    fn layout(&mut self, available: Size) -> Size {
        let inner_available = Size::new(
            available.width.saturating_sub(self.width * 2),
            available.height.saturating_sub(self.width * 2),
        );
        let child_size = self.child.layout(inner_available);
        self.cached_size = child_size;
        
        Size::new(
            child_size.width.saturating_add(self.width * 2),
            child_size.height.saturating_add(self.width * 2),
        )
    }

    fn draw(&self, canvas: &mut Canvas, frame: Rect) {
        // Draw child
        let child_frame = Rect::new(
            frame.x + self.width as i32,
            frame.y + self.width as i32,
            self.cached_size.width,
            self.cached_size.height,
        );
        self.child.draw(canvas, child_frame);

        // Draw border
        for i in 0..self.width {
            let inset = i as i32;
            let w = frame.width.saturating_sub(i * 2);
            let h = frame.height.saturating_sub(i * 2);
            canvas.draw_rect(
                frame.x + inset,
                frame.y + inset,
                w,
                h,
                self.color,
            );
        }
    }

    fn on_event_capture(&mut self, event: &mut crate::event::Event, frame: Rect) -> bool {
        let child_frame = Rect::new(
            frame.x + self.width as i32,
            frame.y + self.width as i32,
            self.cached_size.width,
            self.cached_size.height,
        );
        self.child.on_event_capture(event, child_frame)
    }

    fn on_event(&mut self, event: &mut crate::event::Event, frame: Rect) -> bool {
        let child_frame = Rect::new(
            frame.x + self.width as i32,
            frame.y + self.width as i32,
            self.cached_size.width,
            self.cached_size.height,
        );
        if child_frame.contains(event.x(), event.y()) {
            self.child.on_event(event, child_frame)
        } else {
            false
        }
    }

    fn needs_draw(&self) -> bool {
        self.child.needs_draw()
    }

    fn set_needs_draw(&mut self) {
        self.child.set_needs_draw()
    }
}

/// Wrapper that adds a background to a view
pub struct Background {
    child: Box<dyn View>,
    color: Color,
    cached_size: Size,
}

impl Background {
    pub fn new<V: View + 'static>(child: V, color: Color) -> Self {
        Self {
            child: Box::new(child),
            color,
            cached_size: Size::ZERO,
        }
    }
}

impl View for Background {
    fn layout(&mut self, available: Size) -> Size {
        self.cached_size = self.child.layout(available);
        self.cached_size
    }

    fn draw(&self, canvas: &mut Canvas, frame: Rect) {
        // Draw background
        canvas.fill_rect(frame.x, frame.y, frame.width, frame.height, self.color);
        // Draw child on top
        self.child.draw(canvas, frame);
    }

    fn on_event_capture(&mut self, event: &mut crate::event::Event, frame: Rect) -> bool {
        self.child.on_event_capture(event, frame)
    }

    fn on_event(&mut self, event: &mut crate::event::Event, frame: Rect) -> bool {
        self.child.on_event(event, frame)
    }

    fn needs_draw(&self) -> bool {
        self.child.needs_draw()
    }

    fn set_needs_draw(&mut self) {
        self.child.set_needs_draw()
    }
}

/// Extension trait for applying modifiers to any View
pub trait ViewModifier: View + Sized {
    /// Add rounded corners to this view
    fn corner_radius(self, radius: u32) -> CornerRadius
    where
        Self: 'static,
    {
        CornerRadius::new(self, radius)
    }

    /// Add rounded corners with a background color
    fn corner_radius_with_background(self, radius: u32, color: Color) -> CornerRadius
    where
        Self: 'static,
    {
        CornerRadius::new(self, radius).background(color)
    }

    /// Add a border to this view
    fn border(self, width: u32, color: Color) -> Border
    where
        Self: 'static,
    {
        Border::new(self, width, color)
    }

    /// Add a rounded border to this view
    fn rounded_border(self, width: u32, radius: u32, color: Color) -> RoundedBorder
    where
        Self: 'static,
    {
        RoundedBorder::new(self, width, radius, color)
    }

    /// Add a background color to this view
    fn background_color(self, color: Color) -> Background
    where
        Self: 'static,
    {
        Background::new(self, color)
    }

    /// Add padding to this view
    fn padding(self, amount: u32) -> crate::view::Padding
    where
        Self: 'static,
    {
        crate::view::Padding::new(self).all(amount)
    }

    /// Add horizontal and vertical padding separately
    fn padding_hv(self, horizontal: u32, vertical: u32) -> crate::view::Padding
    where
        Self: 'static,
    {
        crate::view::Padding::new(self).horizontal(horizontal).vertical(vertical)
    }
}

// Implement ViewModifier for all View types
impl<T: View> ViewModifier for T {}
