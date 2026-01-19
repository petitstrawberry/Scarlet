//! Toggle control
//!
//! A switch/checkbox control for boolean values.

extern crate alloc;
use alloc::string::String;

use crate::context::{ControlFlow, EventCtx, LayoutCtx, PaintCtx, UpdateCtx};
use crate::event::{Event, EventKind, MouseButton};
use crate::graphics::Rect;
use crate::layout::{LayoutConstraints, Size};
use crate::view::id::ViewId;
use crate::view::traits::View;
use crate::color::Color;
use crate::state::data::DataContext;
use scarlet_ui_macros::bindable;
use scarlet_std::fmt;

/// Toggle style
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToggleStyle {
    /// iOS-style switch
    Switch,
    /// Checkbox
    Checkbox,
}

impl Default for ToggleStyle {
    fn default() -> Self {
        Self::Switch
    }
}

/// Toggle view
#[bindable]
pub struct Toggle {
    id: ViewId,
    #[bind]
    is_on: bool,
    label: Option<String>,
    style: ToggleStyle,
    on_color: Color,
    off_color: Color,
    is_hovered: bool,
    is_pressed: bool,
    cached_size: Size,
}

impl Toggle {
    /// Create a new toggle
    pub fn new(is_on: bool) -> Self {
        Self {
            is_on,
            ..Default::default()
        }
    }

    /// Create a toggle with a label
    pub fn with_label(is_on: bool, label: impl Into<String>) -> Self {
        let mut toggle = Self::new(is_on);
        toggle.label = Some(label.into());
        toggle
    }

    /// Get the toggle state
    pub fn is_on(&self) -> bool {
        self.is_on
    }

    /// Set the toggle state
    pub fn set_on(&mut self, is_on: bool) {
        self.is_on = is_on;
        // Update the bound data context if present
        if let Some(ref data) = self.is_on_data {
            data.set(is_on);
        }
    }

    /// Toggle the state
    pub fn toggle(&mut self) {
        self.is_on = !self.is_on;
        // Update the bound data context if present
        if let Some(ref data) = self.is_on_data {
            data.set(self.is_on);
        }
    }

    /// Get the label
    pub fn label(&self) -> Option<&str> {
        self.label.as_ref().map(|s| s.as_str())
    }

    /// Set the label
    pub fn set_label(&mut self, label: impl Into<String>) {
        self.label = Some(label.into());
    }

    /// Set the style
    pub fn set_style(&mut self, style: ToggleStyle) {
        self.style = style;
    }

    /// Get the style
    pub fn style(&self) -> ToggleStyle {
        self.style
    }

    /// Set the on/off colors
    pub fn set_colors(&mut self, on: Color, off: Color) {
        self.on_color = on;
        self.off_color = off;
    }

    /// Handle mouse event
    fn handle_mouse_event(&mut self, event: &Event) -> bool {
        match &event.kind {
            EventKind::MouseMove { .. } => {
                // Simplified hover detection
                false
            }
            EventKind::MouseDown { button, .. } => {
                if *button == MouseButton::Left {
                    self.is_pressed = true;
                    return true;
                }
                false
            }
            EventKind::MouseUp { button, .. } => {
                if *button == MouseButton::Left && self.is_pressed {
                    self.is_pressed = false;
                    self.toggle();
                    return true;
                }
                false
            }
            _ => false,
        }
    }

    /// Calculate toggle size
    fn calculate_size(&self) -> Size {
        match self.style {
            ToggleStyle::Switch => {
                // Switch: 51x31 (iOS standard)
                Size::new(51, 31)
            }
            ToggleStyle::Checkbox => {
                // Checkbox: 20x20
                Size::new(20, 20)
            }
        }
    }
}

