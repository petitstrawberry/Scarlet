//! View system - Core UI components
//!
//! This module provides the view system, which is the foundation of ScarletUI.
//! Views are the building blocks of UI - they form a tree structure and
//! participate in layout, drawing, and event handling.

pub mod id;
pub mod dirty;
pub mod registry;
pub mod tracker;
pub mod buffer;
pub mod pool;
pub mod boundary;
pub mod opaque;
pub mod controls;

pub mod render;
pub mod traits;
pub mod node;
pub mod window;

// Re-exports
pub use id::ViewId;
pub use dirty::DirtyFlags;
pub use registry::{ViewRegistry, ViewEntry};
pub use tracker::RenderTracker;
pub use buffer::ViewBuffer;
pub use pool::BufferPool;
pub use boundary::RepaintBoundary;
pub use render::RenderObject;
pub use traits::View;
pub use window::{Window, WindowBuilder, WindowKind, WindowState};

// Re-export layout containers (for convenience)
pub use crate::layout_containers::Spacer;
