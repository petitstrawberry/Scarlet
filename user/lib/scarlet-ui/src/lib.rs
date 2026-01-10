//! Scarlet UI - Native UI toolkit for Scarlet OS
//!
//! This library provides widgets and utilities for building graphical applications
//! on Scarlet OS using the Scarlet Window Server (SWS).
//!
//! # Architecture
//!
//! - `sws-client`: Low-level connection and protocol handling
//! - `scarlet-ui` (this crate): High-level UI toolkit with View-based components
//!
//! # Example
//!
//! ```no_run
//! use scarlet_ui::{Application, Window, VStack, Label, Button, Padding, Color};
//!
//! fn main() {
//!     let mut app = Application::new().expect("Failed to connect");
//!     
//!     let window = Window::new("Demo", 400, 300)
//!         .background(Color::DARK_GRAY)
//!         .content(
//!             Padding::new(
//!                 VStack::new()
//!                     .child(Label::new("Hello, Scarlet!"))
//!                     .child(Button::new("Click Me", || {
//!                         // Handle click
//!                     }))
//!             ).all(20)
//!         );
//!     
//!     app.add_window(window).unwrap();
//!     app.run(); // Framework takes over - never returns
//! }
//! ```

#![no_std]

extern crate scarlet_std as std;

mod application;
pub mod color;
pub mod event;
pub mod graphics;
pub mod view;

// Legacy modules (for compatibility)
pub mod widgets;
pub mod window;

// Re-exports
pub use application::Application;
pub use color::Color;
pub use event::{Event, EventKind, EventType, MouseButton};
pub use graphics::{Canvas, Point, Rect};

// View system re-exports
pub use view::{
    View, Size, ViewBox, IntoViewBox,
    Window,
    VStack, HStack, ZStack, Padding, Center,
    Label, Button, Spacer, RectView,
};

// Legacy widget re-export
pub use widgets::Widget;

