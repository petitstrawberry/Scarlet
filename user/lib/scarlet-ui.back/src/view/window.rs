//! Window view - the root view with decorations

use super::traits::{View, ViewBox, Size};
use crate::graphics::{Canvas, Rect};
use crate::event::{Event, EventKind, MouseButton};
use crate::Color;
use scarlet_std::boxed::Box;
use scarlet_std::vec::Vec;
use sws_client::WindowSizeLimits;
use scarlet_std::string::String;
use scarlet_std::string::ToString;

/// Title bar height in pixels
const TITLEBAR_HEIGHT: u32 = 32;
/// Close button size
const CLOSE_BUTTON_SIZE: u32 = 18;
/// Close button margin from edge
const CLOSE_BUTTON_MARGIN: u32 = 8;
/// Titlebar control buttons: hide (minimize), maximize, close
const TITLEBAR_CONTROL_COUNT: u32 = 3;
/// Window corner radius
const WINDOW_CORNER_RADIUS: u32 = 0;

/// Window type for compositor Z-order management.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowKind {
    /// Standard application window.
    Normal,
    /// Always stays above normal/taskbar windows.
    AlwaysOnTop,
    /// Taskbar or panel surface.
    Taskbar,
    /// Desktop/background surface (lowest layer).
    Desktop,
}

impl WindowKind {
    /// Numeric value expected by the SWS protocol.
    pub const fn to_protocol_value(self) -> u32 {
        match self {
            WindowKind::Normal => 0,
            WindowKind::AlwaysOnTop => 1,
            WindowKind::Taskbar => 2,
            WindowKind::Desktop => 3,
        }
    }
}

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
    app_id: Option<String>,
    width: u32,
    height: u32,
    background: Color,
    content: Option<ViewBox>,
    content_size: Size,

    size_limits: WindowSizeLimits,
    window_type: WindowKind,

    // Window properties
    is_main_window: bool,

    // Focus management
    focused_frame: Option<(Rect, bool)>, // (frame, was_focusable)

    // State
    close_button_hovered: bool,
    close_button_pressed: bool,
    close_requested: bool,

    minimize_button_hovered: bool,
    minimize_button_pressed: bool,
    minimize_requested: bool,

    maximize_button_hovered: bool,
    maximize_button_pressed: bool,
    maximize_toggle_requested: bool,

    move_requested: bool,
    needs_redraw: bool,
}

impl Window {
    /// Title bar height in pixels.
    pub fn titlebar_height() -> u32 {
        TITLEBAR_HEIGHT
    }

    /// Create a new window
    pub fn new(title: &str, width: u32, height: u32) -> Self {
        let mut title_buf = [0u8; 64];
        let bytes = title.as_bytes();
        let len = bytes.len().min(64);
        title_buf[..len].copy_from_slice(&bytes[..len]);
        
        Self {
            title: title_buf,
            title_len: len,
            app_id: None,
            width,
            height,
            background: Color::rgb(40, 40, 40),
            content: None,
            content_size: Size::ZERO,
            size_limits: WindowSizeLimits::NONE,
            window_type: WindowKind::Normal,
            is_main_window: false,
            focused_frame: None,
            close_button_hovered: false,
            close_button_pressed: false,
            close_requested: false,

            minimize_button_hovered: false,
            minimize_button_pressed: false,
            minimize_requested: false,

            maximize_button_hovered: false,
            maximize_button_pressed: false,
            maximize_toggle_requested: false,

            move_requested: false,
            needs_redraw: true,
        }
    }

    /// Mark this as the main window (closing it terminates the app)
    pub fn main_window(mut self) -> Self {
        self.is_main_window = true;
        self
    }

    /// Check if this is the main window
    pub fn is_main_window(&self) -> bool {
        self.is_main_window
    }

