//! Views module - Built-in View implementations
//!
//! This module provides common View implementations like Window, Text, Button, etc.

mod button;
mod canvas;
pub mod containers;
mod divider;
mod either;
mod image;
pub mod menu;
pub mod modifiers;
pub mod navigation;
mod progress;
mod rectangle;
mod select;
mod slider;
mod spacer;
mod text;
pub(crate) mod text_field;
mod text_grid;
mod toggle;
mod window;

pub use button::{Button, ButtonCallback, ButtonRenderObject};
pub use canvas::{CanvasEventHandler, CanvasRenderCallback, CanvasRenderObject, CanvasView};
pub use divider::{Divider, DividerOrientation, DividerRenderObject};
pub use either::{Either, Either3, Either4, Either5, Either6};
pub use image::{BitmapImage, Image, ImageFit, ImageRenderObject, ImageSource};
pub use menu::{Menu, MenuAction, MenuBar, MenuItem, MenuItemContent};
pub use navigation::{NavigationLink, NavigationView};
pub use progress::{ProgressView, ProgressViewRenderObject};
pub use rectangle::{Rectangle, RectangleRenderObject};
pub use select::{Select, SelectChangeCallback, SelectRenderObject};
pub use slider::{Slider, SliderRenderObject};
pub use spacer::{Spacer, SpacerRenderObject};
pub use text::{Text, TextRenderObject};
pub use text_field::{TextField, TextFieldRenderObject};
pub use text_grid::{
    TextGrid, TextGridBuffer, TextGridCell, TextGridCursor, TextGridRenderObject,
    text_grid_cell_width,
};
pub use toggle::{Toggle, ToggleRenderObject};
pub use window::window_type;
pub use window::{
    Window, WindowContentLayout, WindowInfo, WindowRenderElement, WindowRenderObject,
};

// Re-export modifiers for convenience
pub use modifiers::{
    AlignmentFrame, AlignmentRenderObject, Background, BackgroundRenderObject, Focusable,
    FocusableRenderObject, Frame, FrameRenderObject, OnKey, OnKeyRenderObject, Padding,
    PaddingRenderObject, SetSize, SizeRenderObject,
};

// Re-export containers for convenience
pub use containers::{HStack, VStack, ZStack, ZStackRenderObject};
