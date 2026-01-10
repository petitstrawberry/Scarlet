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

/// TextField - text input control
pub struct TextField {
    text: String,
    placeholder: String,
    is_focused: bool,
    cursor_pos: usize,
    text_color: Color,
    background: Color,
    border_color: Color,
    padding: u32,
}

impl TextField {
    pub fn new(placeholder: impl Into<String>) -> Self {
        Self {
            text: String::new(),
            placeholder: placeholder.into(),
            is_focused: false,
            cursor_pos: 0,
            text_color: Color::BLACK,
            background: Color::WHITE,
            border_color: Color::rgb(180, 180, 180),
            padding: 8,
        }
    }

    /// Get the current text
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Set the text
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.cursor_pos = self.text.len();
    }

    /// Set text color
    pub fn text_color(mut self, color: Color) -> Self {
        self.text_color = color;
        self
    }

    /// Set background color
    pub fn background(mut self, color: Color) -> Self {
        self.background = color;
        self
    }

    /// Set border color
    pub fn border_color(mut self, color: Color) -> Self {
        self.border_color = color;
        self
    }
}

impl View for TextField {
    fn layout(&mut self, available: Size) -> Size {
        // Fixed height, flexible width
        let width = available.width.max(150);
        let height = 32;
        Size::new(width, height)
    }

    fn draw(&self, canvas: &mut Canvas, frame: Rect) {
        // Background
        canvas.fill_rect(frame.x, frame.y, frame.width, frame.height, self.background);
        
        // Border (thicker if focused)
        let border_color = if self.is_focused {
            Color::rgb(100, 150, 255)
        } else {
            self.border_color
        };
        canvas.draw_rect(frame.x, frame.y, frame.width, frame.height, border_color);
        if self.is_focused {
            canvas.draw_rect(frame.x + 1, frame.y + 1, frame.width - 2, frame.height - 2, border_color);
        }

        // Text or placeholder
        let display_text = if self.text.is_empty() {
            &self.placeholder
        } else {
            &self.text
        };
        
        let text_color = if self.text.is_empty() {
            Color::rgb(150, 150, 150)
        } else {
            self.text_color
        };

        if !display_text.is_empty() {
            canvas.draw_text(
                frame.x + self.padding as i32,
                frame.y + self.padding as i32,
                display_text,
                text_color,
            );
        }
        
        // Draw cursor if focused
        if self.is_focused {
            let (cursor_x, _) = measure_text_sized(&self.text[..self.cursor_pos], 16.0);
            canvas.fill_rect(
                frame.x + self.padding as i32 + cursor_x as i32,
                frame.y + self.padding as i32,
                2,
                16,
                self.text_color,
            );
        }
    }

    fn on_event(&mut self, event: &mut Event, frame: Rect) -> bool {
        match event.kind {
            EventKind::MouseDown { button: MouseButton::Left } => {
                let was_focused = self.is_focused;
                self.is_focused = frame.contains(event.x(), event.y());
                was_focused != self.is_focused
            }
            _ => false,
        }
    }
}

/// CheckBox - boolean toggle control
pub struct CheckBox {
    checked: bool,
    label: String,
    on_toggle: Option<Box<dyn FnMut(bool) + 'static>>,
    check_color: Color,
    label_color: Color,
}

impl CheckBox {
    pub fn new(label: impl Into<String>, checked: bool) -> Self {
        Self {
            checked,
            label: label.into(),
            on_toggle: None,
            check_color: Color::rgb(50, 150, 255),
            label_color: Color::BLACK,
        }
    }

    /// Set the toggle callback
    pub fn on_toggle<F: FnMut(bool) + 'static>(mut self, callback: F) -> Self {
        self.on_toggle = Some(Box::new(callback));
        self
    }

    /// Check if the checkbox is checked
    pub fn is_checked(&self) -> bool {
        self.checked
    }

    /// Set checked state
    pub fn set_checked(&mut self, checked: bool) {
        self.checked = checked;
    }

    /// Set check color
    pub fn check_color(mut self, color: Color) -> Self {
        self.check_color = color;
        self
    }

    /// Set label color
    pub fn label_color(mut self, color: Color) -> Self {
        self.label_color = color;
        self
    }
}

impl View for CheckBox {
    fn layout(&mut self, _available: Size) -> Size {
        let (label_w, label_h) = measure_text_sized(&self.label, 16.0);
        Size::new(24 + 8 + label_w, 24.max(label_h))
    }

