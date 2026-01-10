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
//! # Features
//!
//! ## Core View System
//!
//! - Declarative view composition
//! - Automatic layout management
//! - Two-phase event handling (capture and bubble)
//! - Window decorations with modern design
//!
//! ## Layout Containers
//!
//! Build complex layouts using:
//! - [`VStack`] - Vertical stack
//! - [`HStack`] - Horizontal stack
//! - [`ZStack`] - Overlay stack
//! - [`Padding`] - Add padding
//! - [`Center`] - Center content
//! - [`Spacer`] - Flexible spacing
//!
//! ## Control Widgets
//!
//! - [`Label`] - Text display
//! - [`Button`] - Interactive button
//! - [`TextField`] - Text input
//! - [`CheckBox`] - Boolean toggle
//! - [`Slider`] - Value selection
//! - [`ProgressBar`] - Progress indicator
//! - [`Toggle`] - Switch control
//!
//! ## View Modifiers
//!
//! Apply styles using the [`ViewModifier`] trait:
//! - `corner_radius()` - Rounded corners
//! - `border()` - Border styling
//! - `background_color()` - Background color
//!
//! # Example
//!
//! ```no_run
//! use scarlet_ui::{
//!     Application, Window, VStack, HStack, Label, Button, 
//!     CheckBox, Slider, Padding, Color, ViewModifier,
//! };
//!
//! fn main() {
//!     let mut app = Application::new().expect("Failed to connect");
//!
//!     // Scarlet UI loads its default vector font from rootfs.
//!     // Make sure `/system/scarlet/fonts/Mplus1-Regular.ttf` exists in the image.
//!     
//!     let window = Window::new("Demo", 600, 400)
//!         .background(Color::rgb(245, 245, 250))
//!         .content(
//!             Padding::new(
//!                 VStack::new()
//!                     .spacing(16)
//!                     .child(
//!                         Label::new("Welcome to ScarletUI!")
//!                             .color(Color::TEXT)
//!                             .font_size(32)
//!                     )
//!                     .child(
//!                         CheckBox::new("Enable feature", true)
//!                             .on_toggle(|checked| {
//!                                 println!("Feature enabled: {}", checked);
//!                             })
//!                     )
//!                     .child(
//!                         Slider::new(0.5, 0.0, 1.0)
//!                             .on_change(|value| {
//!                                 println!("Value: {:.2}", value);
//!                             })
//!                     )
//!                     .child(HStack::new()
//!                         .spacing(12)
//!                         .child(Button::new("OK", || {
//!                             println!("OK clicked");
//!                         }))
//!                         .child(Button::new("Cancel", || {
//!                             println!("Cancel clicked");
//!                         }))
//!                     )
//!             ).all(20)
//!         );
//!     
//!     app.add_window(window).unwrap();
//!     app.run(); // Framework takes over - never returns
//! }
//! ```
//!
//! # Design Philosophy
//!
//! Scarlet UI follows SwiftUI-inspired design principles:
//!
//! 1. **Declarative** - Describe what you want, not how to build it
//! 2. **Composable** - Build complex UIs from simple components
//! 3. **Type-safe** - Leverage Rust's type system
//! 4. **Efficient** - Minimal allocations, `no_std` compatible

#![no_std]

extern crate scarlet_std as std;

mod application;
pub mod color;
pub mod event;
pub mod graphics;
pub mod modifiers;
pub mod state;
pub mod timer;
pub mod view;

// Re-exports
pub use application::{Application, ApplicationDelegate, ApplicationHandle};
pub use color::Color;
pub use event::{Event, EventKind, EventType, MouseButton};
pub use graphics::{set_default_font, Canvas, Point, Rect};
pub use state::{State, Binding, Observable};
pub use timer::{Timer, schedule_on_main_thread};

// Proc macro re-exports
pub use scarlet_ui_macros::{View as DeriveView, state, binding, view_builder};

// Vector font support (always enabled)
pub use ab_glyph::{FontRef, InvalidFont};

// View system re-exports
pub use view::{
    View, Size, ViewBox, IntoViewBox,
    Window,
    VStack, HStack, ZStack, Padding, Center,
    Label, Button, Spacer, RectView,
    TextField, CheckBox, Slider, ProgressBar, Toggle,
};

// Modifier re-exports
pub use modifiers::{ViewModifier, CornerRadius, Border, Background};
