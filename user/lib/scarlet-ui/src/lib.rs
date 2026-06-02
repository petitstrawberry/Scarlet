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

#![no_std]

extern crate alloc;
extern crate scarlet_std as std;

// Import procedural macros crate (derives work automatically)
extern crate scarlet_ui_macros;

pub mod geometry;
pub mod color;
pub mod error;
pub mod state;
pub mod view;
pub mod element;
pub mod event;
pub mod buffer;
pub mod compositor;
pub mod render;
pub mod pipeline;
pub mod views;
pub mod platform;
pub mod application;
pub mod macros;
pub mod graphics;
pub mod debug;
pub mod menu_model;

// Re-exports for convenience
pub use geometry::{Size, Point, Rect, Offset, EdgeInsets, Alignment};
pub use color::{Color, ColorScheme, ColorPalette, SemanticColor, SystemColors};
pub use color::system::{GrayColors, BlueColors, GreenColors, OrangeColors, PinkColors, PurpleColors, RedColors, YellowColors};
pub use error::{Error, Result};
pub use state::{generate_state_id, InvalidationKind, Listenable, State, StateId, SubscriptionId};
pub use view::{View, ViewExt};
pub use element::{Element, ElementId, LayoutConstraints, ComponentElement, RenderElement, ElementTree, ElementRenderObject, DirtyFlags};
pub use event::{Event, MouseEvent, KeyEvent, InputEvent, KeyCode, MouseButton, EventDispatcher, FocusEvent, LifecycleEvent};
pub use buffer::Buffer;
pub use compositor::Compositor;
pub use render::{RenderTree, RenderNode};
pub use pipeline::{PipelineOwner, RenderingPipeline, DirtyPhase, StateRegistry};
pub use views::{Window, Text, TextField, Button, Rectangle, Spacer, Image, VStack, HStack, ZStack};
pub use views::{BitmapImage, Divider, DividerOrientation, Toggle, Slider, ProgressView, CanvasView};
pub use views::{NavigationView, NavigationLink};
pub use views::{TextGrid, TextGridBuffer, TextGridCell, TextGridCursor, text_grid_cell_width};
pub use views::navigation::Icon;
pub use views::modifiers::{
    AlignmentFrame, Background, Clip, Focusable, Frame, OnKey, Padding, SetSize,
};
pub use platform::{PlatformWindow, SWSPlatformWindow};
pub use application::Application;
pub use graphics::{
    Canvas, FontStack, add_default_font_fallback, clear_default_font_fallbacks,
    default_font_stack, measure_text_sized, measure_text_sized_with_font_stack, set_default_font,
    set_default_font_stack,
};
pub use menu_model::{MenuBarModel, MenuEntry, MenuItemModel};

// Macros are exported at root via #[macro_export]
// Users can use them directly: vstack! {}, hstack! {}, etc.

/// Prelude module for convenient imports
pub mod prelude {
    pub use crate::geometry::*;
    pub use crate::color::{Color, ColorScheme, ColorPalette, SemanticColor};
    pub use crate::error::{Error, Result};
    pub use crate::state::{InvalidationKind, Listenable, State, StateId, SubscriptionId};
    pub use crate::view::{View, ViewExt};
    pub use crate::element::{Element, ElementId, LayoutConstraints, ElementRenderObject, DirtyFlags};
    pub use crate::event::{Event, MouseEvent, KeyEvent, FocusEvent, LifecycleEvent};
    pub use crate::application::Application;
    pub use crate::views::{Window, Text, TextField, Button, Rectangle, Spacer, Image, VStack, HStack, ZStack};
    pub use crate::views::{BitmapImage, Divider, DividerOrientation, Toggle, Slider, ProgressView, CanvasView};
    pub use crate::views::{TextGrid, TextGridBuffer, TextGridCell, TextGridCursor, text_grid_cell_width};
    pub use crate::views::{MenuItem, MenuBar, Menu, MenuItemContent, MenuAction};
    pub use crate::views::modifiers::{Background, Clip, Focusable, Frame, OnKey, Padding};
    pub use crate::menu_model::{MenuBarModel, MenuEntry, MenuItemModel};
    pub use crate::graphics::{
        FontStack, add_default_font_fallback, clear_default_font_fallbacks, default_font_stack,
        measure_text_sized_with_font_stack, set_default_font_stack,
    };

    // Note: The View derive macro must be imported from scarlet_ui_macros:
    // use scarlet_ui_macros::View;
    //
    // Declarative macros (vstack!, hstack!, zstack!) can be imported as:
    // use scarlet_ui::{vstack, hstack, zstack};
}
