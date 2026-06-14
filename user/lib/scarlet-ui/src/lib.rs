//! ScarletUI 2.0 - Declarative UI Framework for Scarlet OS
//!
//! ScarletUI is a reactive UI framework inspired by Flutter and SwiftUI.
//! It provides:
//!
//! - **Declarative Views**: Describe your UI with composable Views
//! - **State Management**: Reactive State<T> with automatic updates
//! - **Element System**: Efficient runtime representation with reconciliation
//! - **Layout Engine**: Constraint-based layout system
//! - **Event Handling**: Pointer and keyboard event routing
//! - **Platform Abstraction**: Work with SWS, SDL2, Winit, or custom backends
//!
//! # Basic Usage
//!
//! ```no_run
//! use scarlet_ui::*;
//!
//! #[derive(View, Clone)]
//! struct CounterApp {
//!     #[state]
//!     count: State<i32>,
//! }
//!
//! impl CounterApp {
//!     fn body(&self) -> impl View {
//!         vstack! {
//!             Text::new("Counter"),
//!             Button::new("Increment")
//!                 .on_click(|_| self.count.update(|c| *c += 1)),
//!             Text::new(&format!("Count: {}", self.count.get())),
//!         }
//!     }
//! }
//!
//! fn main() {
//!     let mut app = CounterApp {
//!         count: State::initial(0),
//!     };
//!     app.run();
//! }
//! ```

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;
#[cfg(not(feature = "std"))]
extern crate scarlet_std as std;

// Import procedural macros crate (derives work automatically)
extern crate scarlet_ui_macros;

pub mod application;
pub mod buffer;
pub mod color;
pub mod compositor;
pub mod debug;
pub mod element;
pub mod error;
pub mod event;
pub mod geometry;
pub mod graphics;
pub mod macros;
pub mod menu_model;
mod os;
pub mod pipeline;
pub mod platform;
pub mod render;
pub mod state;
pub mod view;
pub mod views;

#[doc(hidden)]
pub mod __private {
    pub use alloc::boxed::Box;
    pub use alloc::vec::Vec;
}

#[cfg(feature = "std")]
macro_rules! logln {
    ($($arg:tt)*) => {
        std::println!($($arg)*)
    };
}

#[cfg(not(feature = "std"))]
macro_rules! logln {
    ($($arg:tt)*) => {
        scarlet_std::println!($($arg)*)
    };
}

pub(crate) use logln;

// Re-exports for convenience
pub use application::Application;
pub use buffer::Buffer;
pub use color::system::{
    BlueColors, GrayColors, GreenColors, OrangeColors, PinkColors, PurpleColors, RedColors,
    YellowColors,
};
pub use color::{Color, ColorPalette, ColorScheme, SemanticColor, SystemColors};
pub use compositor::Compositor;
pub use element::{
    ComponentElement, DirtyFlags, Element, ElementId, ElementRenderObject, ElementTree,
    LayoutConstraints, RenderElement,
};
pub use error::{Error, Result};
pub use event::{
    Event, EventDispatcher, FocusEvent, InputEvent, KeyCode, KeyEvent, LifecycleEvent, MouseButton,
    MouseEvent,
};
pub use geometry::{Alignment, EdgeInsets, Offset, Point, Rect, Size};
pub use graphics::{
    Canvas, FontStack, add_default_font_fallback, clear_default_font_fallbacks, default_font_stack,
    measure_text_sized, measure_text_sized_with_font_stack, set_default_font,
    set_default_font_stack,
};
pub use menu_model::{MenuBarModel, MenuEntry, MenuItemModel};
pub use pipeline::{DirtyPhase, PipelineOwner, RenderingPipeline, StateRegistry};
pub use platform::{PlatformWindow, SWSPlatformWindow};
pub use render::{RenderNode, RenderTree};
pub use state::{InvalidationKind, Listenable, State, StateId, SubscriptionId, generate_state_id};
pub use view::{View, ViewExt};
pub use views::modifiers::{
    AlignmentFrame, Background, Clip, Focusable, Frame, OnKey, Padding, SetSize,
};
pub use views::navigation::Icon;
pub use views::{
    BitmapImage, CanvasView, Divider, DividerOrientation, ProgressView, Select, Slider, Toggle,
};
pub use views::{
    Button, HStack, Image, Rectangle, Spacer, Text, TextField, VStack, Window, WindowContentLayout,
    ZStack,
};
pub use views::{NavigationLink, NavigationView};
pub use views::{TextGrid, TextGridBuffer, TextGridCell, TextGridCursor, text_grid_cell_width};

// Macros are exported at root via #[macro_export]
// Users can use them directly: vstack! {}, hstack! {}, etc.

/// Prelude module for convenient imports
pub mod prelude {
    pub use crate::application::Application;
    pub use crate::color::{Color, ColorPalette, ColorScheme, SemanticColor};
    pub use crate::element::{
        DirtyFlags, Element, ElementId, ElementRenderObject, LayoutConstraints,
    };
    pub use crate::error::{Error, Result};
    pub use crate::event::{Event, FocusEvent, KeyEvent, LifecycleEvent, MouseEvent};
    pub use crate::geometry::*;
    pub use crate::graphics::{
        FontStack, add_default_font_fallback, clear_default_font_fallbacks, default_font_stack,
        measure_text_sized_with_font_stack, set_default_font_stack,
    };
    pub use crate::menu_model::{MenuBarModel, MenuEntry, MenuItemModel};
    pub use crate::state::{InvalidationKind, Listenable, State, StateId, SubscriptionId};
    pub use crate::view::{View, ViewExt};
    pub use crate::views::modifiers::{Background, Clip, Focusable, Frame, OnKey, Padding};
    pub use crate::views::{
        BitmapImage, CanvasView, Divider, DividerOrientation, ProgressView, Select, Slider, Toggle,
    };
    pub use crate::views::{
        Button, HStack, Image, Rectangle, Spacer, Text, TextField, VStack, Window,
        WindowContentLayout, ZStack,
    };
    pub use crate::views::{Menu, MenuAction, MenuBar, MenuItem, MenuItemContent};
    pub use crate::views::{
        TextGrid, TextGridBuffer, TextGridCell, TextGridCursor, text_grid_cell_width,
    };

    // Note: The View derive macro must be imported from scarlet_ui_macros:
    // use scarlet_ui_macros::View;
    //
    // Declarative macros (vstack!, hstack!, zstack!) can be imported as:
    // use scarlet_ui::{vstack, hstack, zstack};
}
