//! Element module - runtime components for ScarletUI

mod id;
mod element;
mod component;
mod render;
mod tree;

pub use id::ElementId;
pub use element::{Element, LayoutConstraints};
pub use component::ComponentElement;
pub use render::{RenderElement, RenderObject};
pub use tree::{ElementTree, StateRegistry, generate_element_id};
