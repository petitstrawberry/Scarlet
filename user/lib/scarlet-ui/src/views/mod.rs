mod button;
mod frame;
mod image;
mod rectangle;
mod slider;
pub mod spacer;  // Public marker type
mod text;
mod text_field;
mod toggle;
mod window;  // Re-export from containers

use crate::traits::View;

pub use button::{Button, ButtonColors};
pub use frame::{Frame, Padding};
pub use image::Image;
pub use rectangle::Rectangle;
pub use slider::Slider;
pub use spacer::Spacer;
pub use text::Text;
pub use text_field::TextField;
pub use toggle::Toggle;
pub use window::Window;  // Re-export from containers

/// Convenience trait for view modifiers
pub trait ViewExt: View + Clone {
    fn frame(self) -> Frame<Self>
    where
        Self: Sized,
    {
        Frame::new(self)
    }

    fn frame_max(self) -> Frame<Self>
    where
        Self: Sized,
    {
        Frame::new(self).fill_parent()
    }

    fn padding(self) -> Padding<Self>
    where
        Self: Sized,
    {
        Padding::new(self).all(16.0)
    }
}

// Implement ViewExt for all Views
impl<V: View + Clone> ViewExt for V {}
