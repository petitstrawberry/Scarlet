//! Window view - the root view with decorations

use super::traits::{View, ViewBox, Size};
use crate::graphics::{Canvas, Rect};
use crate::event::{Event, EventKind, MouseButton};
use crate::Color;
use scarlet_std::boxed::Box;
use scarlet_std::vec::Vec;
use sws_client::WindowSizeLimits;

/// Title bar height in pixels
const TITLEBAR_HEIGHT: u32 = 32;
/// Close button size
const CLOSE_BUTTON_SIZE: u32 = 18;
/// Close button margin from edge
const CLOSE_BUTTON_MARGIN: u32 = 8;
/// Window corner radius
const WINDOW_CORNER_RADIUS: u32 = 8;

/// Window - a root view with decorations (title bar, border)
///
/// Window is the top-level View in a view hierarchy. It manages:
/// - Title bar with close button
/// - Border decoration
/// - Content area for child views
///
/// # Example
///
/// ```no_run
/// use scarlet_ui::{Window, VStack, Label, Button};
///
/// let window = Window::new("My App", 400, 300)
///     .background(Color::DARK_GRAY)
///     .content(
///         VStack::new()
///             .child(Label::new("Hello"))
///             .child(Button::new("Click", || {}))
///     );
/// ```
pub struct Window {
    title: [u8; 64],
    title_len: usize,
    width: u32,
    height: u32,
    background: Color,
    content: Option<ViewBox>,
    content_size: Size,

    size_limits: WindowSizeLimits,
    
    // State
    close_button_hovered: bool,
    close_button_pressed: bool,
    close_requested: bool,
    move_requested: bool,
    needs_redraw: bool,
}

impl Window {
    /// Create a new window
    pub fn new(title: &str, width: u32, height: u32) -> Self {
        let mut title_buf = [0u8; 64];
        let bytes = title.as_bytes();
        let len = bytes.len().min(64);
        title_buf[..len].copy_from_slice(&bytes[..len]);
        
        Self {
            title: title_buf,
            title_len: len,
            width,
            height,
            background: Color::rgb(40, 40, 40),
            content: None,
            content_size: Size::ZERO,
            size_limits: WindowSizeLimits::NONE,
            close_button_hovered: false,
            close_button_pressed: false,
            close_requested: false,
            move_requested: false,
            needs_redraw: true,
        }
    }

    /// Set minimum window size in pixels.
    pub fn min_size(mut self, width: u32, height: u32) -> Self {
        self.size_limits.min_width = width;
        self.size_limits.min_height = height;
        self
    }

    /// Set maximum window size in pixels.
    pub fn max_size(mut self, width: u32, height: u32) -> Self {
        self.size_limits.max_width = width;
        self.size_limits.max_height = height;
        self
    }

    /// Set size limits in pixels.
    pub fn size_limits(mut self, limits: WindowSizeLimits) -> Self {
        self.size_limits = limits;
        self
    }

    /// Get configured size limits.
    pub fn get_size_limits(&self) -> WindowSizeLimits {
        self.size_limits
    }

    /// Set background color (builder pattern)
    pub fn background(mut self, color: Color) -> Self {
        self.background = color;
        self
    }

