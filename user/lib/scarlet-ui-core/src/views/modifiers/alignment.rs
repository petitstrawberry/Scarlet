//! Alignment View Modifier
//!
//! Controls the alignment of a child view within its available space.

use crate::element::LayoutConstraints;
use crate::element::{Element, ElementRenderObject, RenderElement};
use crate::geometry::Alignment;
use crate::geometry::{Point, Size};
use crate::view::View;
use alloc::boxed::Box;
use alloc::vec;
use core::any::Any;

/// Alignment view modifier - controls child alignment
#[derive(Clone)]
pub struct AlignmentFrame<V: View> {
    inner: V,
    alignment: Alignment,
}

impl<V: View> AlignmentFrame<V> {
    /// Create a new AlignmentFrame modifier
    pub fn new(inner: V, alignment: Alignment) -> Self {
        Self { inner, alignment }
    }

    /// Get the inner view
    pub fn inner(&self) -> &V {
        &self.inner
    }

    /// Get the alignment
    pub fn get_alignment(&self) -> Alignment {
        self.alignment
    }
}

impl<V: View + Clone> View for AlignmentFrame<V> {
    fn create_element(&self) -> Box<dyn Element> {
        Box::new(RenderElement::with_children(
            self.clone(),
            AlignmentRenderObject::new(self.alignment),
            vec![self.inner.create_element()],
        ))
    }

    fn listenables(&self) -> alloc::vec::Vec<&dyn crate::state::Listenable> {
        self.inner.listenables()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Alignment RenderObject
pub struct AlignmentRenderObject {
    alignment: Alignment,
    size: Size,
}

impl AlignmentRenderObject {
    /// Create a new AlignmentRenderObject
    pub fn new(alignment: Alignment) -> Self {
        Self {
            alignment,
            size: Size::ZERO,
        }
    }

    /// Get the alignment
    pub fn get_alignment(&self) -> Alignment {
        self.alignment
    }
}

impl ElementRenderObject for AlignmentRenderObject {
    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        // Alignment frame takes all available space
        let width = if constraints.max_width > 0.0 {
            constraints.max_width.max(constraints.min_width)
        } else {
            constraints.min_width.max(0.0)
        };

        let height = if constraints.max_height > 0.0 {
            constraints.max_height.max(constraints.min_height)
        } else {
            constraints.min_height.max(0.0)
        };

        self.size = Size { width, height };
        self.size
    }

    fn layout_with_children(
        &mut self,
        constraints: LayoutConstraints,
        children: &mut [Box<dyn Element>],
    ) -> Size {
        if let Some(child) = children.first_mut() {
            let child_constraints =
                LayoutConstraints::new(0.0, constraints.max_width, 0.0, constraints.max_height);
            let child_size = child.layout(child_constraints);

            let width = if constraints.max_width.is_finite() && constraints.max_width > 0.0 {
                constraints.max_width.max(constraints.min_width)
            } else {
                child_size.width.max(constraints.min_width)
            };
            let height = if constraints.max_height.is_finite() && constraints.max_height > 0.0 {
                constraints.max_height.max(constraints.min_height)
            } else {
                child_size.height.max(constraints.min_height)
            };

            self.size = Size { width, height };
            let origin = self.alignment.apply(self.size, child_size);
            child.set_position(origin);
            self.size
        } else {
            self.layout(constraints)
        }
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
        // Modifier doesn't directly render - child handles its own rendering
    }
}
