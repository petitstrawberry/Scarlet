//! Button widget

use crate::graphics::{Canvas, Point, Rect};
use crate::widgets::Widget;
use crate::Color;
use scarlet_std::vec::Vec;

/// Button widget
pub struct Button {
    rect: Rect,
    label: Vec<u8>,
    state: ButtonState,
    on_click_handler: Option<fn()>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ButtonState {
    Normal,
    Hovered,
    Pressed,
}

impl Button {
    /// Create a new button
    pub fn new(x: i32, y: i32, width: u32, height: u32, label: &str) -> Self {
        let mut label_vec = Vec::new();
        for ch in label.chars() {
            label_vec.push(ch as u8);
        }

        Self {
            rect: Rect::new(x, y, width, height),
            label: label_vec,
            state: ButtonState::Normal,
            on_click_handler: None,
        }
    }

    /// Set click handler
    pub fn on_click(mut self, handler: fn()) -> Self {
        self.on_click_handler = Some(handler);
        self
    }

    /// Set button state
    pub fn set_hovered(&mut self, hovered: bool) {
        if hovered && self.state != ButtonState::Pressed {
            self.state = ButtonState::Hovered;
        } else if !hovered && self.state == ButtonState::Hovered {
            self.state = ButtonState::Normal;
        }
    }

    /// Set pressed state
    pub fn set_pressed(&mut self, pressed: bool) {
        if pressed {
            self.state = ButtonState::Pressed;
        } else {
            self.state = ButtonState::Normal;
        }
    }
}

impl Widget for Button {
    fn draw(&self, canvas: &mut Canvas) {
        // Choose color based on state
        let bg_color = match self.state {
            ButtonState::Normal => Color::BUTTON_NORMAL,
            ButtonState::Hovered => Color::BUTTON_HOVER,
            ButtonState::Pressed => Color::BUTTON_PRESSED,
        };

        // Fill button background
        canvas.fill_rect(self.rect, bg_color);

        // Draw border
        canvas.draw_rect(self.rect, Color::BORDER);

        // Draw label text (centered)
        let label_str = core::str::from_utf8(&self.label).unwrap_or("");
        let text_width = (label_str.len() * 8) as u32;
        let text_x = self.rect.x + ((self.rect.width as i32 - text_width as i32) / 2);
        let text_y = self.rect.y + ((self.rect.height as i32 - 8) / 2);

        canvas.draw_text(text_x, text_y, label_str, Color::TEXT);
    }

    fn on_click(&mut self, _point: Point) -> bool {
        if let Some(handler) = self.on_click_handler {
            handler();
            true
        } else {
            false
        }
    }

    fn contains(&self, point: Point) -> bool {
        self.rect.contains(point)
    }
}
