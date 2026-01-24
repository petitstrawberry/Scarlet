//! Padding View Modifier
//!
//! Adds padding around a child view.

use core::any::Any;
use crate::view::View;
use crate::element::{Element, RenderElement, ElementRenderObject};
use crate::geometry::{Size, Point};
use crate::element::LayoutConstraints;
use crate::geometry::EdgeInsets;
use alloc::boxed::Box;
use alloc::vec;

/// Padding view modifier - adds space around a child view
#[derive(Clone)]
pub struct Padding<V: View> {
    inner: V,
    insets: EdgeInsets,
}

impl<V: View> Padding<V> {
    /// Create a new Padding modifier with uniform padding
    pub fn new(inner: V, padding: f32) -> Self {
        Self {
            inner,
            insets: EdgeInsets::all(padding),
        }
    }

    /// Create a new Padding modifier with custom insets
    pub fn with_insets(inner: V, insets: EdgeInsets) -> Self {
        Self {
            inner,
            insets,
        }
    }

    /// Get the inner view
    pub fn inner(&self) -> &V {
        &self.inner
    }

    /// Get the insets
    pub fn insets(&self) -> EdgeInsets {
        self.insets
    }
}

impl<V: View + Clone> View for Padding<V> {
    fn create_element(&self) -> Box<dyn Element> {
        Box::new(RenderElement::with_children(
            self.clone(),
            PaddingRenderObject::new(self.insets),
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

/// Padding RenderObject
pub struct PaddingRenderObject {
    insets: EdgeInsets,
    size: Size,
}

impl PaddingRenderObject {
    /// Create a new PaddingRenderObject
    pub fn new(insets: EdgeInsets) -> Self {
        Self {
            insets,
            size: Size::ZERO,
        }
    }
}

impl ElementRenderObject for PaddingRenderObject {
    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        // Calculate padding contribution
        let horizontal_padding = self.insets.left + self.insets.right;
        let vertical_padding = self.insets.top + self.insets.bottom;

        // Padding takes up at least its padding size
        let width = horizontal_padding.max(constraints.min_width);
        let height = vertical_padding.max(constraints.min_height);

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
