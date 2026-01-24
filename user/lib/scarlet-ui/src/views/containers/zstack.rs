//! ZStack - Layered stack layout container
//!
//! Arranges children layered on top of each other.

use alloc::vec::Vec;
use core::any::Any;
use crate::view::View;
use crate::element::{Element, RenderElement, ElementRenderObject};
use crate::geometry::{Size, Point};
use crate::element::LayoutConstraints;
use alloc::boxed::Box;

/// ZStack View - arranges children in layers
pub struct ZStack {
    children: Vec<Box<dyn View>>,
    alignment: crate::geometry::Alignment,
}

impl ZStack {
    /// Create a new ZStack
    pub fn new() -> Self {
        Self {
            children: Vec::new(),
            alignment: crate::geometry::Alignment::Center,
        }
    }

    /// Add a child view
    pub fn add_child(mut self, child: Box<dyn View>) -> Self {
        self.children.push(child);
        self
    }

    /// Set alignment for children
    pub fn alignment(mut self, alignment: crate::geometry::Alignment) -> Self {
        self.alignment = alignment;
        self
    }

    /// Get the number of children
    pub fn child_count(&self) -> usize {
        self.children.len()
    }

    /// Get alignment
    pub fn get_alignment(&self) -> crate::geometry::Alignment {
        self.alignment
    }
}

impl Default for ZStack {
    fn default() -> Self {
        Self::new()
    }
}

impl View for ZStack {
    fn create_element(&self) -> Box<dyn Element> {
        Box::new(RenderElement::new(
            ZStackRenderObject::new(self.alignment, self.children.len()),
        ))
    }

    fn listenables(&self) -> alloc::vec::Vec<&dyn crate::state::Listenable> {
        let mut listenables = alloc::vec::Vec::new();
        for child in &self.children {
            listenables.extend(child.listenables());
        }
        listenables
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// ZStack RenderObject
pub struct ZStackRenderObject {
    alignment: crate::geometry::Alignment,
    child_count: usize,
    size: Size,
}

impl ZStackRenderObject {
    /// Create a new ZStackRenderObject
    pub fn new(alignment: crate::geometry::Alignment, child_count: usize) -> Self {
        Self {
            alignment,
            child_count,
            size: Size::ZERO,
        }
    }
}

impl ElementRenderObject for ZStackRenderObject {
    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        // ZStack takes the size of the largest child
        // For now, use constraints as size
        let width = constraints.min_width.max(constraints.max_width.min(200.0));
        let height = constraints.min_height.max(constraints.max_height.min(200.0));

        self.size = Size { width, height };
        self.size
    }

    fn size(&self) -> Size {
        self.size
    }

    fn hit_test(&self, point: Point) -> bool {
        let bounds = crate::geometry::Rect {
            origin: Point::ZERO,
            size: self.size,
        };
        bounds.contains(point)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
