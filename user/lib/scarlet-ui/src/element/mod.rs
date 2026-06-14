//! Element module - runtime components for ScarletUI

mod component;
mod dirty;
mod element;
mod focus;
mod hstack;
mod id;
mod render;
mod tree;
mod vstack;

pub use component::ComponentElement;
pub use dirty::DirtyFlags;
pub use element::{
    Element, LayoutConstraints, TextInputElementState, UpdateResult, WindowSizeLimits,
};
pub(crate) use focus::{focused_descendant_path, restore_focus_at_path};
pub use hstack::HStackElement;
pub use id::ElementId;
pub use render::{RenderElement, RenderObject as ElementRenderObject};
pub use tree::{ElementTree, generate_element_id};
pub use vstack::VStackElement;
