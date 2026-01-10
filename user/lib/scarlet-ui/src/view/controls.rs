//! Control views (basic UI widgets)

use super::traits::{View, Size};
use crate::graphics::{measure_text_sized, Canvas, Rect};
use crate::Color;
use crate::event::{Event, EventKind, MouseButton};
use scarlet_std::string::String;

/// Text label view
pub struct Label {
    text: String,
    color: Color,
    font_size: u32,
}

impl Label {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            color: Color::WHITE,
            font_size: 16,
        }
    }

    /// Set text color
    pub fn color(mut self, color: Color) -> Self {
        self.color = color;
        self
    }

    /// Set font size
    pub fn font_size(mut self, size: u32) -> Self {
        self.font_size = size;
        self
    }

    /// Update text content
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
    }
}

impl View for Label {
    fn layout(&mut self, _available: Size) -> Size {
        let (w, h) = measure_text_sized(&self.text, self.font_size as f32);
        Size::new(w, h)
    }

    fn draw(&self, canvas: &mut Canvas, frame: Rect) {
        canvas.draw_text_sized(frame.x, frame.y, &self.text, self.color, self.font_size as f32);
    }
}

/// Button view with click action
pub struct Button<F: FnMut() + 'static> {
    label: String,
    on_click: F,
    background: Color,
    text_color: Color,
    padding: u32,
    is_hovered: bool,
    is_pressed: bool,
}

impl<F: FnMut() + 'static> Button<F> {
    pub fn new(label: impl Into<String>, on_click: F) -> Self {
        Self {
            label: label.into(),
            on_click,
            background: Color::rgb(60, 60, 60),
            text_color: Color::WHITE,
            padding: 12,
            is_hovered: false,
            is_pressed: false,
        }
    }

    /// Set background color
    pub fn background(mut self, color: Color) -> Self {
        self.background = color;
        self
    }

    /// Set text color
    pub fn text_color(mut self, color: Color) -> Self {
        self.text_color = color;
        self
    }

    /// Set padding
    pub fn padding(mut self, padding: u32) -> Self {
        self.padding = padding;
        self
    }

    fn current_background(&self) -> Color {
        if self.is_pressed {
            Color::rgb(
                self.background.r.saturating_sub(30),
                self.background.g.saturating_sub(30),
                self.background.b.saturating_sub(30),
            )
        } else if self.is_hovered {
            Color::rgb(
                self.background.r.saturating_add(20),
                self.background.g.saturating_add(20),
                self.background.b.saturating_add(20),
            )
        } else {
            self.background
        }
    }
}

impl<F: FnMut() + 'static> View for Button<F> {
    fn layout(&mut self, _available: Size) -> Size {
        // Text size + padding
        let (text_width, text_height) = measure_text_sized(&self.label, 16.0);
        Size::new(
            text_width + self.padding * 2,
            text_height + self.padding * 2,
        )
    }

    fn draw(&self, canvas: &mut Canvas, frame: Rect) {
        // Draw background
        canvas.fill_rect(frame.x, frame.y, frame.width, frame.height, self.current_background());

        // Draw border
        let border_color = if self.is_hovered {
            Color::rgb(150, 150, 150)
        } else {
            Color::rgb(100, 100, 100)
        };
        canvas.draw_rect(frame.x, frame.y, frame.width, frame.height, border_color);

        // Draw text centered
        let text_x = frame.x + self.padding as i32;
        let text_y = frame.y + self.padding as i32;
        canvas.draw_text(text_x, text_y, &self.label, self.text_color);
    }

    fn on_event(&mut self, event: &mut Event, frame: Rect) -> bool {
        match event.kind {
            EventKind::MouseMove => {
                let was_hovered = self.is_hovered;
                self.is_hovered = frame.contains(event.x(), event.y());
                was_hovered != self.is_hovered // Return true if state changed
            }
            EventKind::MouseDown { button: MouseButton::Left } => {
                if frame.contains(event.x(), event.y()) {
                    self.is_pressed = true;
                    true
                } else {
                    false
                }
            }
            EventKind::MouseUp { button: MouseButton::Left } => {
                if self.is_pressed {
                    self.is_pressed = false;
                    if frame.contains(event.x(), event.y()) {
                        (self.on_click)();
                    }
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }
}

/// Flexible spacer - takes up available space
pub struct Spacer {
    min_length: u32,
}

impl Spacer {
    pub fn new() -> Self {
        Self { min_length: 0 }
    }

    /// Set minimum length
    pub fn min_length(mut self, length: u32) -> Self {
        self.min_length = length;
        self
    }
}

impl Default for Spacer {
    fn default() -> Self {
        Self::new()
    }
}

impl View for Spacer {
    fn flex_factor(&self) -> u32 {
        1
    }

    fn layout(&mut self, available: Size) -> Size {
        // Spacer should only expand along the parent's main axis.
        // Stacks pass `0` for the cross-axis when laying out flex children.
        if available.width == 0 {
            // Vertical spacer (in VStack)
            Size::new(0, available.height.max(self.min_length))
        } else if available.height == 0 {
            // Horizontal spacer (in HStack)
            Size::new(available.width.max(self.min_length), 0)
        } else {
            // Fallback: if used outside stacks, behave conservatively.
            Size::new(
                available.width.max(self.min_length),
                available.height.max(self.min_length),
            )
        }
    }

    fn draw(&self, _canvas: &mut Canvas, _frame: Rect) {
        // Spacer is invisible
    }
}

/// Rectangle view - simple colored rectangle
pub struct RectView {
    color: Color,
    width: Option<u32>,
    height: Option<u32>,
}

impl RectView {
    pub fn new(color: Color) -> Self {
        Self {
            color,
            width: None,
            height: None,
        }
    }

    /// Set fixed width
    pub fn width(mut self, width: u32) -> Self {
        self.width = Some(width);
        self
    }

    /// Set fixed height
    pub fn height(mut self, height: u32) -> Self {
        self.height = Some(height);
        self
    }
}

impl View for RectView {
    fn layout(&mut self, available: Size) -> Size {
        Size::new(
            self.width.unwrap_or(available.width),
            self.height.unwrap_or(available.height),
        )
    }

    fn draw(&self, canvas: &mut Canvas, frame: Rect) {
        canvas.fill_rect(frame.x, frame.y, frame.width, frame.height, self.color);
    }
}
