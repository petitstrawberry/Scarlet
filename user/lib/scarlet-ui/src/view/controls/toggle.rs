//! Toggle control
//!
//! A switch/checkbox control for boolean values.

extern crate alloc;
use alloc::string::String;
use alloc::sync::Arc;

use crate::context::{ControlFlow, EventCtx, LayoutCtx, PaintCtx, UpdateCtx};
use crate::event::{Event, EventKind, MouseButton};
use crate::graphics::Rect;
use crate::layout::{LayoutConstraints, Size};
use crate::view::id::ViewId;
use crate::view::traits::View;
use crate::color::Color;
use crate::state::DataContext;
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
pub struct Toggle {
    id: ViewId,
    is_on: bool,
    data: Option<Arc<DataContext<bool>>>,
    label: Option<Arc<String>>,
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
            id: ViewId::new(),
            is_on,
            data: None,
            label: None,
            style: ToggleStyle::default(),
            on_color: Color::SUCCESS,
            off_color: Color::GRAY,
            is_hovered: false,
            is_pressed: false,
            cached_size: Size::ZERO,
        }
    }

    /// Create a toggle with a label
    pub fn with_label(is_on: bool, label: impl Into<Arc<String>>) -> Self {
        let mut toggle = Self::new(is_on);
        toggle.label = Some(label.into());
        toggle
    }

    /// Bind this toggle to a DataContext<bool>
    ///
    /// The toggle will read its initial state from the data context,
    /// and update the data context when toggled.
    pub fn bind(mut self, data: &Arc<DataContext<bool>>) -> Self {
        self.is_on = data.get();
        self.data = Some(Arc::clone(data));
        self
    }

    /// Get the toggle state
    pub fn is_on(&self) -> bool {
        self.is_on
    }

    /// Set the toggle state
    pub fn set_on(&mut self, is_on: bool) {
        self.is_on = is_on;
        // Update the bound data context if present
        if let Some(ref data) = self.data {
            data.set(is_on);
        }
    }

    /// Toggle the state
    pub fn toggle(&mut self) {
        self.is_on = !self.is_on;
        // Update the bound data context if present
        if let Some(ref data) = self.data {
            data.set(self.is_on);
        }
    }

    /// Get the label
    pub fn label(&self) -> Option<&str> {
        self.label.as_ref().map(|s| s.as_str())
    }

    /// Set the label
    pub fn set_label(&mut self, label: impl Into<Arc<String>>) {
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

impl View for Toggle {
    fn id(&self) -> ViewId {
        self.id
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
        // TODO: Implement actual toggle rendering
        let _ = (ctx, frame);

        // Draw switch or checkbox based on style
        // Draw label if present
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
