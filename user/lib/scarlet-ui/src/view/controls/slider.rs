//! Slider control
//!
//! A horizontal slider for selecting a value within a range.

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

/// Slider view
#[bindable]
pub struct Slider {
    id: ViewId,
    #[bind]
    value: f32,
    minimum: f32,
    maximum: f32,
    step: Option<f32>,
    label: Option<String>,
    track_color: Color,
    fill_color: Color,
    thumb_color: Color,
    is_dragging: bool,
    cached_size: Size,
}

impl Slider {
    /// Create a new slider with a range
    pub fn new(minimum: f32, maximum: f32) -> Self {
        assert!(minimum < maximum, "Minimum must be less than maximum");

        Self {
            value: minimum,
            minimum,
            maximum,
            ..Default::default()
        }
    }

    /// Create a slider with an initial value
    pub fn with_value(minimum: f32, maximum: f32, value: f32) -> Self {
        let mut slider = Self::new(minimum, maximum);
        slider.set_value(value);
        slider
    }

    /// Get the current value
    pub fn get_value(&self) -> f32 {
        self.value
    }

    /// Set the current value
    pub fn set_value(&mut self, mut value: f32) {
        value = value.clamp(self.minimum, self.maximum);

        // Apply step if set
        if let Some(step) = self.step {
            // Manual round implementation using truncation
            let stepped = (value - self.minimum) / step;
            let rounded = (stepped + if stepped >= 0.0 { 0.5 } else { -0.5 }) as i32;
            value = rounded as f32 * step + self.minimum;
        }

        self.value = value;
    }

    /// Set the value (chainable)
    pub fn value(mut self, value: f32) -> Self {
        self.set_value(value);
        self
    }

    /// Get the minimum value
    pub fn minimum(&self) -> f32 {
        self.minimum
    }

    /// Get the maximum value
    pub fn maximum(&self) -> f32 {
        self.maximum
    }

    /// Set the range
    pub fn set_range(&mut self, minimum: f32, maximum: f32) {
        assert!(minimum < maximum, "Minimum must be less than maximum");
        self.minimum = minimum;
        self.maximum = maximum;
        self.set_value(self.value); // Clamp current value
    }

    /// Set the minimum value (chainable)
    pub fn min(mut self, minimum: f32) -> Self {
        assert!(minimum < self.maximum, "Minimum must be less than maximum");
        self.minimum = minimum;
        self.set_value(self.value); // Clamp current value
        self
    }

    /// Set the maximum value (chainable)
    pub fn max(mut self, maximum: f32) -> Self {
        assert!(self.minimum < maximum, "Maximum must be greater than minimum");
        self.maximum = maximum;
        self.set_value(self.value); // Clamp current value
        self
    }

    /// Set the step size
    pub fn set_step(&mut self, step: f32) {
        assert!(step > 0.0, "Step must be positive");
        self.step = Some(step);
    }

    /// Clear the step size
    pub fn clear_step(&mut self) {
        self.step = None;
    }

    /// Get the label
    pub fn label(&self) -> Option<&str> {
        self.label.as_ref().map(|s| s.as_str())
    }

    /// Set the label
    pub fn set_label(&mut self, label: impl Into<String>) {
        self.label = Some(label.into());
    }

    /// Set the colors
    pub fn set_colors(&mut self, track: Color, fill: Color, thumb: Color) {
        self.track_color = track;
        self.fill_color = fill;
        self.thumb_color = thumb;
    }

    /// Calculate value from position
    fn value_from_position(&self, x: i32, width: u32) -> f32 {
        let relative_x = (x as f32).clamp(0.0, width as f32);
        let fraction = relative_x / width as f32;
        let value = self.minimum + fraction * (self.maximum - self.minimum);

        // Apply step if set
        if let Some(step) = self.step {
            let stepped = (value - self.minimum) / step;
            let rounded = (stepped + if stepped >= 0.0 { 0.5 } else { -0.5 }) as i32;
            rounded as f32 * step + self.minimum
        } else {
            value
        }
    }

    /// Calculate thumb position from value
    fn thumb_position(&self, width: u32) -> i32 {
        let fraction = (self.value - self.minimum) / (self.maximum - self.minimum);
        (fraction * width as f32) as i32
    }

