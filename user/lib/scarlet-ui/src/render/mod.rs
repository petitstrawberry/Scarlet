//! Render module - RenderObject trait and rendering system
//!
//! This module provides the core rendering abstraction for ScarletUI.

mod object;
mod tree;

pub use object::RenderObject;
pub use tree::{RenderNode, RenderTree};