    /// Set the application identifier for this window
    pub fn app_id(mut self, app_id: &str) -> Self {
        self.app_id = Some(app_id.to_string());
        self
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

    /// Get the application identifier.
    pub fn get_app_id(&self) -> Option<&str> {
        self.app_id.as_deref()
    }

    /// Set the window type used by the compositor for stacking.
    pub fn window_type(mut self, kind: WindowKind) -> Self {
        self.window_type = kind;
        self
    }

    /// Get the requested window type.
    pub fn get_window_type(&self) -> WindowKind {
        self.window_type
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

    /// Draw window decorations (title bar + border).
    ///
    /// This is useful when a different system renders the window content but
    /// wants to reuse ScarletUI's native decorations.
    pub fn draw_decorations(&self, canvas: &mut Canvas) {
        self.draw_titlebar(canvas);
        self.draw_border(canvas);
    }

    /// Draw only the title bar.
    pub fn draw_titlebar_only(&self, canvas: &mut Canvas) {
        self.draw_titlebar(canvas);
    }

    /// Draw only the border.
    pub fn draw_border_only(&self, canvas: &mut Canvas) {
        self.draw_border(canvas);
    }

    /// Check if close was requested
    pub fn is_close_requested(&self) -> bool {
        self.close_requested
    }

    pub fn take_minimize_requested(&mut self) -> bool {
        let v = self.minimize_requested;
        self.minimize_requested = false;
        v
    }

    pub fn take_maximize_toggle_requested(&mut self) -> bool {
        let v = self.maximize_toggle_requested;
        self.maximize_toggle_requested = false;
        v
    }

    pub fn can_maximize(&self) -> bool {
        self.size_limits.max_width == 0 && self.size_limits.max_height == 0
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
        self.control_button_rect(0)
    }

    fn maximize_button_rect(&self) -> Rect {
        self.control_button_rect(1)
    }

    fn minimize_button_rect(&self) -> Rect {
        self.control_button_rect(2)
    }

    /// Get control button rects.
    ///
    /// `index_from_right`: 0=close, 1=maximize, 2=minimize.
    fn control_button_rect(&self, index_from_right: u32) -> Rect {
        // Don't draw control buttons if window is too narrow (avoids negative positioning)
        if self.width < TITLEBAR_CONTROL_COUNT {
            return Rect::new(0, 0, 0, 0);
        }
        
        let base_seg_w = CLOSE_BUTTON_SIZE + CLOSE_BUTTON_MARGIN * 2;
        let seg_w = if self.width >= base_seg_w * TITLEBAR_CONTROL_COUNT {
            base_seg_w
        } else {
            // Ensure 3 segments always fit (even if tiny).
            (self.width / TITLEBAR_CONTROL_COUNT).max(1)
        };
        let total_w = seg_w.saturating_mul(TITLEBAR_CONTROL_COUNT).min(self.width);
        let right_x0 = (self.width - total_w) as i32;
        let x = right_x0 + (total_w as i32) - (seg_w as i32) * (index_from_right as i32 + 1);
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
        let maximize_rect = self.maximize_button_rect();
        let minimize_rect = self.minimize_button_rect();

        let close_color = if self.close_button_pressed {
            Color::rgb(190, 190, 194)
        } else if self.close_button_hovered {
            Color::rgb(210, 210, 214)
        } else {
            base_color
        };

        let maximize_color = if !self.can_maximize() {
            Color::rgb(225, 225, 228)
        } else if self.maximize_button_pressed {
            Color::rgb(190, 190, 194)
        } else if self.maximize_button_hovered {
            Color::rgb(210, 210, 214)
        } else {
            base_color
        };

        let minimize_color = if self.minimize_button_pressed {
            Color::rgb(190, 190, 194)
        } else if self.minimize_button_hovered {
            Color::rgb(210, 210, 214)
        } else {
            base_color
        };
        
        for y in 0..TITLEBAR_HEIGHT {
            let color = base_color;

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
                        let xi = x as i32;
                        let c = if close_rect.contains(xi, y as i32) {
                            close_color
                        } else if maximize_rect.contains(xi, y as i32) {
                            maximize_color
                        } else if minimize_rect.contains(xi, y as i32) {
                            minimize_color
                        } else {
                            color
                        };
                        canvas.put_pixel(x as i32, y as i32, c);
                    }
                }
            } else {
                canvas.fill_rect(0, y as i32, self.width, 1, color);
                canvas.fill_rect(close_rect.x, y as i32, close_rect.width, 1, close_color);
                canvas.fill_rect(maximize_rect.x, y as i32, maximize_rect.width, 1, maximize_color);
                canvas.fill_rect(minimize_rect.x, y as i32, minimize_rect.width, 1, minimize_color);
            }
        }

        // Title text
        let title_str = core::str::from_utf8(&self.title[..self.title_len]).unwrap_or("");
        canvas.draw_text(10, 9, title_str, Color::rgb(20, 20, 24));

