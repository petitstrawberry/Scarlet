//! Window View - Top-level window container with decorations
//!
//! Window is a View that provides window-level decorations including:
//! - Title bar with close, maximize, minimize buttons
//! - Window border with shadow
//! - Proper event handling for window controls

use alloc::string::String;
use alloc::vec::Vec;
use alloc::boxed::Box;
use alloc::format;
use core::any::Any;

use crate::view::View;
use crate::element::{Element, RenderElement, ElementRenderObject, LayoutConstraints, UpdateResult};
use crate::geometry::{Size, Point, Rect};
use crate::color::{Color, ColorPalette};
use crate::buffer::Buffer;
use crate::event::{MouseEvent, MouseButton};
use crate::state::Listenable;

/// Constants for window decorations
const TITLEBAR_HEIGHT: f32 = 32.0;
const CLOSE_BUTTON_SIZE: f32 = 18.0;
const BUTTON_MARGIN: f32 = 8.0;
const BUTTON_SPACING: f32 = 4.0;
const WINDOW_CORNER_RADIUS: f32 = 0.0;

/// Window View - top-level window container
///
/// Window provides window-level properties like title, size, and decorations.
/// It wraps a child View and provides window decorations (titlebar, buttons, border).
pub struct Window<V: View> {
    app_id: String,
    title: String,
    size: Size,
    child: V,
    resizable: bool,
    decorated: bool,
}

impl<V: View> Window<V> {
    /// Create a new Window with a title and child
    pub fn new(title: impl Into<String>, child: V) -> Self {
        let title_str = title.into();
        Self {
            app_id: String::from("com.example.scarletui"),
            title: title_str,
            size: Size::new(800.0, 600.0),
            child,
            resizable: true,
            decorated: true,
        }
    }

    /// Set the application ID
    pub fn app_id(mut self, app_id: impl Into<String>) -> Self {
        self.app_id = app_id.into();
        self
    }

    /// Set the window size
    pub fn size(mut self, size: Size) -> Self {
        self.size = size;
        self
    }

    /// Set whether the window is resizable
    pub fn resizable(mut self, resizable: bool) -> Self {
        self.resizable = resizable;
        self
    }

    /// Set whether the window has decorations (title bar, borders)
    pub fn decorated(mut self, decorated: bool) -> Self {
        self.decorated = decorated;
        self
    }

    /// Get the application ID
    pub fn get_app_id(&self) -> &str {
        &self.app_id
    }

    /// Get the window title
    pub fn get_title(&self) -> &str {
        &self.title
    }

    /// Get the window size
    pub fn get_window_size(&self) -> Size {
        self.size
    }

    /// Check if the window is resizable
    pub fn is_resizable(&self) -> bool {
        self.resizable
    }

    /// Check if the window is decorated
    pub fn is_decorated(&self) -> bool {
        self.decorated
    }

    /// Get the child View
    pub fn child(&self) -> &V {
        &self.child
    }

    /// Get mutable reference to the child View
    pub fn child_mut(&mut self) -> &mut V {
        &mut self.child
    }
}

impl<V: View + Clone> Clone for Window<V> {
    fn clone(&self) -> Self {
        Self {
            app_id: self.app_id.clone(),
            title: self.title.clone(),
            size: self.size,
            child: self.child.clone(),
            resizable: self.resizable,
            decorated: self.decorated,
        }
    }
}

impl<V: View + Clone> View for Window<V> {
    fn create_element(&self) -> Box<dyn Element> {
        Box::new(RenderElement::new(
            self.clone(),
            WindowRenderObject::new(self.title.clone(), self.size, self.decorated),
        ))
    }

