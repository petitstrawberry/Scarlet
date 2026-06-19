//! NavigationView - Sidebar navigation with dynamic content switching
//!
//! This module provides a SwiftUI-style NavigationView component that provides
//! sidebar navigation with dynamic content switching based on user selection.

mod link;
mod render;
mod tuple;
mod view;
mod view_impl;

pub use link::{Icon, NavigationLink};
pub use render::NavigationViewRenderObject;
pub use view::NavigationView;
