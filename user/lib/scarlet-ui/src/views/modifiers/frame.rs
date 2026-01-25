//! Frame View Modifier
//!
//! Constrains a child view to a specific size.

use core::any::Any;
use crate::view::View;
use crate::element::{Element, RenderElement, ElementRenderObject};
use crate::geometry::{Size, Point};
use crate::element::LayoutConstraints;
use alloc::boxed::Box;
use alloc::vec;

/// Frame view modifier - constrains a child to a specific size
#[derive(Clone)]
pub struct Frame<V: View> {
    inner: V,
    width: Option<f32>,
    height: Option<f32>,
    min_width: f32,
    min_height: f32,
    max_width: f32,
    max_height: f32,
}

impl<V: View> Frame<V> {
    /// Create a new Frame modifier with fixed size
    pub fn new(inner: V, width: f32, height: f32) -> Self {
        Self {
            inner,
            width: Some(width),
            height: Some(height),
            min_width: width,
            min_height: height,
            max_width: width,
            max_height: height,
        }
    }

    /// Create a new Frame modifier with width only
    pub fn width(inner: V, width: f32) -> Self {
        Self {
            inner,
            width: Some(width),
            height: None,
            min_width: width,
            min_height: 0.0,
            max_width: width,
            max_height: f32::INFINITY,
        }
    }

    /// Create a new Frame modifier with height only
    pub fn height(inner: V, height: f32) -> Self {
        Self {
            inner,
            width: None,
            height: Some(height),
            min_width: 0.0,
            min_height: height,
            max_width: f32::INFINITY,
            max_height: height,
        }
    }

    /// Set minimum width
    pub fn min_width(mut self, min_width: f32) -> Self {
        self.min_width = min_width;
        self
    }

    /// Set minimum height
    pub fn min_height(mut self, min_height: f32) -> Self {
        self.min_height = min_height;
        self
    }

    /// Set maximum width
    pub fn max_width(mut self, max_width: f32) -> Self {
        self.max_width = max_width;
        self
    }

    /// Set maximum height
    pub fn max_height(mut self, max_height: f32) -> Self {
        self.max_height = max_height;
        self
    }

    /// Get the inner view
    pub fn inner(&self) -> &V {
        &self.inner
    }

    /// Get the width constraint
    pub fn width_value(&self) -> Option<f32> {
        self.width
    }

    /// Get the height constraint
    pub fn height_value(&self) -> Option<f32> {
        self.height
    }
}

impl<V: View + Clone> View for Frame<V> {
    fn create_element(&self) -> Box<dyn Element> {
        Box::new(RenderElement::with_children(
            self.clone(),
            FrameRenderObject::new(self.width, self.height, self.min_width, self.min_height, self.max_width, self.max_height),
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

/// Frame RenderObject
pub struct FrameRenderObject {
    width: Option<f32>,
    height: Option<f32>,
    min_width: f32,
    min_height: f32,
    max_width: f32,
    max_height: f32,
    size: Size,
}

impl FrameRenderObject {
    /// Create a new FrameRenderObject
    pub fn new(
        width: Option<f32>,
        height: Option<f32>,
        min_width: f32,
        min_height: f32,
        max_width: f32,
        max_height: f32,
    ) -> Self {
        Self {
            width,
            height,
            min_width,
            min_height,
            max_width,
            max_height,
            size: Size::ZERO,
        }
    }
}

impl ElementRenderObject for FrameRenderObject {
    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        scarlet_std::println!("[FrameRenderObject::layout] START: constraints=({:?}, {:?}) -> ({:?}, {:?}), self.width={:?}, self.height={:?}",
            constraints.min_width, constraints.min_height, constraints.max_width, constraints.max_height, self.width, self.height);
        // Determine the size based on explicit width/height and constraints
        let width = if let Some(w) = self.width {
            if w.is_finite() {
                // Explicit finite width
                w
            } else {
                // INFINITY width - use parent's max_width if finite, else min_width
                if constraints.max_width.is_finite() {
                    constraints.max_width
                } else {
                    constraints.min_width.max(self.min_width)
                }
            }
        } else {
            // No explicit width - use constraints
            constraints.min_width
                .max(self.min_width)
                .min(constraints.max_width.min(self.max_width))
        };

        let height = if let Some(h) = self.height {
            if h.is_finite() {
                // Explicit finite height
                h
            } else {
                // INFINITY height - use parent's max_height if finite, else min_height
                if constraints.max_height.is_finite() {
                    constraints.max_height
                } else {
                    constraints.min_height.max(self.min_height)
                }
            }
        } else {
            // No explicit height - use constraints
            constraints.min_height
                .max(self.min_height)
                .min(constraints.max_height.min(self.max_height))
        };

        self.size = Size { width, height };
        scarlet_std::println!("[FrameRenderObject::layout] FINAL: size={}x{}", width, height);
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