    fn listenables(&self) -> Vec<&dyn Listenable> {
        self.child.listenables()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Window RenderObject - handles rendering of window decorations
///
/// This renders the titlebar, control buttons (close, maximize, minimize),
/// window border, and shadow. The child content is rendered separately.
pub struct WindowRenderObject {
    title: String,
    size: Size,
    decorated: bool,
    focused: bool,

    // Button states
    close_button_hovered: bool,
    close_button_pressed: bool,
    minimize_button_hovered: bool,
    minimize_button_pressed: bool,
    maximize_button_hovered: bool,
    maximize_button_pressed: bool,

    // Layout
    titlebar_rect: Rect,
    close_button_rect: Rect,
    minimize_button_rect: Rect,
    maximize_button_rect: Rect,
    content_rect: Rect,

    // Buffers
    titlebar_buffer: Option<Buffer>,

    dirty: bool,
}

impl WindowRenderObject {
    pub fn new(title: String, size: Size, decorated: bool) -> Self {
        Self {
            title,
            size,
            decorated,
            focused: true,

            // Button states
            close_button_hovered: false,
            close_button_pressed: false,
            minimize_button_hovered: false,
            minimize_button_pressed: false,
            maximize_button_hovered: false,
            maximize_button_pressed: false,

            // Layout
            titlebar_rect: Rect::new(Point::new(0.0, 0.0), Size::new(0.0, 0.0)),
            close_button_rect: Rect::new(Point::new(0.0, 0.0), Size::new(0.0, 0.0)),
            minimize_button_rect: Rect::new(Point::new(0.0, 0.0), Size::new(0.0, 0.0)),
            maximize_button_rect: Rect::new(Point::new(0.0, 0.0), Size::new(0.0, 0.0)),
            content_rect: Rect::new(Point::new(0.0, 0.0), Size::new(0.0, 0.0)),

            // Buffers
            titlebar_buffer: None,

            dirty: true,
        }
    }

    /// Get the content area (excluding titlebar and borders)
    pub fn content_area(&self) -> Rect {
        self.content_rect
    }

    /// Get the titlebar height
    pub fn titlebar_height(&self) -> f32 {
        if self.decorated {
            TITLEBAR_HEIGHT
        } else {
            0.0
        }
    }

    /// Draw the titlebar with control buttons
    fn draw_titlebar(&mut self, palette: &ColorPalette) {
        if !self.decorated {
            return;
        }

        // Extract all needed state before borrowing
        let button_y = (TITLEBAR_HEIGHT - CLOSE_BUTTON_SIZE) / 2.0;
        let button_start_x = BUTTON_MARGIN;
        let close_rect = Rect::new(
            Point::new(button_start_x, button_y),
            Size::new(CLOSE_BUTTON_SIZE, CLOSE_BUTTON_SIZE)
        );
        let maximize_x = button_start_x + CLOSE_BUTTON_SIZE + BUTTON_SPACING;
        let maximize_rect = Rect::new(
            Point::new(maximize_x, button_y),
            Size::new(CLOSE_BUTTON_SIZE, CLOSE_BUTTON_SIZE)
        );
        let minimize_x = maximize_x + CLOSE_BUTTON_SIZE + BUTTON_SPACING;
        let minimize_rect = Rect::new(
            Point::new(minimize_x, button_y),
            Size::new(CLOSE_BUTTON_SIZE, CLOSE_BUTTON_SIZE)
        );

        let close_hovered = self.close_button_hovered;
        let close_pressed = self.close_button_pressed;
        let minimize_hovered = self.minimize_button_hovered;
        let minimize_pressed = self.minimize_button_pressed;
        let maximize_hovered = self.maximize_button_hovered;
        let maximize_pressed = self.maximize_button_pressed;
        let focused = self.focused;
        let title = self.title.clone();

        let width = libm::ceilf(self.size.width) as usize;
        let height = libm::ceilf(TITLEBAR_HEIGHT) as usize;

        // Create or resize buffer
        let needed = (width * height * 4) as usize;
        if self.titlebar_buffer.as_ref().map_or(true, |b| b.data().len() < needed) {
            self.titlebar_buffer = Some(Buffer::from_dimensions(width as u32, height as u32));
        }

        if let Some(ref mut buffer) = self.titlebar_buffer {
            // Base color from Scarlet_old
            let base_color = if focused {
                Color::rgb(0.92, 0.92, 0.93) // rgb(235, 235, 238)
            } else {
                Color::rgb(0.85, 0.85, 0.86)
            };

            // Button colors (all same color, only change on hover/press)
            let close_color = if close_pressed {
                Color::rgb(0.74, 0.74, 0.76) // rgb(190, 190, 194)
            } else if close_hovered {
                Color::rgb(0.82, 0.82, 0.84) // rgb(210, 210, 214)
            } else {
                base_color
            };

            let maximize_color = if maximize_pressed {
                Color::rgb(0.74, 0.74, 0.76)
            } else if maximize_hovered {
                Color::rgb(0.82, 0.82, 0.84)
            } else {
                base_color
            };

            let minimize_color = if minimize_pressed {
                Color::rgb(0.74, 0.74, 0.76)
            } else if minimize_hovered {
                Color::rgb(0.82, 0.82, 0.84)
            } else {
                base_color
            };

            // Fill titlebar background
            let mut data = buffer.data_mut();
            for y in 0..height {
                for x in 0..width {
                    let idx = (y * width + x) * 4;
                    let bgra = base_color.to_bgra();
                    // Write u32 as 4 u8 bytes (little-endian BGRA)
                    data[idx] = (bgra & 0xFF) as u8;
                    data[idx + 1] = ((bgra >> 8) & 0xFF) as u8;
                    data[idx + 2] = ((bgra >> 16) & 0xFF) as u8;
                    data[idx + 3] = ((bgra >> 24) & 0xFF) as u8;
                }
            }

            use crate::graphics::Canvas;
            let mut canvas = Canvas::new(&mut data, width as u32, height as u32);

            // Draw buttons with their colors
            Self::draw_button_rect(&mut canvas, &close_rect, close_color);
            Self::draw_button_rect(&mut canvas, &maximize_rect, maximize_color);
            Self::draw_button_rect(&mut canvas, &minimize_rect, minimize_color);

            // Draw titlebar border (bottom)
            let border_color = Color::rgb(0.39, 0.39, 0.41); // rgb(100, 100, 105)
            for x in 0..width as i32 {
                canvas.put_pixel(x, (height - 1) as i32, border_color);
            }

            // Draw title text (left-aligned at 10, 9 like Scarlet_old)
            if !title.is_empty() {
                use crate::graphics::measure_text_sized;

                let title_display = if title.len() > 60 {
                    format!("{}...", &title[..57])
                } else {
                    title
                };

                canvas.draw_text_sized(10, 9, &title_display, Color::rgb(0.08, 0.08, 0.09), 13.0);
            }

            // Draw icons on all buttons (always visible, like Scarlet_old)
            let icon_color = Color::rgb(0.12, 0.12, 0.13); // rgb(30, 30, 34)

            // Close: X icon (double-stroke)
            let cx = (close_rect.origin.x + CLOSE_BUTTON_SIZE / 2.0) as i32;
            let cy = (close_rect.origin.y + CLOSE_BUTTON_SIZE / 2.0) as i32;
            let size = 10i32;
            let half = size / 2;
            Self::draw_line(&mut canvas, cx - half, cy - half, cx + half - 1, cy + half - 1, icon_color);
            Self::draw_line(&mut canvas, cx + half - 1, cy - half, cx - half, cy + half - 1, icon_color);

            // Maximize: square outline
            let mx = (maximize_rect.origin.x + CLOSE_BUTTON_SIZE / 2.0) as i32;
            let my = (maximize_rect.origin.y + CLOSE_BUTTON_SIZE / 2.0) as i32;
            let msize = 10i32;
            let mhalf = msize / 2;
            Self::draw_rect_outline(&mut canvas, mx - mhalf, my - mhalf, msize as u32, msize as u32, icon_color);

            // Minimize: horizontal line
            let nx = (minimize_rect.origin.x + CLOSE_BUTTON_SIZE / 2.0) as i32;
            let ny = (minimize_rect.origin.y + CLOSE_BUTTON_SIZE / 2.0) as i32 + 3;
            let nsize = 12i32;
            let nhalf = nsize / 2;
            Self::draw_line(&mut canvas, nx - nhalf, ny, nx + nhalf, ny, icon_color);

            // Update self rects after drawing
            self.close_button_rect = close_rect;
            self.maximize_button_rect = maximize_rect;
            self.minimize_button_rect = minimize_rect;
        }
    }

    /// Draw a filled rectangle for a button
    fn draw_button_rect(canvas: &mut crate::graphics::Canvas, rect: &Rect, color: Color) {
        let x = rect.origin.x as i32;
        let y = rect.origin.y as i32;
        let w = libm::ceilf(rect.size.width) as u32;
        let h = libm::ceilf(rect.size.height) as u32;

        for dy in 0..h {
            for dx in 0..w {
                canvas.put_pixel(x + dx as i32, y + dy as i32, color);
            }
        }
    }

    /// Draw a line (for icons)
    fn draw_line(canvas: &mut crate::graphics::Canvas, x0: i32, y0: i32, x1: i32, y1: i32, color: Color) {
        // Bresenham's line algorithm
        let dx = (x1 - x0).abs();
        let dy = (y1 - y0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut err = dx - dy;

        let mut x = x0;
        let mut y = y0;

        loop {
            canvas.put_pixel(x, y, color);
            if x == x1 && y == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 > -dy {
                err -= dy;
                x += sx;
            }
            if e2 < dx {
                err += dx;
                y += sy;
            }
        }
    }

    /// Draw a rectangle outline (for maximize icon)
    fn draw_rect_outline(canvas: &mut crate::graphics::Canvas, x: i32, y: i32, w: u32, h: u32, color: Color) {
        let w = w as i32;
        let h = h as i32;

        // Top and bottom
        for i in 0..w {
            canvas.put_pixel(x + i, y, color);
            canvas.put_pixel(x + i, y + h - 1, color);
        }
        // Left and right
        for i in 0..h {
            canvas.put_pixel(x, y + i, color);
            canvas.put_pixel(x + w - 1, y + i, color);
        }
    }

    /// Calculate layout for decorations
    fn calculate_layout(&mut self) {
        if self.decorated {
            // Titlebar
            self.titlebar_rect = Rect::new(
                Point::new(0.0, 0.0),
                Size::new(self.size.width, TITLEBAR_HEIGHT)
            );

            // Content area (below titlebar)
            let content_y = TITLEBAR_HEIGHT;
            let content_height = (self.size.height - TITLEBAR_HEIGHT).max(0.0);
            self.content_rect = Rect::new(
                Point::new(0.0, content_y),
                Size::new(self.size.width, content_height)
            );
        } else {
            // No decorations - full window is content
            self.content_rect = Rect::new(Point::new(0.0, 0.0), self.size);
            self.titlebar_rect = Rect::new(Point::new(0.0, 0.0), Size::new(0.0, 0.0));
        }
    }

    /// Update button hover states based on mouse position
    pub fn update_hover_states(&mut self, point: Point) {
        let in_close = self.close_button_rect.contains(point);
        let in_minimize = self.minimize_button_rect.contains(point);
        let in_maximize = self.maximize_button_rect.contains(point);

        self.close_button_hovered = in_close;
        self.minimize_button_hovered = in_minimize;
        self.maximize_button_hovered = in_maximize;
    }

    /// Handle mouse events
    pub fn handle_mouse_event(&mut self, event: &MouseEvent) -> bool {
        match event {
            MouseEvent::Moved { x, y } => {
                let point = Point::new(*x as f32, *y as f32);
                self.update_hover_states(point);
                self.dirty = true;
                false
            }
            MouseEvent::ButtonPressed { button, x, y, .. } => {
                if *button == MouseButton::Left {
                    let point = Point::new(*x as f32, *y as f32);

                    if self.close_button_rect.contains(point) {
                        self.close_button_pressed = true;
                        self.dirty = true;
                        return true;
                    } else if self.minimize_button_rect.contains(point) {
                        self.minimize_button_pressed = true;
                        self.dirty = true;
                        return true;
                    } else if self.maximize_button_rect.contains(point) {
                        self.maximize_button_pressed = true;
                        self.dirty = true;
                        return true;
                    }
                }
                false
            }
            MouseEvent::ButtonReleased { button, .. } => {
                if *button == MouseButton::Left {
                    let close_clicked = self.close_button_pressed;
                    let minimize_clicked = self.minimize_button_pressed;
                    let maximize_clicked = self.maximize_button_pressed;

                    self.close_button_pressed = false;
                    self.minimize_button_pressed = false;
                    self.maximize_button_pressed = false;
                    self.dirty = true;

                    close_clicked || minimize_clicked || maximize_clicked
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    /// Set the focused state
    pub fn set_focused(&mut self, focused: bool) {
        if self.focused != focused {
            self.focused = focused;
            self.dirty = true;
        }
    }
}

impl ElementRenderObject for WindowRenderObject {
    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        // Use the window's fixed size
        self.size = Size::new(
            self.size.width.max(constraints.min_width),
            self.size.height.max(constraints.min_height)
        );

        self.calculate_layout();

        // Return the size including decorations
        self.size
    }

    fn size(&self) -> Size {
        self.size
    }

    fn render(&mut self) {
        // Re-render if dirty
        if self.dirty {
            let palette = ColorPalette::default();
            self.draw_titlebar(&palette);
            self.dirty = false;
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn update(&mut self, _new_view: &dyn View) -> UpdateResult {
        // Window properties don't update dynamically for now
        UpdateResult::NoChange
    }

    fn get_buffer(&self) -> Option<&Buffer> {
        self.titlebar_buffer.as_ref()
    }
}