    /// Handle mouse event
    fn handle_mouse_event(&mut self, event: &Event) -> bool {
        match &event.kind {
            EventKind::MouseDown { button, .. } => {
                if *button == MouseButton::Left {
                    self.is_dragging = true;
                    let new_value = self.value_from_position(event.position.x, self.cached_size.width);
                    self.set_value(new_value);
                    // Update bound DataContext
                    if let Some(ref data) = self.value_data {
                        data.set(new_value);
                    }
                    return true;
                }
                false
            }
            EventKind::MouseUp { button, .. } => {
                if *button == MouseButton::Left {
                    self.is_dragging = false;
                    return true;
                }
                false
            }
            EventKind::MouseMove => {
                if self.is_dragging {
                    let new_value = self.value_from_position(event.position.x, self.cached_size.width);
                    self.set_value(new_value);
                    // Update bound DataContext
                    if let Some(ref data) = self.value_data {
                        data.set(new_value);
                    }
                    return true;
                }
                false
            }
            _ => false,
        }
    }

    /// Calculate slider dimensions
    fn calculate_size(&self) -> Size {
        // Standard slider height
        const SLIDER_HEIGHT: u32 = 20;
        // Default width
        const SLIDER_WIDTH: u32 = 200;

        Size::new(SLIDER_WIDTH, SLIDER_HEIGHT)
    }
}

impl crate::view::render::RenderObject for Slider {
    fn id(&self) -> ViewId {
        self.id
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn layout(&mut self, _ctx: &mut LayoutCtx, constraints: LayoutConstraints) -> Size {
        let slider_size = self.calculate_size();

        // Add space for label if present
        let mut width = slider_size.width;
        if self.label.is_some() {
            // Add padding and estimated text width
            width += 10 + 100; // 10px padding + estimated text width
        }

        let height = slider_size.height;

        let size = Size::new(
            width.max(constraints.min_width).min(constraints.max_width),
            height.max(constraints.min_height).min(constraints.max_height),
        );

        self.cached_size = size;
        size
    }

    fn draw(&self, ctx: &mut PaintCtx, frame: Rect) {
        // TODO: Implement actual slider rendering
        let _ = (ctx, frame);

        // Draw track
        // Draw filled portion based on value
        // Draw thumb at appropriate position
        // Draw label if present
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &Event) -> ControlFlow {
        if self.handle_mouse_event(event) {
            ctx.request_paint();
        }
        ControlFlow::Continue
    }

    fn update(&mut self, _ctx: &mut UpdateCtx) {
        // Slider doesn't need periodic updates
    }
}

impl fmt::Debug for Slider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Slider")
            .field("value", &self.value)
            .field("minimum", &self.minimum)
            .field("maximum", &self.maximum)
            .field("step", &self.step)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slider_new() {
        let slider = Slider::new(0.0, 100.0);
        assert_eq!(slider.value(), 0.0);
        assert_eq!(slider.minimum(), 0.0);
        assert_eq!(slider.maximum(), 100.0);
    }

    #[test]
    fn test_slider_with_value() {
        let slider = Slider::with_value(0.0, 100.0, 50.0);
        assert_eq!(slider.value(), 50.0);
    }

    #[test]
    fn test_slider_set_value() {
        let mut slider = Slider::new(0.0, 100.0);
        slider.set_value(75.0);
        assert_eq!(slider.value(), 75.0);
    }

    #[test]
    fn test_slider_clamp() {
        let mut slider = Slider::new(0.0, 100.0);
        slider.set_value(150.0);
        assert_eq!(slider.value(), 100.0);

        slider.set_value(-50.0);
        assert_eq!(slider.value(), 0.0);
    }

    #[test]
    fn test_slider_step() {
        let mut slider = Slider::new(0.0, 100.0);
        slider.set_step(10.0);

        slider.set_value(37.0);
        assert_eq!(slider.value(), 40.0);

        slider.set_value(34.9);
        assert_eq!(slider.value(), 30.0);
    }

    #[test]
    fn test_slider_set_range() {
        let mut slider = Slider::new(0.0, 100.0);
        slider.set_value(50.0);

        slider.set_range(0.0, 10.0);
        assert_eq!(slider.value(), 10.0); // Clamped to new maximum
        assert_eq!(slider.maximum(), 10.0);
    }

    #[test]
    fn test_slider_with_label() {
        let slider = Slider::with_label(0.0, 100.0, "Volume");
        assert_eq!(slider.label(), Some("Volume"));
    }
}