    /// Set content view (builder pattern)
    pub fn content<V: View + 'static>(mut self, view: V) -> Self {
        self.content = Some(Box::new(view));
        self
    }

    /// Set content view
    pub fn set_content<V: View + 'static>(&mut self, view: V) {
        self.content = Some(Box::new(view));
        self.needs_redraw = true;
    }

    /// Get window width
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Get window height
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Update window size.
    ///
    /// This does not perform any protocol-level resize; it only updates the UI's
    /// layout and drawing dimensions.
    pub fn set_size(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
        self.needs_redraw = true;
    }

    /// Get content area (excluding title bar)
    pub fn content_frame(&self) -> Rect {
        Rect::new(
            0,
            TITLEBAR_HEIGHT as i32,
            self.width,
            self.height.saturating_sub(TITLEBAR_HEIGHT),
        )
    }

    /// Check if close was requested
    pub fn is_close_requested(&self) -> bool {
        self.close_requested
    }

    pub fn take_move_requested(&mut self) -> bool {
        let v = self.move_requested;
        self.move_requested = false;
        v
    }

    /// Clear redraw flag after a successful draw/commit.
    pub fn clear_needs_draw(&mut self) {
        self.needs_redraw = false;
    }

    /// Get close button rect
    fn close_button_rect(&self) -> Rect {
        let seg_w = (CLOSE_BUTTON_SIZE + CLOSE_BUTTON_MARGIN * 2).min(self.width);
        let x = (self.width - seg_w) as i32;
        Rect::new(x, 0, seg_w, TITLEBAR_HEIGHT)
    }

    /// Get title bar rect
    fn titlebar_rect(&self) -> Rect {
        Rect::new(0, 0, self.width, TITLEBAR_HEIGHT)
    }

    /// Draw the title bar
    fn draw_titlebar(&self, canvas: &mut Canvas) {
        // Title bar with gradient effect and rounded top corners
        // Light (white-based) titlebar (no gradient)
        let base_color = Color::rgb(235, 235, 238);
        
        let r = WINDOW_CORNER_RADIUS;

        let close_rect = self.close_button_rect();
        let close_x0 = close_rect.x.max(0) as u32;
        let close_color = if self.close_button_pressed {
            Color::rgb(190, 190, 194)
        } else if self.close_button_hovered {
            Color::rgb(210, 210, 214)
        } else {
            base_color
        };
        
        for y in 0..TITLEBAR_HEIGHT {
            let color = base_color;
            let color_close = close_color;
            
            // For top rows, apply corner rounding
            if y < r {
                let dy = (r - y) as i32;
                for x in 0..self.width {
                    // Check if inside rounded corners
                    let in_left = x < r;
                    let in_right = x >= self.width - r;
                    
                    let mut skip = false;
                    if in_left {
                        let dx = (r - x) as i32;
                        if dx * dx + dy * dy > (r * r) as i32 {
                            skip = true;
                        }
                    }
                    if in_right {
                        let dx = (x - (self.width - r)) as i32;
                        if dx * dx + dy * dy > (r * r) as i32 {
                            skip = true;
                        }
                    }
                    
                    if !skip {
                        let c = if x >= close_x0 { color_close } else { color };
                        canvas.put_pixel(x as i32, y as i32, c);
                    }
                }
            } else {
                canvas.fill_rect(0, y as i32, close_x0, 1, color);
                canvas.fill_rect(close_rect.x, y as i32, self.width.saturating_sub(close_x0), 1, color_close);
            }
        }

        // Title text
        let title_str = core::str::from_utf8(&self.title[..self.title_len]).unwrap_or("");
        canvas.draw_text(10, 9, title_str, Color::rgb(20, 20, 24));

        // X mark on close segment
        let cx = close_rect.x + close_rect.width as i32 / 2;
        let cy = close_rect.y + close_rect.height as i32 / 2;
        let size: i32 = 10;
        let half = size / 2;
        let x0 = cx - half;
        let x1 = cx + half - 1;
        let y0 = cy - half;
        let y1 = cy + half - 1;

        // Double-stroke for better visibility (2px)
        let x_color = Color::rgb(30, 30, 34);
        canvas.draw_line(x0, y0, x1, y1, x_color);
        canvas.draw_line(x1, y0, x0, y1, x_color);
    }

    /// Draw the border
    fn draw_border(&self, canvas: &mut Canvas) {
        // Modern border with subtle shadow effect and rounded corners
        let border_color = Color::rgb(100, 100, 105);
        canvas.draw_rounded_rect(0, 0, self.width, self.height, WINDOW_CORNER_RADIUS, border_color);
        
        // Inner highlight for depth
        canvas.draw_rounded_rect(1, 1, self.width - 2, self.height - 2, WINDOW_CORNER_RADIUS.saturating_sub(1), Color::rgb(90, 90, 95));
    }
}

impl View for Window {
    fn layout(&mut self, _available: Size) -> Size {
        // Window takes the size it was created with
        let size = Size::new(self.width, self.height);
        
        // Layout content in content area
        if let Some(ref mut content) = self.content {
            let content_available = Size::new(
                self.width,
                self.height.saturating_sub(TITLEBAR_HEIGHT),
            );
            self.content_size = content.layout(content_available);
        }
        
        size
    }

