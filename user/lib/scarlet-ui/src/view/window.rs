//! Window view - the root view with decorations

use super::traits::{View, ViewBox, Size};
use crate::graphics::{Canvas, Rect};
use crate::event::{Event, EventKind, MouseButton};
use crate::Color;
use scarlet_std::boxed::Box;
use scarlet_std::vec::Vec;

/// Title bar height in pixels
const TITLEBAR_HEIGHT: u32 = 28;
/// Close button size
const CLOSE_BUTTON_SIZE: u32 = 16;
/// Close button margin from edge
const CLOSE_BUTTON_MARGIN: u32 = 6;

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
    
    // State
    close_button_hovered: bool,
    close_button_pressed: bool,
    close_requested: bool,
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
            close_button_hovered: false,
            close_button_pressed: false,
            close_requested: false,
            needs_redraw: true,
        }
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

    /// Clear redraw flag after a successful draw/commit.
    pub fn clear_needs_draw(&mut self) {
        self.needs_redraw = false;
    }

    /// Get close button rect
    fn close_button_rect(&self) -> Rect {
        let x = (self.width - CLOSE_BUTTON_SIZE - CLOSE_BUTTON_MARGIN) as i32;
        let y = CLOSE_BUTTON_MARGIN as i32;
        Rect::new(x, y, CLOSE_BUTTON_SIZE, CLOSE_BUTTON_SIZE)
    }

    /// Get title bar rect
    fn titlebar_rect(&self) -> Rect {
        Rect::new(0, 0, self.width, TITLEBAR_HEIGHT)
    }

    /// Draw the title bar
    fn draw_titlebar(&self, canvas: &mut Canvas) {
        // Title bar background
        canvas.fill_rect(0, 0, self.width, TITLEBAR_HEIGHT, Color::rgb(60, 60, 60));

        // Title text
        let title_str = core::str::from_utf8(&self.title[..self.title_len]).unwrap_or("");
        canvas.draw_text(10, 8, title_str, Color::WHITE);

        // Close button
        let close_rect = self.close_button_rect();
        let close_color = if self.close_button_pressed {
            Color::rgb(200, 60, 60)
        } else if self.close_button_hovered {
            Color::rgb(230, 80, 80)
        } else {
            Color::rgb(180, 180, 180)
        };
        canvas.fill_rect(close_rect.x, close_rect.y, close_rect.width, close_rect.height, close_color);

        // X mark on close button
        for i in 0..CLOSE_BUTTON_SIZE {
            canvas.put_pixel(close_rect.x + i as i32, close_rect.y + i as i32, Color::BLACK);
            canvas.put_pixel(
                close_rect.x + i as i32,
                close_rect.y + (CLOSE_BUTTON_SIZE - 1 - i) as i32,
                Color::BLACK,
            );
        }
    }

    /// Draw the border
    fn draw_border(&self, canvas: &mut Canvas) {
        canvas.draw_rect(0, 0, self.width, self.height, Color::rgb(80, 80, 80));
    }
}

impl View for Window {
    fn layout(&mut self, available: Size) -> Size {
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
        // Fill background
        canvas.fill_rect(frame.x, frame.y, frame.width, frame.height, self.background);
        
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

    fn on_event_capture(&mut self, event: &mut Event, frame: Rect) -> bool {
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
                        false
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

    fn needs_draw(&self) -> bool {
        self.needs_redraw
    }

    fn set_needs_draw(&mut self) {
        self.needs_redraw = true;
    }
}
