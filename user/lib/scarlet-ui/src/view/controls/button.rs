//! Button control
//!
//! A clickable button with text label.

extern crate alloc;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::boxed::Box;

use crate::context::{ControlFlow, EventCtx, LayoutCtx, PaintCtx, UpdateCtx};
use crate::event::{Event, EventKind, MouseButton};
use crate::graphics::Rect;
use crate::layout::{LayoutConstraints, Size};
use crate::view::id::ViewId;
use crate::view::traits::View;
use crate::color::Color;
use crate::view::controls::text::Text;
use scarlet_std::fmt;

/// Action to perform when button is clicked
pub type ButtonAction = Arc<dyn Fn() + 'static>;

/// Button view
pub struct Button {
    id: ViewId,
    label: Text,
    action: Option<ButtonAction>,
    background_color: Color,
    hover_color: Color,
    pressed_color: Color,
    is_hovered: bool,
    is_pressed: bool,
    cached_size: Size,
}

impl Button {
    /// Create a new button with a label
    pub fn new(label: impl AsRef<str>) -> Self {
        let label = Text::new(label.as_ref());
        Self {
            id: ViewId::new(),
            label,
            action: None,
            background_color: Color::BUTTON_NORMAL,
            hover_color: Color::BUTTON_HOVER,
            pressed_color: Color::BUTTON_PRESSED,
            is_hovered: false,
            is_pressed: false,
            cached_size: Size::ZERO,
        }
    }

    /// Set the button action
    pub fn set_action(&mut self, action: ButtonAction) {
        self.action = Some(action);
    }

    /// Get the label text
    pub fn label(&self) -> &str {
        self.label.text()
    }

    /// Set the label text
    pub fn set_label(&mut self, label: impl AsRef<str>) {
        self.label.set_text(label.as_ref());
    }

    /// Set the background colors for different states
    pub fn set_colors(&mut self, normal: Color, hover: Color, pressed: Color) {
        self.background_color = normal;
        self.hover_color = hover;
        self.pressed_color = pressed;
    }

    /// Get whether the button is currently hovered
    pub fn is_hovered(&self) -> bool {
        self.is_hovered
    }

    /// Get whether the button is currently pressed
    pub fn is_pressed(&self) -> bool {
        self.is_pressed
    }

    /// Handle mouse event
    fn handle_mouse_event(&mut self, event: &Event) -> bool {
        match &event.kind {
            EventKind::MouseMove => {
                // TODO: Check if mouse is over button bounds
                // For now, always consider hovered
                let was_hovered = self.is_hovered;
                self.is_hovered = true;
                was_hovered != self.is_hovered
            }
            EventKind::MouseDown { button, .. } => {
                if *button == MouseButton::Left && self.is_hovered {
                    self.is_pressed = true;
                    return true;
                }
                false
            }
            EventKind::MouseUp { button, .. } => {
                if *button == MouseButton::Left && self.is_pressed {
                    self.is_pressed = false;
                    if self.is_hovered {
                        // Trigger action
                        if let Some(action) = &self.action {
                            action();
                        }
                    }
                    return true;
                }
                false
            }
            _ => false,
        }
    }

    /// Get current background color based on state
    fn current_color(&self) -> Color {
        if self.is_pressed {
            self.pressed_color
        } else if self.is_hovered {
            self.hover_color
        } else {
            self.background_color
        }
    }
}

impl View for Button {
    fn id(&self) -> ViewId {
        self.id
    }

    fn layout(&mut self, ctx: &mut LayoutCtx, constraints: LayoutConstraints) -> Size {
        // Add padding for button appearance
        let padding = 10;
        let min_width = constraints.min_width.saturating_add(padding * 2);
        let min_height = constraints.min_height.saturating_add(padding * 2);

        let child_constraints = LayoutConstraints::new(
            min_width,
            constraints.max_width.saturating_sub(padding * 2),
            min_height,
            constraints.max_height.saturating_sub(padding * 2),
        );

        let label_size = self.label.layout(ctx, child_constraints);
        let width = label_size.width.saturating_add(padding * 2);
        let height = label_size.height.saturating_add(padding * 2);

        self.cached_size = Size::new(width, height);
        self.cached_size
    }

    fn draw(&self, ctx: &mut PaintCtx, frame: Rect) {
        // Draw button background
        // TODO: Implement actual background drawing

        // Draw label centered in button
        let padding = 10;
        let label_frame = Rect::new(
            frame.x + padding as i32,
            frame.y + padding as i32,
            frame.width.saturating_sub(padding * 2),
            frame.height.saturating_sub(padding * 2),
        );

        self.label.draw(ctx, label_frame);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &Event) -> ControlFlow {
        if self.handle_mouse_event(event) {
            // Request repaint when state changes
            ctx.request_paint();
        }
        ControlFlow::Continue
    }

    fn update(&mut self, _ctx: &mut UpdateCtx) {
        // Button doesn't need periodic updates
    }
}

impl fmt::Debug for Button {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Button")
            .field("label", &self.label.text())
            .field("is_hovered", &self.is_hovered)
            .field("is_pressed", &self.is_pressed)
            .field("cached_size", &self.cached_size)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::String;

    #[test]
    fn test_button_new() {
        let button = Button::new("Click Me");
        assert_eq!(button.label(), "Click Me");
        assert!(!button.is_hovered());
        assert!(!button.is_pressed());
    }

    #[test]
    fn test_button_set_label() {
        let mut button = Button::new("Click");
        button.set_label("Press Me");
        assert_eq!(button.label(), "Press Me");
    }

    #[test]
    fn test_button_set_colors() {
        let mut button = Button::new("Test");
        button.set_colors(Color::RED, Color::GREEN, Color::BLUE);
        // Colors are set, state depends on interaction
    }
}
