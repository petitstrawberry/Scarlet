//! View modifiers for styling and composition
//!
//! Modifiers enable SwiftUI-style method chaining for view customization:
//! ```no_run
//! Label::new("Hello")
//!     .corner_radius(8)
//!     .padding(10)
//!     .background(Color::BLUE)
//!     .border(2, Color::WHITE)
//! ```

use crate::graphics::{Canvas, Rect};
use crate::view::{Size, View};
use crate::Color;
use scarlet_std::boxed::Box;

/// Wrapper that adds rounded corners to a view
pub struct CornerRadius {
    child: Box<dyn View>,
    radius: u32,
    cached_size: Size,
}

impl CornerRadius {
    pub fn new<V: View + 'static>(child: V, radius: u32) -> Self {
        Self {
            child: Box::new(child),
            radius,
            cached_size: Size::ZERO,
        }
    }
}

impl View for CornerRadius {
    fn layout(&mut self, available: Size) -> Size {
        self.cached_size = self.child.layout(available);
        self.cached_size
    }

    fn draw(&self, canvas: &mut Canvas, frame: Rect) {
        // For now, draw the child normally
        // TODO: Implement actual corner clipping/masking
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

    /// Add a border to this view
    fn border(self, width: u32, color: Color) -> Border
    where
        Self: 'static,
    {
        Border::new(self, width, color)
    }

    /// Add a background color to this view
    fn background_color(self, color: Color) -> Background
    where
        Self: 'static,
    {
        Background::new(self, color)
    }
}

// Implement ViewModifier for all View types
impl<T: View> ViewModifier for T {}
