//! Element module - runtime components for ScarletUI

mod id;
mod element;
mod component;
mod render;
mod tree;
mod dirty;
mod vstack;
mod hstack;
mod focus;

pub use id::ElementId;
pub use element::{
    Element, LayoutConstraints, TextInputElementState, UpdateResult, WindowSizeLimits,
};
pub use component::ComponentElement;
pub use render::{RenderElement, RenderObject as ElementRenderObject};
pub use tree::{ElementTree, generate_element_id};
pub use dirty::DirtyFlags;
pub use vstack::VStackElement;
pub use hstack::HStackElement;
pub(crate) use focus::{focused_descendant_path, restore_focus_at_path};
