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

// Re-exports for convenience
pub use geometry::{Size, Point, Rect, Offset, EdgeInsets, Alignment};
pub use color::Color;
pub use error::{Error, Result};
pub use state::{State, StateId, generate_state_id, Listenable};
pub use view::View;
pub use element::{Element, ElementId, LayoutConstraints, ComponentElement, RenderElement, ElementTree, StateRegistry};
pub use event::{Event, MouseEvent, KeyEvent, InputEvent, KeyCode, MouseButton};
pub use buffer::Buffer;
pub use compositor::Compositor;
pub use render::RenderObject;
pub use pipeline::{PipelineOwner, RenderingPipeline, DirtyPhase};

/// Prelude module for convenient imports
pub mod prelude {
    pub use crate::geometry::*;
    pub use crate::color::Color;
    pub use crate::error::{Error, Result};
    pub use crate::state::{State, StateId, Listenable};
    pub use crate::view::View;
    pub use crate::element::{Element, ElementId, LayoutConstraints};
    pub use crate::event::{Event, MouseEvent, KeyEvent};
}
