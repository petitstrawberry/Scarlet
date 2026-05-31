//! Views module - Built-in View implementations
//!
//! This module provides common View implementations like Window, Text, Button, etc.

mod window;
mod text;
mod button;
pub(crate) mod text_field;
mod rectangle;
mod spacer;
mod image;
mod divider;
mod toggle;
mod slider;
mod progress;
mod canvas;
mod text_grid;
pub mod modifiers;
pub mod containers;
pub mod menu;
pub mod navigation;

pub use window::{Window, WindowRenderObject, WindowRenderElement, WindowInfo};
pub use window::window_type;
pub use text::{Text, TextRenderObject};
pub use button::{Button, ButtonRenderObject, ButtonCallback};
pub use text_field::{TextField, TextFieldRenderObject};
pub use rectangle::{Rectangle, RectangleRenderObject};
pub use spacer::{Spacer, SpacerRenderObject};
pub use image::{BitmapImage, Image, ImageFit, ImageRenderObject, ImageSource};
pub use divider::{Divider, DividerRenderObject, DividerOrientation};
pub use toggle::{Toggle, ToggleRenderObject};
pub use slider::{Slider, SliderRenderObject};
pub use progress::{ProgressView, ProgressViewRenderObject};
pub use canvas::{CanvasView, CanvasRenderObject, CanvasRenderCallback, CanvasEventHandler};
pub use text_grid::{
    TextGrid, TextGridBuffer, TextGridCell, TextGridCursor, TextGridRenderObject,
};
pub use menu::{MenuItem, MenuBar, Menu, MenuItemContent, MenuAction};
pub use navigation::{NavigationView, NavigationLink};

// Re-export modifiers for convenience
pub use modifiers::{
    Padding, PaddingRenderObject,
    Frame, FrameRenderObject,
    Background, BackgroundRenderObject,
    SetSize, SizeRenderObject,
    AlignmentFrame, AlignmentRenderObject,
    Focusable, FocusableRenderObject,
    OnKey, OnKeyRenderObject,
};

// Re-export containers for convenience
pub use containers::{
    VStack,
    HStack,
    ZStack, ZStackRenderObject,
};
