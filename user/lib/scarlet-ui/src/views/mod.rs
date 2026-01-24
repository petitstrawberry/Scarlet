//! Views module - Built-in View implementations
//!
//! This module provides common View implementations like Window, Text, Button, etc.

mod window;
mod text;
mod button;
mod rectangle;
mod spacer;
mod image;
pub mod modifiers;

pub use window::Window;
pub use text::{Text, TextRenderObject};
pub use button::{Button, ButtonRenderObject, ButtonCallback};
pub use rectangle::{Rectangle, RectangleRenderObject};
pub use spacer::{Spacer, SpacerRenderObject};
pub use image::{Image, ImageRenderObject, ImageSource, ImageFit};

// Re-export modifiers for convenience
pub use modifiers::{
    Padding, PaddingRenderObject,
    Frame, FrameRenderObject,
    Background, BackgroundRenderObject,
    SetSize, SizeRenderObject,
    AlignmentFrame, AlignmentRenderObject,
};
