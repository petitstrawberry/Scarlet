//! Views module - Built-in View implementations
//!
//! This module provides common View implementations like Window, Text, Button, etc.

mod window;
mod text;
mod button;
mod rectangle;
mod spacer;
mod image;
mod divider;
mod toggle;
mod slider;
mod progress;
pub mod modifiers;
pub mod containers;

pub use window::{Window, WindowRenderObject, WindowRenderElement};
pub use text::{Text, TextRenderObject};
pub use button::{Button, ButtonRenderObject, ButtonCallback};
pub use rectangle::{Rectangle, RectangleRenderObject};
pub use spacer::{Spacer, SpacerRenderObject};
pub use image::{Image, ImageRenderObject, ImageSource, ImageFit};
pub use divider::{Divider, DividerRenderObject, DividerOrientation};
pub use toggle::{Toggle, ToggleRenderObject};
pub use slider::{Slider, SliderRenderObject};
pub use progress::{ProgressView, ProgressViewRenderObject};

// Re-export modifiers for convenience
pub use modifiers::{
    Padding, PaddingRenderObject,
    Frame, FrameRenderObject,
    Background, BackgroundRenderObject,
    SetSize, SizeRenderObject,
    AlignmentFrame, AlignmentRenderObject,
};

// Re-export containers for convenience
pub use containers::{
    VStack,
    HStack,
    ZStack, ZStackRenderObject,
};
