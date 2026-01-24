//! ZStack - Layered stack layout container
//!
//! Arranges children layered on top of each other.

use core::any::Any;
use crate::view::View;
use crate::element::{Element, RenderElement, ElementRenderObject};
use crate::geometry::{Size, Point};
use crate::element::LayoutConstraints;
use alloc::boxed::Box;
use super::ViewTuple;

/// ZStack View - arranges children in layers
///
/// # Examples
///
/// ```ignore
/// let stack = ZStack::new((
///     Rectangle::new().fill(Color::BLUE),
///     Text::new("Overlay"),
/// ))
/// .alignment(Alignment::Center);
/// ```
pub struct ZStack<C: ViewTuple> {
    content: C,
    alignment: crate::geometry::Alignment,
}

impl<C: ViewTuple> ZStack<C> {
    /// Create a new ZStack with the given content tuple
    pub fn new(content: C) -> Self {
        Self {
            content,
            alignment: crate::geometry::Alignment::Center,
        }
    }

    /// Set alignment for children
    pub fn alignment(mut self, alignment: crate::geometry::Alignment) -> Self {
        self.alignment = alignment;
        self
    }

    /// Get alignment
    pub fn get_alignment(&self) -> crate::geometry::Alignment {
        self.alignment
    }
}

impl<C: ViewTuple + Clone> Clone for ZStack<C> {
    fn clone(&self) -> Self {
        Self {
            content: self.content.clone(),
            alignment: self.alignment,
        }
    }
}

impl<C: ViewTuple + Clone + 'static> View for ZStack<C> {
    fn create_element(&self) -> Box<dyn Element> {
        Box::new(RenderElement::new(
            self.clone(),
            ZStackRenderObject::new(self.alignment),
        ))
    }

    fn listenables(&self) -> alloc::vec::Vec<&dyn crate::state::Listenable> {
        let mut listenables = alloc::vec::Vec::new();
        self.content.collect_listenables(&mut listenables);
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
    pub fn new(alignment: crate::geometry::Alignment) -> Self {
        Self {
            alignment,
            child_count: 0,
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

    fn render(&mut self) {
        // Container doesn't directly render - children handle their own rendering
    }
}
