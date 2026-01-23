//! Background View Modifier
//!
//! Adds a background color behind a child view.

use core::any::Any;
use crate::view::View;
use crate::element::{Element, RenderElement, ElementRenderObject};
use crate::geometry::{Size, Point};
use crate::element::LayoutConstraints;
use crate::color::Color;
use alloc::boxed::Box;

/// Background view modifier - adds a background color
#[derive(Clone)]
pub struct Background<V: View> {
    inner: V,
    color: Color,
}

impl<V: View> Background<V> {
    /// Create a new Background modifier
    pub fn new(inner: V, color: Color) -> Self {
        Self {
            inner,
            color,
        }
    }

    /// Get the inner view
    pub fn inner(&self) -> &V {
        &self.inner
    }

    /// Get the background color
    pub fn background_color(&self) -> Color {
        self.color
    }
}

impl<V: View + Clone> View for Background<V> {
    fn create_element(&self) -> Box<dyn Element> {
        Box::new(RenderElement::new(
            BackgroundRenderObject::new(self.color),
        ))
    }

    fn listenables(&self) -> alloc::vec::Vec<&dyn crate::state::Listenable> {
        self.inner.listenables()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Background RenderObject
pub struct BackgroundRenderObject {
    color: Color,
    size: Size,
}

impl BackgroundRenderObject {
    /// Create a new BackgroundRenderObject
    pub fn new(color: Color) -> Self {
        Self {
            color,
            size: Size::ZERO,
        }
    }

    /// Get the background color
    pub fn get_color(&self) -> Color {
        self.color
    }
}

impl ElementRenderObject for BackgroundRenderObject {
    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        // Background takes at least the minimum size
        let width = constraints.min_width.max(1.0);
        let height = constraints.min_height.max(1.0);

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