impl crate::view::render::RenderObject for Toggle {
    fn id(&self) -> ViewId {
        self.id
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn layout(&mut self, _ctx: &mut LayoutCtx, constraints: LayoutConstraints) -> Size {
        let toggle_size = self.calculate_size();

        // Add space for label if present
        let mut width = toggle_size.width;
        if self.label.is_some() {
            // Add padding and estimated text width
            width += 10 + 100; // 10px padding + estimated text width
        }

        let height = toggle_size.height;

        let size = Size::new(
            width.max(constraints.min_width).min(constraints.max_width),
            height.max(constraints.min_height).min(constraints.max_height),
        );

        self.cached_size = size;
        size
    }

    fn draw(&self, ctx: &mut PaintCtx, frame: Rect) {
        match self.style {
            ToggleStyle::Switch => {
                // iOS-style switch
                let switch_width = 51;
                let switch_height = 31;
                let thumb_radius = 13;
                let corner_radius = 15;

                // Background (on/off color)
                let bg_color = if self.is_on { self.on_color } else { self.off_color };
                ctx.canvas.fill_rounded_rect(
                    frame.x,
                    frame.y + (frame.height as i32 - switch_height as i32) / 2,
                    switch_width,
                    switch_height,
                    corner_radius,
                    bg_color,
                );

                // Thumb position
                let thumb_x = if self.is_on {
                    frame.x + switch_width as i32 - thumb_radius as i32 - 3
                } else {
                    frame.x + thumb_radius as i32 + 3
                };
                let thumb_y = frame.y + frame.height as i32 / 2;

                // Draw thumb (circle)
                ctx.canvas.fill_circle(thumb_x, thumb_y, thumb_radius, Color::WHITE);
            }
            ToggleStyle::Checkbox => {
                // Traditional checkbox
                let box_size = 20;
                let box_y = frame.y + (frame.height as i32 - box_size as i32) / 2;
                let corner_radius = 3;

                // Background with rounded corners
                ctx.canvas.fill_rounded_rect(
                    frame.x,
                    box_y,
                    box_size,
                    box_size,
                    corner_radius,
                    Color::WHITE,
                );

                // Border
                ctx.canvas.draw_rounded_rect(
                    frame.x,
                    box_y,
                    box_size,
                    box_size,
                    corner_radius,
                    Color::rgb(180, 180, 180),
                );

                // Check mark if checked (3px thick)
                if self.is_on {
                    let check_color = self.on_color;

                    // Draw checkmark as two lines
                    // Line 1: from top-left to center
                    ctx.canvas.draw_line(
                        frame.x + 4,
                        box_y + 10,
                        frame.x + 8,
                        box_y + 14,
                        check_color,
                    );
                    ctx.canvas.draw_line(
                        frame.x + 4,
                        box_y + 9,
                        frame.x + 8,
                        box_y + 13,
                        check_color,
                    );
                    ctx.canvas.draw_line(
                        frame.x + 4,
                        box_y + 11,
                        frame.x + 8,
                        box_y + 15,
                        check_color,
                    );

                    // Line 2: from center to bottom-right
                    ctx.canvas.draw_line(
                        frame.x + 8,
                        box_y + 14,
                        frame.x + 16,
                        box_y + 6,
                        check_color,
                    );
                    ctx.canvas.draw_line(
                        frame.x + 8,
                        box_y + 13,
                        frame.x + 16,
                        box_y + 5,
                        check_color,
                    );
                    ctx.canvas.draw_line(
                        frame.x + 8,
                        box_y + 15,
                        frame.x + 16,
                        box_y + 7,
                        check_color,
                    );
                }
            }
        }

        // Draw label if present
        if let Some(ref label) = self.label {
            let label_x = frame.x + match self.style {
                ToggleStyle::Switch => 51 + 8,
                ToggleStyle::Checkbox => 20 + 8,
            };
            ctx.canvas.draw_text(
                label_x,
                frame.y + (frame.height as i32 - 16) / 2,
                label,
                Color::rgb(200, 200, 200),
            );
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &Event) -> ControlFlow {
        if self.handle_mouse_event(event) {
            ctx.request_paint();
        }
        ControlFlow::Continue
    }

    fn update(&mut self, _ctx: &mut UpdateCtx) {
        // Toggle doesn't need periodic updates
    }
}

impl fmt::Debug for Toggle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Toggle")
            .field("is_on", &self.is_on)
            .field("label", &self.label)
            .field("style", &self.style)
            .field("has_is_on_binding", &self.is_on_data.is_some())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_toggle_new() {
        let toggle = Toggle::new(false);
        assert!(!toggle.is_on());
        assert_eq!(toggle.style(), ToggleStyle::Switch);
    }

    #[test]
    fn test_toggle_with_label() {
        let toggle = Toggle::with_label(true, "Enable feature");
        assert!(toggle.is_on());
        assert_eq!(toggle.label(), Some("Enable feature"));
    }

    #[test]
    fn test_toggle_toggle() {
        let mut toggle = Toggle::new(false);
        toggle.toggle();
        assert!(toggle.is_on());
        toggle.toggle();
        assert!(!toggle.is_on());
    }

    #[test]
    fn test_toggle_set_on() {
        let mut toggle = Toggle::new(false);
        toggle.set_on(true);
        assert!(toggle.is_on());
    }

    #[test]
    fn test_toggle_set_style() {
        let mut toggle = Toggle::new(false);
        toggle.set_style(ToggleStyle::Checkbox);
        assert_eq!(toggle.style(), ToggleStyle::Checkbox);
    }
}
