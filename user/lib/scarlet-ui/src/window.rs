//! Window with Client-Side Decorations (CSD)

use crate::{Canvas, Color, Rect};
use scarlet_std::vec::Vec;
use sws_client::Connection;

const TITLEBAR_HEIGHT: u32 = 24;
const BORDER_WIDTH: u32 = 1;
const CLOSE_BUTTON_SIZE: u32 = 16;
const CLOSE_BUTTON_MARGIN: u32 = 4;

/// Window with CSD (Client-Side Decorations)
pub struct Window {
    surface_id: u32,
    width: u32,
    height: u32,
    title: Vec<u8>,
    buffer: &'static mut [u8],
    mouse_x: i32,
    mouse_y: i32,
    close_button_hovered: bool,
}

impl Window {
    /// Create a new window (called by Application)
    pub(crate) fn new(
        connection: &mut Connection,
        surface_id: u32,
        title: &str,
        width: u32,
        height: u32,
    ) -> Result<Self, &'static str> {
        // Get the surface buffer from the connection
        let surface = connection
            .surface_mut(surface_id)
            .ok_or("Surface not found")?;

        // Get buffer reference (we need to transmute lifetime for CSD ownership)
        let buffer = unsafe {
            let ptr = surface.buffer_mut().as_mut_ptr();
            let len = surface.buffer_mut().len();
            core::slice::from_raw_parts_mut(ptr, len)
        };

        let mut title_vec = Vec::new();
        for ch in title.chars() {
            title_vec.push(ch as u8);
        }

        let mut window = Self {
            surface_id,
            width,
            height,
            title: title_vec,
            buffer,
            mouse_x: 0,
            mouse_y: 0,
            close_button_hovered: false,
        };

        // Draw initial decorations
        window.draw_decorations();

        Ok(window)
    }

    /// Get surface ID
    #[inline]
    pub fn surface_id(&self) -> u32 {
        self.surface_id
    }

    /// Get window width
    #[inline]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Get window height
    #[inline]
    pub fn height(&self) -> u32 {
        self.height
    }


    /// Draw window decorations (titlebar, border, close button)
    fn draw_decorations(&mut self) {
        let mut canvas = Canvas::new(self.buffer, self.width, self.height);

        // Draw titlebar
        canvas.fill_rect(0, 0, self.width, TITLEBAR_HEIGHT, Color::TITLEBAR);

        // Draw title text
        let title_str = core::str::from_utf8(&self.title).unwrap_or("");
        canvas.draw_text(8, 8, title_str, Color::TITLEBAR_TEXT);

        // Draw close button
        let close_x = (self.width - CLOSE_BUTTON_SIZE - CLOSE_BUTTON_MARGIN) as i32;
        let close_y = CLOSE_BUTTON_MARGIN as i32;
        let close_color = if self.close_button_hovered {
            Color::RED
        } else {
            Color::LIGHT_GRAY
        };

        canvas.fill_rect(close_x, close_y, CLOSE_BUTTON_SIZE, CLOSE_BUTTON_SIZE, close_color);

        // Draw X mark
        for i in 0..CLOSE_BUTTON_SIZE {
            canvas.put_pixel(close_x + i as i32, close_y + i as i32, Color::BLACK);
            canvas.put_pixel(
                close_x + i as i32,
                close_y + (CLOSE_BUTTON_SIZE - 1 - i) as i32,
                Color::BLACK,
            );
        }

        // Draw borders
        canvas.draw_rect(0, 0, self.width, self.height, Color::BORDER);
    }

    /// Update mouse position and check for hover effects
    pub fn update_mouse(&mut self, x: i32, y: i32) {
        self.mouse_x = x;
        self.mouse_y = y;

        // Check if hovering over close button
        let close_x = (self.width - CLOSE_BUTTON_SIZE - CLOSE_BUTTON_MARGIN) as i32;
        let close_y = CLOSE_BUTTON_MARGIN as i32;
        let close_rect = Rect::new(close_x, close_y, CLOSE_BUTTON_SIZE, CLOSE_BUTTON_SIZE);

        let was_hovered = self.close_button_hovered;
        self.close_button_hovered = close_rect.contains(x, y);

        if was_hovered != self.close_button_hovered {
            // Redraw decorations if hover state changed
            self.draw_decorations();
        }
    }

    /// Check if close button was clicked
    pub fn is_close_clicked(&self, x: i32, y: i32) -> bool {
        let close_x = (self.width - CLOSE_BUTTON_SIZE - CLOSE_BUTTON_MARGIN) as i32;
        let close_y = CLOSE_BUTTON_MARGIN as i32;
        let close_rect = Rect::new(close_x, close_y, CLOSE_BUTTON_SIZE, CLOSE_BUTTON_SIZE);
        close_rect.contains(x, y)
    }

    /// Update mouse position
    pub fn update_mouse_position(&mut self, x: i32, y: i32) {
        if x >= 0 {
            self.mouse_x = x;
        }
        if y >= 0 {
            self.mouse_y = y;
        }
    }

    /// Get current mouse X position
    pub fn mouse_x(&self) -> i32 {
        self.mouse_x
    }

    /// Get current mouse Y position
    pub fn mouse_y(&self) -> i32 {
        self.mouse_y
    }

    /// Get canvas for content area (excluding decorations)
    pub fn canvas(&mut self) -> Canvas {
        let content_offset = ((TITLEBAR_HEIGHT + BORDER_WIDTH) * self.width * 4) as usize;
        let content_height = self.height - TITLEBAR_HEIGHT - BORDER_WIDTH * 2;
        let content_size = (self.width * content_height * 4) as usize;

        let content_buffer = &mut self.buffer[content_offset..content_offset + content_size];
        Canvas::new(content_buffer, self.width, content_height)
    }

    /// Get full canvas (including decorations)
    pub fn full_canvas(&mut self) -> Canvas {
        Canvas::new(self.buffer, self.width, self.height)
    }
}