        // Icons on control segments
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

        // Maximize: draw a square outline
        let mx = maximize_rect.x + maximize_rect.width as i32 / 2;
        let my = maximize_rect.y + maximize_rect.height as i32 / 2;
        let msize: i32 = 10;
        let mhalf = msize / 2;
        let mx0 = mx - mhalf;
        let my0 = my - mhalf;
        canvas.draw_rect(mx0, my0, msize as u32, msize as u32, x_color);

        // Minimize (hide): draw a horizontal line
        let nx = minimize_rect.x + minimize_rect.width as i32 / 2;
        let ny = minimize_rect.y + minimize_rect.height as i32 / 2 + 3;
        let nsize: i32 = 12;
        let nhalf = nsize / 2;
        canvas.draw_line(nx - nhalf, ny, nx + nhalf, ny, x_color);
    }

    /// Draw the border
    fn draw_border(&self, canvas: &mut Canvas) {
        // Modern border with subtle shadow effect (no rounded corners)
        let border_color = Color::rgb(100, 100, 105);
        if self.width == 0 || self.height == 0 {
            return;
        }

        canvas.draw_rect(0, 0, self.width, self.height, border_color);

        // Inner highlight for depth
        if self.width > 2 && self.height > 2 {
            canvas.draw_rect(
                1,
                1,
                self.width.saturating_sub(2),
                self.height.saturating_sub(2),
                Color::rgb(90, 90, 95),
            );
        }
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
        // Fill background (no rounded corners)
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

                    let maximize_rect = self.maximize_button_rect();
                    let was_hovered = self.maximize_button_hovered;
                    self.maximize_button_hovered = maximize_rect.contains(event.x(), event.y());
                    if was_hovered != self.maximize_button_hovered {
                        self.needs_redraw = true;
                    }

                    let minimize_rect = self.minimize_button_rect();
                    let was_hovered = self.minimize_button_hovered;
                    self.minimize_button_hovered = minimize_rect.contains(event.x(), event.y());
                    if was_hovered != self.minimize_button_hovered {
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
                        let maximize_rect = self.maximize_button_rect();
                        if maximize_rect.contains(event.x(), event.y()) {
                            if self.can_maximize() {
                                self.maximize_button_pressed = true;
                                self.needs_redraw = true;
                                true
                            } else {
                                // Disabled state: don't consume, allow drag.
                                false
                            }
                        } else {
                            let minimize_rect = self.minimize_button_rect();
                            if minimize_rect.contains(event.x(), event.y()) {
                                self.minimize_button_pressed = true;
                                self.needs_redraw = true;
                                true
                            } else {
                                // Titlebar drag: request compositor-level move.
                                self.move_requested = true;
                                true
                            }
                        }
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
                    } else if self.maximize_button_pressed {
                        self.maximize_button_pressed = false;
                        let maximize_rect = self.maximize_button_rect();
                        if maximize_rect.contains(event.x(), event.y()) {
                            self.maximize_toggle_requested = true;
                        }
                        self.needs_redraw = true;
                        true
                    } else if self.minimize_button_pressed {
                        self.minimize_button_pressed = false;
                        let minimize_rect = self.minimize_button_rect();
                        if minimize_rect.contains(event.x(), event.y()) {
                            self.minimize_requested = true;
                        }
                        self.needs_redraw = true;
                        true
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
            if self.maximize_button_hovered {
                self.maximize_button_hovered = false;
                self.needs_redraw = true;
            }
            if self.minimize_button_hovered {
                self.minimize_button_hovered = false;
                self.needs_redraw = true;
            }

            // Handle focus management for MouseDown events
            if matches!(event.kind, EventKind::MouseDown { button: MouseButton::Left }) {
                // If we have a focused frame and click is outside it, send blur
                if let Some((focused_rect, _)) = self.focused_frame {
                    if !focused_rect.contains(event.x(), event.y()) {
                        // Send blur to all children (they'll handle it if they have focus)
                        let mut blur_event = Event::new(EventKind::Blur, event.position);
                        if let Some(ref mut content) = self.content {
                            let content_frame = Rect::new(
                                0,
                                TITLEBAR_HEIGHT as i32,
                                self.content_size.width,
                                self.content_size.height,
                            );
                            let _ = content.on_event(&mut blur_event, content_frame);
                        }
                        self.focused_frame = None;
                    }
                }
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
