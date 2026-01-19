//! Scarlet UI - Native UI toolkit for Scarlet OS
//!
//! This library provides a modern, data-first UI framework inspired by
//! Druid, Flutter, and AppKit.
//!
//! # Architecture (New - Under Development)
//!
//! The new ScarletUI architecture follows a data-first MVC pattern:
//!
//! - **Data-first**: All state flows through DataContext<T>
//! - **Unidirectional**: Data flows down, events flow up
//! - **Phase-separated**: Event → Layout → Draw → Compose → Present
//! - **Incremental**: Only redraw what changes (dirty tracking)
//!
//! # Key Components
//!
//! - [`View`]: Core trait for all UI components
//! - [`DataContext`]: State management with automatic invalidation
//! - [`ViewRegistry`]: O(1) view lookup and tracking
//! - [`LayoutConstraints`]: Constraint-based layout system
//!
//! # Example
//!
//! ```no_run
//! use scarlet_ui::{
//!     View, ViewId, LayoutCtx, PaintCtx, EventCtx, UpdateCtx,
//!     LayoutConstraints, Size, ControlFlow, DataContext, ViewRegistry,
//!     graphics::{Canvas, Rect}, event::Event,
//! };
//!
//! struct MyView {
//!     id: ViewId,
//! }
//!
//! impl MyView {
//!     fn new() -> Self {
//!         Self { id: ViewId::new() }
//!     }
//! }
//!
//! impl View for MyView {
//!     fn id(&self) -> ViewId { self.id }
//!
//!     fn layout(&mut self, _ctx: &mut LayoutCtx, _constraints: LayoutConstraints) -> Size {
//!         Size::new(100, 100)
//!     }
//!
//!     fn draw(&self, _ctx: &mut PaintCtx, _frame: Rect) {
//!         // Draw the view
//!     }
//! }
//! ```

#![no_std]

extern crate scarlet_std as std;

// Core modules
pub mod context;
pub mod layout;
pub mod layout_containers;
pub mod application;

// Re-export core types
pub use context::{ControlFlow, EventCtx, LayoutCtx, PaintCtx, UpdateCtx};
pub use layout::{Layout, LayoutConstraints, Size, CrossAxisAlignment, MainAxisAlignment};
pub use layout_containers::{VStack, HStack, ZStack, Spacer};

// Graphics and events (existing - to be integrated)
pub mod color;
pub mod composition;
pub mod design;
pub mod event;
pub mod graphics;
pub mod menu;
pub mod modifiers;
pub mod timer;

// State management (new architecture)
pub mod state;

// View system (new architecture)
pub mod view;

// Re-export state types
pub use state::{DataContext, Local, Observable, ObservableNotifier, StateObject, Observed};

// Re-export view types
pub use view::{
    ViewId,
    ViewRegistry,
    DirtyFlags,
    RenderTracker,
    ViewBuffer,
    BufferPool,
    RepaintBoundary,
    Window,
    WindowKind,
    WindowState,
};
pub use view::node::ViewNode;
pub use view::traits::{View, ViewChild, Container, Opaque, RepaintBoundary as RepaintBoundaryTrait, LayoutBoundary};

// Re-export controls
pub use view::controls::{
    Text, Button, ButtonAction, Image, TextField, Toggle, Slider,
    FontConfig, TextAlignment,
    ImageData, ImageScaling,
    ToggleStyle,
};

// Re-export commonly used types
pub use color::Color;
pub use composition::{Compositor, CompositorLayer, CompositorError};
pub use event::{Event, EventKind, MouseButton};
pub use graphics::{Canvas, Point, Rect};
pub use modifiers::{ViewExt, Padding, Frame, Background};
pub use application::{Application, ApplicationDelegate, ApplicationHandle};

// Proc macro re-exports (existing)
pub use scarlet_ui_macros::{View as DeriveView, state, binding, view, view_builder, view_builder as view_builder_macro};

// Observable macros
pub use scarlet_ui_macros::{observable, published};

// Vector font support (existing)
pub use ab_glyph::{FontRef, InvalidFont};
