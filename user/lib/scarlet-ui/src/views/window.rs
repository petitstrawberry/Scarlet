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
use alloc::vec;
use core::any::Any;

use crate::view::View;
use crate::element::{Element, ElementId, ElementRenderObject, LayoutConstraints, UpdateResult, RenderElement};
use crate::geometry::{Size, Point, Rect};
use crate::color::Color;
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
pub struct Window {
    app_id: String,
    title: String,
    size: Size,
    resizable: bool,
    decorated: bool,
}

impl Window {
    /// Create a new Window with a title
    pub fn new(title: impl Into<String>) -> Self {
        let title_str = title.into();
        Self {
            app_id: String::from("com.example.scarletui"),
            title: title_str,
            size: Size::new(800.0, 600.0),
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

}

impl Clone for Window {
    fn clone(&self) -> Self {
        Self {
            app_id: self.app_id.clone(),
            title: self.title.clone(),
            size: self.size,
            resizable: self.resizable,
            decorated: self.decorated,
        }
    }
}

impl View for Window {
    fn create_element(&self) -> Box<dyn Element> {
        // Create WindowRenderObject with titlebar included
        let render_object = WindowRenderObject::new(
            self.title.clone(),
            self.size,
            self.decorated,
        );

        Box::new(RenderElement::new(
            self.clone(),
            render_object,
        ))
    }

    fn listenables(&self) -> Vec<&dyn Listenable> {
        Vec::new()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// WindowRenderObject - renders window with titlebar and background
///
/// This RenderObject owns a single buffer that contains:
/// - Window background (WHITE or custom)
/// - Titlebar with buttons (if decorated)
pub struct WindowRenderObject {
    title: String,
    size: Size,
    decorated: bool,
    focused: bool,
    buffer: Option<Buffer>,
}

impl WindowRenderObject {
    pub fn new(title: String, size: Size, decorated: bool) -> Self {
        Self {
            title,
            size,
            decorated,
            focused: true,
            buffer: None,
        }
    }

    /// Draw the window background and titlebar
    fn draw(&mut self) {
        let width = libm::ceilf(self.size.width) as usize;
        let height = libm::ceilf(self.size.height) as usize;
        let needed = width * height;
        let title = self.title.clone();
        let decorated = self.decorated;
        let focused = self.focused;

        // Create or resize buffer
        if self.buffer.as_ref().map_or(true, |b| b.as_slice().len() < needed) {
            scarlet_std::println!("[WindowRenderObject] Creating buffer: {}x{}", width, height);
            self.buffer = Some(Buffer::from_dimensions(width as u32, height as u32));
        }

        if let Some(ref mut buffer) = self.buffer {
            // Fill entire background with white using u32 slice
            let bg_color = Color::WHITE;
            let bgra = bg_color.to_bgra();
            let slice = buffer.as_mut_slice();
            for i in 0..(width * height) {
                slice[i] = bgra;
            }

            // Draw titlebar if decorated (needs u8 slice for Canvas)
            if decorated {
                Self::draw_titlebar_static(&title, focused, buffer.data_mut(), width, height);
            }
        }
    }

    /// Draw titlebar on the buffer (static method to avoid self borrow issues)
    fn draw_titlebar_static(title: &str, focused: bool, data: &mut [u8], width: usize, _height: usize) {
        let titlebar_height = libm::ceilf(TITLEBAR_HEIGHT) as usize;

        // Titlebar background color
        let base_color = if focused {
            Color::rgb(0.92, 0.92, 0.93)
        } else {
            Color::rgb(0.85, 0.85, 0.86)
        };
        let bgra = base_color.to_bgra();

        // Fill titlebar area
        for y in 0..titlebar_height {
            for x in 0..width {
                let idx = (y * width + x) * 4;
                data[idx] = (bgra & 0xFF) as u8;
                data[idx + 1] = ((bgra >> 8) & 0xFF) as u8;
                data[idx + 2] = ((bgra >> 16) & 0xFF) as u8;
                data[idx + 3] = ((bgra >> 24) & 0xFF) as u8;
            }
        }

        // Draw border at bottom of titlebar
        let border_color = Color::rgb(0.39, 0.39, 0.41);
        let border_bgra = border_color.to_bgra();
        for x in 0..width {
            let idx = ((titlebar_height - 1) * width + x) * 4;
            data[idx] = (border_bgra & 0xFF) as u8;
            data[idx + 1] = ((border_bgra >> 8) & 0xFF) as u8;
            data[idx + 2] = ((border_bgra >> 16) & 0xFF) as u8;
            data[idx + 3] = ((border_bgra >> 24) & 0xFF) as u8;
        }

        // Draw title text
        use crate::graphics::Canvas;
        let mut canvas = Canvas::new(data, width as u32, _height as u32);
        canvas.draw_text_sized(10, 9, title, Color::rgb(0.08, 0.08, 0.09), 13.0);

        // Draw window controls (close, maximize, minimize buttons)
        Self::draw_window_controls_static(&mut canvas, width);

        scarlet_std::println!("[WindowRenderObject] Drew titlebar: {}x{}, title='{}'",
            width, titlebar_height, title);
    }

    /// Draw window control buttons (static method)
    fn draw_window_controls_static(canvas: &mut crate::graphics::Canvas, _width: usize) {
        let button_y = (TITLEBAR_HEIGHT - CLOSE_BUTTON_SIZE) / 2.0;
        let button_start_x = BUTTON_MARGIN;

        // Close button
        let close_rect = Rect::new(
            Point::new(button_start_x, button_y),
            Size::new(CLOSE_BUTTON_SIZE, CLOSE_BUTTON_SIZE)
        );
        Self::draw_button_static(canvas, &close_rect);

        // Maximize button
        let maximize_x = button_start_x + CLOSE_BUTTON_SIZE + BUTTON_SPACING;
        let maximize_rect = Rect::new(
            Point::new(maximize_x, button_y),
            Size::new(CLOSE_BUTTON_SIZE, CLOSE_BUTTON_SIZE)
        );
        Self::draw_button_static(canvas, &maximize_rect);

        // Minimize button
        let minimize_x = maximize_x + CLOSE_BUTTON_SIZE + BUTTON_SPACING;
        let minimize_rect = Rect::new(
            Point::new(minimize_x, button_y),
            Size::new(CLOSE_BUTTON_SIZE, CLOSE_BUTTON_SIZE)
        );
        Self::draw_button_static(canvas, &minimize_rect);
    }

    /// Draw a single button (static method)
    fn draw_button_static(canvas: &mut crate::graphics::Canvas, rect: &Rect) {
        let x = rect.origin.x as i32;
        let y = rect.origin.y as i32;
        let w = libm::ceilf(rect.size.width) as u32;
        let h = libm::ceilf(rect.size.height) as u32;

        // Button background (same as titlebar)
        let color = Color::rgb(0.92, 0.92, 0.93);
        for dy in 0..h {
            for dx in 0..w {
                canvas.put_pixel(x + dx as i32, y + dy as i32, color);
            }
        }

        // Button icon (simple circle for now)
        let cx = x + w as i32 / 2;
        let cy = y + h as i32 / 2;
        let icon_color = Color::rgb(0.2, 0.2, 0.2);

        // Draw X for close button
        let size = 6i32;
        for i in -size..=size {
            canvas.put_pixel(cx + i, cy + i, icon_color);
            canvas.put_pixel(cx + i, cy - i, icon_color);
        }
    }
}

impl ElementRenderObject for WindowRenderObject {
    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        let width = if constraints.max_width.is_finite() && constraints.max_width > 0.0 {
            constraints.max_width.max(constraints.min_width)
        } else {
            self.size.width
        };

        let height = if constraints.max_height.is_finite() && constraints.max_height > 0.0 {
            constraints.max_height.max(constraints.min_height)
        } else {
            self.size.height
        };

        self.size = Size { width, height };
        self.size
    }

    fn size(&self) -> Size {
        self.size
    }

    fn render(&mut self) {
        scarlet_std::println!("[WindowRenderObject] render: size={}x{}, decorated={}",
            self.size.width, self.size.height, self.decorated);
        self.draw();
        scarlet_std::println!("[WindowRenderObject] render: complete, buffer={}",
            self.buffer.is_some());
    }

    fn get_buffer(&self) -> Option<&Buffer> {
        self.buffer.as_ref()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn update(&mut self, _new_view: &dyn View) -> UpdateResult {
        UpdateResult::NoChange
    }
}