    fn draw(&self, canvas: &mut Canvas, frame: Rect) {
        // Fill background with rounded corners
        canvas.fill_rounded_rect(frame.x, frame.y, frame.width, frame.height, WINDOW_CORNER_RADIUS, self.background);
        
        // Draw content
        if let Some(ref content) = self.content {
            let content_frame = Rect::new(
                frame.x,
                frame.y + TITLEBAR_HEIGHT as i32,
                self.content_size.width,
                self.content_size.height,
            );
            content.draw(canvas, content_frame);
        }
        
        // Draw decorations on top
        self.draw_titlebar(canvas);
        self.draw_border(canvas);
    }

    fn on_event_capture(&mut self, event: &mut Event, _frame: Rect) -> bool {
        // Window can intercept events in title bar
        let titlebar = self.titlebar_rect();
        
        if titlebar.contains(event.x(), event.y()) {
            match event.kind {
                EventKind::MouseMove => {
                    let close_rect = self.close_button_rect();
                    let was_hovered = self.close_button_hovered;
                    self.close_button_hovered = close_rect.contains(event.x(), event.y());
                    if was_hovered != self.close_button_hovered {
                        self.needs_redraw = true;
                    }
                    // Don't consume - let event continue for other title bar interactions
                    false
                }
                EventKind::MouseDown { button: MouseButton::Left } => {
                    let close_rect = self.close_button_rect();
                    if close_rect.contains(event.x(), event.y()) {
                        self.close_button_pressed = true;
                        self.needs_redraw = true;
                        true // Consume
                    } else {
                        // Titlebar drag: request compositor-level move.
                        self.move_requested = true;
                        true
                    }
                }
                EventKind::MouseUp { button: MouseButton::Left } => {
                    if self.close_button_pressed {
                        self.close_button_pressed = false;
                        let close_rect = self.close_button_rect();
                        if close_rect.contains(event.x(), event.y()) {
                            self.close_requested = true;
                        }
                        self.needs_redraw = true;
                        true // Consume
                    } else {
                        false
                    }
                }
                _ => false,
            }
        } else {
            // Not in title bar - reset close button hover
            if self.close_button_hovered {
                self.close_button_hovered = false;
                self.needs_redraw = true;
            }
            false
        }
    }

    fn on_event(&mut self, _event: &mut Event, _frame: Rect) -> bool {
        // Window doesn't handle events in bubble phase
        false
    }

    fn children(&self) -> Vec<(&dyn View, Rect)> {
        if let Some(ref content) = self.content {
            let content_frame = Rect::new(
                0,
                TITLEBAR_HEIGHT as i32,
                self.content_size.width,
                self.content_size.height,
            );
            let mut v = Vec::new();
            v.push((content.as_ref() as &dyn View, content_frame));
            v
        } else {
            Vec::new()
        }
    }

    fn children_mut(&mut self) -> Vec<(&mut dyn View, Rect)> {
        if let Some(ref mut content) = self.content {
            let content_frame = Rect::new(
                0,
                TITLEBAR_HEIGHT as i32,
                self.content_size.width,
                self.content_size.height,
            );
            let mut v = Vec::new();
            v.push((content.as_mut() as &mut dyn View, content_frame));
            v
        } else {
            Vec::new()
        }
    }

    fn visit_children(&self, visitor: &mut dyn FnMut(&dyn View, Rect) -> bool) {
        if let Some(ref content) = self.content {
            let content_frame = Rect::new(
                0,
                TITLEBAR_HEIGHT as i32,
                self.content_size.width,
                self.content_size.height,
            );
            let _ = visitor(content.as_ref() as &dyn View, content_frame);
        }
    }

    fn visit_children_mut(&mut self, visitor: &mut dyn FnMut(&mut dyn View, Rect) -> bool) {
        if let Some(ref mut content) = self.content {
            let content_frame = Rect::new(
                0,
                TITLEBAR_HEIGHT as i32,
                self.content_size.width,
                self.content_size.height,
            );
            let _ = visitor(content.as_mut() as &mut dyn View, content_frame);
        }
    }

    fn needs_draw(&self) -> bool {
        self.needs_redraw
    }

    fn set_needs_draw(&mut self) {
        self.needs_redraw = true;
    }

    fn clear_needs_draw(&mut self) {
        self.needs_redraw = false;
    }
}
