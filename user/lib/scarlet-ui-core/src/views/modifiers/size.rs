//! Size View Modifier
//!
//! Constrains a child view to minimum/maximum dimensions.

use crate::element::LayoutConstraints;
use crate::element::{Element, ElementRenderObject, RenderElement};
use crate::geometry::{Point, Size};
use crate::view::View;
use alloc::boxed::Box;
use alloc::vec;
use core::any::Any;

/// Size view modifier - sets size constraints
#[derive(Clone)]
pub struct SetSize<V: View> {
    inner: V,
    min_width: f32,
    min_height: f32,
    max_width: f32,
    max_height: f32,
}

impl<V: View> SetSize<V> {
    /// Create a new SetSize modifier with all constraints
    pub fn new(inner: V, min_width: f32, min_height: f32, max_width: f32, max_height: f32) -> Self {
        Self {
            inner,
            min_width,
            min_height,
            max_width,
            max_height,
        }
    }

    /// Set minimum width
    pub fn min_width(mut self, width: f32) -> Self {
        self.min_width = width;
        self
    }

    /// Set minimum height
    pub fn min_height(mut self, height: f32) -> Self {
        self.min_height = height;
        self
    }

    /// Set maximum width
    pub fn max_width(mut self, width: f32) -> Self {
        self.max_width = width;
        self
    }

    /// Set maximum height
    pub fn max_height(mut self, height: f32) -> Self {
        self.max_height = height;
        self
    }

    /// Get the inner view
    pub fn inner(&self) -> &V {
        &self.inner
    }
}

impl<V: View + Clone> View for SetSize<V> {
    fn create_element(&self) -> Box<dyn Element> {
        Box::new(RenderElement::with_children(
            self.clone(),
            SizeRenderObject::new(
                self.min_width,
                self.min_height,
                self.max_width,
                self.max_height,
            ),
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

/// Size RenderObject
pub struct SizeRenderObject {
    min_width: f32,
    min_height: f32,
    max_width: f32,
    max_height: f32,
    size: Size,
}

impl SizeRenderObject {
    /// Create a new SizeRenderObject
    pub fn new(min_width: f32, min_height: f32, max_width: f32, max_height: f32) -> Self {
        Self {
            min_width,
            min_height,
            max_width,
            max_height,
            size: Size::ZERO,
        }
    }
}

impl ElementRenderObject for SizeRenderObject {
    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        // Combine the constraints from SetSize with the parent constraints
        let min_width = self.min_width.max(constraints.min_width);
        let min_height = self.min_height.max(constraints.min_height);
        let max_width = self.max_width.min(constraints.max_width);
        let max_height = self.max_height.min(constraints.max_height);

        // If no constraints, use minimum size
        let width = if max_width > 0.0 {
            min_width.max(0.0).min(max_width)
        } else {
            min_width
        };

        let height = if max_height > 0.0 {
            min_height.max(0.0).min(max_height)
        } else {
            min_height
        };

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
        // Modifier doesn't directly render - child handles its own rendering
    }
}