    fn draw(&self, canvas: &mut Canvas, frame: Rect) {
        // Draw checkbox box
        let box_size = 20;
        let box_y = frame.y + (frame.height as i32 - box_size as i32) / 2;
        
        // Background
        canvas.fill_rect(frame.x, box_y, box_size, box_size, Color::WHITE);
        // Border
        canvas.draw_rect(frame.x, box_y, box_size, box_size, Color::rgb(180, 180, 180));
        
        // Check mark if checked
        if self.checked {
            // Draw a simple checkmark
            for i in 0..3 {
                // Left part of check
                for j in 0..5 {
                    canvas.put_pixel(
                        frame.x + 5 + j,
                        box_y + 10 + j - i,
                        self.check_color,
                    );
                }
                // Right part of check
                for j in 0..8 {
                    canvas.put_pixel(
                        frame.x + 10 + j,
                        box_y + 14 - j - i,
                        self.check_color,
                    );
                }
            }
        }
        
        // Draw label
        canvas.draw_text(
            frame.x + box_size as i32 + 8,
            frame.y + (frame.height as i32 - 16) / 2,
            &self.label,
            self.label_color,
        );
    }

    fn on_event(&mut self, event: &mut Event, frame: Rect) -> bool {
        match event.kind {
            EventKind::MouseDown { button: MouseButton::Left } => {
                if frame.contains(event.x(), event.y()) {
                    self.checked = !self.checked;
                    if let Some(ref mut callback) = self.on_toggle {
                        callback(self.checked);
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

/// Slider - value selection control
pub struct Slider {
    value: f32,
    min: f32,
    max: f32,
    on_change: Option<Box<dyn FnMut(f32) + 'static>>,
    track_color: Color,
    thumb_color: Color,
    is_dragging: bool,
}

impl Slider {
    pub fn new(value: f32, min: f32, max: f32) -> Self {
        Self {
            value: value.clamp(min, max),
            min,
            max,
            on_change: None,
            track_color: Color::rgb(200, 200, 200),
            thumb_color: Color::rgb(50, 150, 255),
            is_dragging: false,
        }
    }

    /// Set the change callback
    pub fn on_change<F: FnMut(f32) + 'static>(mut self, callback: F) -> Self {
        self.on_change = Some(Box::new(callback));
        self
    }

    /// Get current value
    pub fn value(&self) -> f32 {
        self.value
    }

    /// Set value
    pub fn set_value(&mut self, value: f32) {
        self.value = value.clamp(self.min, self.max);
    }

    fn update_value_from_mouse(&mut self, mouse_x: i32, frame: Rect) {
        let thumb_radius = 8;
        let track_start = frame.x + thumb_radius;
        let track_end = frame.x + frame.width as i32 - thumb_radius;
        let track_width = (track_end - track_start) as f32;
        
        if track_width > 0.0 {
            let position = (mouse_x - track_start).max(0).min(track_width as i32) as f32;
            let ratio = position / track_width;
            let new_value = self.min + ratio * (self.max - self.min);
            self.value = new_value.clamp(self.min, self.max);
            
            if let Some(ref mut callback) = self.on_change {
                callback(self.value);
            }
        }
    }
}

impl View for Slider {
    fn layout(&mut self, available: Size) -> Size {
        Size::new(available.width.max(100), 32)
    }

    fn draw(&self, canvas: &mut Canvas, frame: Rect) {
        let track_height = 4;
        let thumb_radius = 8;
        let track_y = frame.y + (frame.height as i32 - track_height as i32) / 2;
        
        // Draw track
        canvas.fill_rect(
            frame.x + thumb_radius,
            track_y,
            frame.width - thumb_radius as u32 * 2,
            track_height,
            self.track_color,
        );
        
        // Calculate thumb position
        let track_width = frame.width as f32 - thumb_radius as f32 * 2.0;
        let ratio = if self.max > self.min {
            (self.value - self.min) / (self.max - self.min)
        } else {
            0.0
        };
        let thumb_x = frame.x + thumb_radius + (track_width * ratio) as i32;
        let thumb_y = frame.y + frame.height as i32 / 2;
        
        // Draw thumb (simple circle approximation)
        for dy in -(thumb_radius as i32)..=(thumb_radius as i32) {
            for dx in -(thumb_radius as i32)..=(thumb_radius as i32) {
                if dx * dx + dy * dy <= (thumb_radius * thumb_radius) as i32 {
                    canvas.put_pixel(thumb_x + dx, thumb_y + dy, self.thumb_color);
                }
            }
        }
    }

    fn on_event(&mut self, event: &mut Event, frame: Rect) -> bool {
        match event.kind {
            EventKind::MouseDown { button: MouseButton::Left } => {
                if frame.contains(event.x(), event.y()) {
                    self.is_dragging = true;
                    self.update_value_from_mouse(event.x(), frame);
                    true
                } else {
                    false
                }
            }
            EventKind::MouseMove => {
                if self.is_dragging {
                    self.update_value_from_mouse(event.x(), frame);
                    true
                } else {
                    false
                }
            }
            EventKind::MouseUp { button: MouseButton::Left } => {
                if self.is_dragging {
                    self.is_dragging = false;
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }
}

/// ProgressBar - progress indicator
pub struct ProgressBar {
    progress: f32, // 0.0 to 1.0
    track_color: Color,
    fill_color: Color,
    height: u32,
}

impl ProgressBar {
    pub fn new(progress: f32) -> Self {
        Self {
            progress: progress.clamp(0.0, 1.0),
            track_color: Color::rgb(230, 230, 230),
            fill_color: Color::rgb(50, 150, 255),
            height: 16,
        }
    }

    /// Set progress (0.0 to 1.0)
    pub fn set_progress(&mut self, progress: f32) {
        self.progress = progress.clamp(0.0, 1.0);
    }

    /// Get progress
    pub fn progress(&self) -> f32 {
        self.progress
    }

    /// Set track color
    pub fn track_color(mut self, color: Color) -> Self {
        self.track_color = color;
        self
    }

    /// Set fill color
    pub fn fill_color(mut self, color: Color) -> Self {
        self.fill_color = color;
        self
    }

    /// Set height
    pub fn height(mut self, height: u32) -> Self {
        self.height = height;
        self
    }
}

impl View for ProgressBar {
    fn layout(&mut self, available: Size) -> Size {
        Size::new(available.width.max(100), self.height)
    }

    fn draw(&self, canvas: &mut Canvas, frame: Rect) {
        // Draw track
        canvas.fill_rect(frame.x, frame.y, frame.width, frame.height, self.track_color);
        
        // Draw filled portion
        let fill_width = (frame.width as f32 * self.progress) as u32;
        if fill_width > 0 {
            canvas.fill_rect(frame.x, frame.y, fill_width, frame.height, self.fill_color);
        }
        
        // Draw border
        canvas.draw_rect(frame.x, frame.y, frame.width, frame.height, Color::rgb(180, 180, 180));
    }
}

/// Toggle - boolean switch control
pub struct Toggle {
    enabled: bool,
    on_toggle: Option<Box<dyn FnMut(bool) + 'static>>,
    on_color: Color,
    off_color: Color,
    thumb_color: Color,
    is_hovered: bool,
}

impl Toggle {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            on_toggle: None,
            on_color: Color::rgb(50, 200, 100),
            off_color: Color::rgb(180, 180, 180),
            thumb_color: Color::WHITE,
            is_hovered: false,
        }
    }

    /// Set the toggle callback
    pub fn on_toggle<F: FnMut(bool) + 'static>(mut self, callback: F) -> Self {
        self.on_toggle = Some(Box::new(callback));
        self
    }

    /// Check if enabled
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Set enabled state
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }
}

impl View for Toggle {
    fn layout(&mut self, _available: Size) -> Size {
        Size::new(50, 28)
    }

    fn draw(&self, canvas: &mut Canvas, frame: Rect) {
        let track_height = 24;
        let track_radius = track_height / 2;
        let thumb_radius = 10;
        
        // Track background color
        let track_color = if self.enabled {
            self.on_color
        } else {
            self.off_color
        };
        
        // Draw track (rounded rect)
        canvas.fill_rect(
            frame.x,
            frame.y + 2,
            frame.width,
            track_height,
            track_color,
        );
        
        // Calculate thumb position
        let thumb_x = if self.enabled {
            frame.x + frame.width as i32 - track_radius as i32 - 2
        } else {
            frame.x + track_radius as i32 + 2
        };
        let thumb_y = frame.y + frame.height as i32 / 2;
        
        // Draw thumb
        for dy in -(thumb_radius as i32)..=(thumb_radius as i32) {
            for dx in -(thumb_radius as i32)..=(thumb_radius as i32) {
                if dx * dx + dy * dy <= (thumb_radius * thumb_radius) as i32 {
                    canvas.put_pixel(thumb_x + dx, thumb_y + dy, self.thumb_color);
                }
            }
        }
        
        // Draw border if hovered
        if self.is_hovered {
            canvas.draw_rect(frame.x, frame.y + 2, frame.width, track_height, Color::rgb(100, 100, 100));
        }
    }

    fn on_event(&mut self, event: &mut Event, frame: Rect) -> bool {
        match event.kind {
            EventKind::MouseMove => {
                let was_hovered = self.is_hovered;
                self.is_hovered = frame.contains(event.x(), event.y());
                was_hovered != self.is_hovered
            }
            EventKind::MouseDown { button: MouseButton::Left } => {
                if frame.contains(event.x(), event.y()) {
                    self.enabled = !self.enabled;
                    if let Some(ref mut callback) = self.on_toggle {
                        callback(self.enabled);
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
