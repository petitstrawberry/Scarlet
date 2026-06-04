//! RenderTree - RenderObject tree derived from the Element tree
//!
//! This keeps a separate render tree (Flutter-style) by projecting the
//! Element tree into RenderNodes that carry RenderObjects plus layout data.

use alloc::vec::Vec;

use crate::element::{Element, ElementId, ElementRenderObject};
use crate::geometry::Point;

/// RenderNode - a node in the RenderObject tree
pub struct RenderNode<'a> {
    id: ElementId,
    render_object: Option<&'a dyn ElementRenderObject>,
    position: Point,
    children: Vec<RenderNode<'a>>,
}

impl<'a> RenderNode<'a> {
    /// Create a RenderNode from an Element (recursive)
    fn from_element(element: &'a dyn Element) -> Self {
        let mut children = Vec::new();
        for child in element.children() {
            children.push(Self::from_element(child.as_ref()));
        }

        let has_ro = element.render_object().is_some();
        let position = element.position();
        if crate::debug::is_enabled() {
            crate::logln!(
                "[RenderTree] node id={} type={} render_object={} pos=({}, {}) children={}",
                element.id().get(),
                element.type_name_debug(),
                has_ro,
                position.x,
                position.y,
                children.len()
            );
        }

        Self {
            id: element.id(),
            render_object: element.render_object(),
            position,
            children,
        }
    }

    /// Get the ElementId of this node
    pub fn id(&self) -> ElementId {
        self.id
    }

    /// Get the RenderObject (if any)
    pub fn render_object(&self) -> Option<&'a dyn ElementRenderObject> {
        self.render_object
    }

    /// Get the local position for this node
    pub fn position(&self) -> Point {
        self.position
    }

    /// Get the children of this node
    pub fn children(&self) -> &[RenderNode<'a>] {
        &self.children
    }
}

/// RenderTree - root of the RenderObject tree
pub struct RenderTree<'a> {
    root: RenderNode<'a>,
}

impl<'a> RenderTree<'a> {
    /// Build a RenderTree from the Element tree
    pub fn build(root: &'a dyn Element) -> Self {
        Self {
            root: RenderNode::from_element(root),
        }
    }

    /// Get the root node
    pub fn root(&self) -> &RenderNode<'a> {
        &self.root
    }
}
