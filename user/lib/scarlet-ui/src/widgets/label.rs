//! Label widget

use crate::graphics::{Canvas, Point, Rect};
use crate::widgets::Widget;
use crate::Color;
use scarlet_std::vec::Vec;

/// Label widget (text display)
pub struct Label {
    x: i32,
    y: i32,
    text: Vec<u8>,
    color: Color,
}

impl Label {
    /// Create a new label
    pub fn new(x: i32, y: i32, text: &str, color: Color) -> Self {
        let mut text_vec = Vec::new();
        for ch in text.chars() {
            text_vec.push(ch as u8);
        }

        Self {
            x,
            y,
            text: text_vec,
            color,
        }
    }

    /// Update label text
    pub fn set_text(&mut self, text: &str) {
        self.text.clear();
        for ch in text.chars() {
            self.text.push(ch as u8);
        }
    }
}

impl Widget for Label {
    fn draw(&self, canvas: &mut Canvas) {
        let text_str = core::str::from_utf8(&self.text).unwrap_or("");
        canvas.draw_text(self.x, self.y, text_str, self.color);
    }

    fn contains(&self, point: Point) -> bool {
        // Labels are typically not clickable, but provide bounds for consistency
        let width = (self.text.len() * 8) as u32;
        let rect = Rect::new(self.x, self.y, width, 8);
        rect.contains_point(point)
    }
}
