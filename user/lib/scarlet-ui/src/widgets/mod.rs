//! UI Widgets

pub mod button;
pub mod label;

pub use button::Button;
pub use label::Label;

use crate::{Canvas, Point};

/// Base widget trait
pub trait Widget {
    /// Draw the widget
    fn draw(&self, canvas: &mut Canvas);

    /// Handle mouse click
    fn on_click(&mut self, _point: Point) -> bool {
        false
    }

    /// Check if point is within widget bounds
    fn contains(&self, point: Point) -> bool;
}
