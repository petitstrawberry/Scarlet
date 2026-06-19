//! ScarletUI core implementation.
//!
//! This crate contains the platform-independent UI implementation used by the
//! `scarlet-ui` facade crate. It provides:
//!
//! - **Declarative Views**: Describe your UI with composable Views
//! - **State Management**: Reactive State<T> with automatic updates
//! - **Element System**: Efficient runtime representation with reconciliation
//! - **Layout Engine**: Constraint-based layout system
//! - **Event Handling**: Pointer and keyboard event routing
//! - **Scene Runtime**: Declare top-level windows through `Application::scenes()`
//! - **Platform Abstraction**: Backend traits implemented by platform crates
//!
//! Normal applications should depend on `scarlet-ui`, not this crate directly.
//! The facade re-exports the app-facing API and provides `ApplicationRunExt`.
//!
//! # Backend Integration
//!
//! ```ignore
//! scarlet_ui_core::ApplicationRunner::new(Box::new(my_backend)).run(&mut app)
//! ```
//!
//! # Platform Features
//!
//! Platform implementations live in separate crates and implement the platform
//! traits exposed by this core crate.

#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(all(feature = "std", feature = "legacy-scarlet-std"))]
compile_error!("scarlet-ui features `std` and `legacy-scarlet-std` are mutually exclusive");

#[cfg(not(any(feature = "std", feature = "legacy-scarlet-std")))]
compile_error!("scarlet-ui requires either the `std` or `legacy-scarlet-std` feature");

extern crate alloc;
#[cfg(not(feature = "std"))]
extern crate scarlet_std as std;

// Import procedural macros crate (derives work automatically)
extern crate scarlet_ui_macros;

pub mod application;
pub mod buffer;
pub mod color;
pub mod command;
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
#[cfg(feature = "preview")]
pub mod preview;
pub mod render;
pub mod scene;
pub mod state;
pub mod view;
pub mod views;

#[doc(hidden)]
pub mod __private {
    pub use alloc::boxed::Box;
    pub use alloc::vec::Vec;
    #[cfg(feature = "preview")]
    pub use inventory;
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
pub use application::{Application, ApplicationRunner};
pub use buffer::Buffer;
pub use color::system::{
    BlueColors, GrayColors, GreenColors, OrangeColors, PinkColors, PurpleColors, RedColors,
    YellowColors,
};
pub use color::{Color, ColorPalette, ColorScheme, SemanticColor, SystemColors};
pub use command::{dismiss_window, open_window};
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
pub use pipeline::{
    DirtyPhase, MountContext, PipelineId, PipelineOwner, RenderingPipeline, StateRegistry,
};
pub use platform::{PlatformBackend, PlatformWindow, WindowCreateRequest};
pub use render::{RenderNode, RenderTree};
pub use scene::{Scene, SceneBuilder, SceneWindowKey, WindowContext, WindowGroup, WindowId};
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

pub use scarlet_ui_macros::preview;

// Macros are exported at root via #[macro_export]
// Users can use them directly: vstack! {}, hstack! {}, scenes! {}, etc.

/// Prelude module for convenient imports
pub mod prelude {
    pub use crate::application::{Application, ApplicationRunner};
    pub use crate::color::{Color, ColorPalette, ColorScheme, SemanticColor};
    pub use crate::command::{dismiss_window, open_window};
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
    pub use crate::scene::{
        Scene, SceneBuilder, SceneWindowKey, WindowContext, WindowGroup, WindowId,
    };
    pub use crate::state::{InvalidationKind, Listenable, State, StateId, SubscriptionId};
    pub use crate::view::{View, ViewExt};
    pub use crate::views::modifiers::{Background, Clip, Focusable, Frame, OnKey, Padding};
    pub use crate::views::{
        BitmapImage, CanvasView, Divider, DividerOrientation, ProgressView, Select, Slider, Toggle,
    };
    pub use crate::views::{
        Button, Either, Either3, Either4, Either5, Either6, HStack, Image, Rectangle, Spacer, Text,
        TextField, VStack, Window, WindowContentLayout, ZStack,
    };
    pub use crate::views::{Menu, MenuAction, MenuBar, MenuItem, MenuItemContent};
    pub use crate::views::{
        TextGrid, TextGridBuffer, TextGridCell, TextGridCursor, text_grid_cell_width,
    };

    // Note: The View derive macro must be imported from scarlet_ui_macros:
    // use scarlet_ui_macros::View;
    //
    // Declarative macros (vstack!, hstack!, zstack!, scenes!) can be imported as:
    // use scarlet_ui::{vstack, hstack, zstack, scenes};
}
