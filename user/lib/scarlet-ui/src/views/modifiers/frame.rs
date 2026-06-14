//! Frame View Modifier
//!
//! Constrains a child view to a specific size.

use crate::element::LayoutConstraints;
use crate::element::{Element, ElementRenderObject, RenderElement};
use crate::geometry::{Point, Size};
use crate::view::View;
use alloc::boxed::Box;
use alloc::vec;
use core::any::Any;

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
        let (min_width, max_width) = if width.is_finite() {
            (width, width)
        } else {
            (0.0, f32::INFINITY)
        };
        let (min_height, max_height) = if height.is_finite() {
            (height, height)
        } else {
            (0.0, f32::INFINITY)
        };
        Self {
            inner,
            width: Some(width),
            height: Some(height),
            min_width,
            min_height,
            max_width,
            max_height,
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
            FrameRenderObject::new(
                self.width,
                self.height,
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

    /// Get the explicit width value
    pub fn width_value(&self) -> Option<f32> {
        self.width
    }

    /// Get the explicit height value
    pub fn height_value(&self) -> Option<f32> {
        self.height
    }

    /// Layout the frame using a child size
    pub fn layout_with_child(&mut self, child_size: Size, constraints: LayoutConstraints) -> Size {
        let min_w = self.min_width.max(constraints.min_width);
        let mut max_w = self.max_width.min(constraints.max_width);
        let min_h = self.min_height.max(constraints.min_height);
        let mut max_h = self.max_height.min(constraints.max_height);

        if min_w > max_w {
            max_w = min_w;
        }
        if min_h > max_h {
            max_h = min_h;
        }

        let width = if let Some(w) = self.width {
            if w.is_finite() {
                w
            } else if constraints.max_width.is_finite() {
                constraints.max_width
            } else {
                child_size.width.clamp(min_w, max_w)
            }
        } else {
            child_size.width.clamp(min_w, max_w)
        };

        let height = if let Some(h) = self.height {
            if h.is_finite() {
                h
            } else if constraints.max_height.is_finite() {
                constraints.max_height
            } else {
                child_size.height.clamp(min_h, max_h)
            }
        } else {
            child_size.height.clamp(min_h, max_h)
        };

        self.size = Size { width, height };
        self.size
    }
}

impl ElementRenderObject for FrameRenderObject {
    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        if crate::debug::is_enabled() {
            crate::logln!(
                "[FrameRenderObject::layout] START: constraints=({:?}, {:?}) -> ({:?}, {:?}), self.width={:?}, self.height={:?}",
                constraints.min_width,
                constraints.min_height,
                constraints.max_width,
                constraints.max_height,
                self.width,
                self.height
            );
        }
        // Determine the size based on explicit width/height and constraints
        let width = if let Some(w) = self.width {
            if w.is_finite() {
                w
            } else if constraints.is_tight_width() {
                constraints.max_width
            } else {
                constraints
                    .min_width
                    .max(self.min_width)
                    .min(constraints.max_width.min(self.max_width))
            }
        } else {
            constraints
                .min_width
                .max(self.min_width)
                .min(constraints.max_width.min(self.max_width))
        };

        let height = if let Some(h) = self.height {
            if h.is_finite() {
                h
            } else if constraints.is_tight_height() {
                constraints.max_height
            } else {
                constraints
                    .min_height
                    .max(self.min_height)
                    .min(constraints.max_height.min(self.max_height))
            }
        } else {
            constraints
                .min_height
                .max(self.min_height)
                .min(constraints.max_height.min(self.max_height))
        };

        self.size = Size { width, height };
        if crate::debug::is_enabled() {
            crate::logln!(
                "[FrameRenderObject::layout] FINAL: size={}x{}",
                width,
                height
            );
        }
        self.size
    }

    fn layout_with_children(
        &mut self,
        constraints: LayoutConstraints,
        children: &mut [Box<dyn Element>],
    ) -> Size {
        let (min_w, max_w) = match self.width {
            Some(w) if w.is_finite() => (w, w),
            Some(_) if constraints.max_width.is_finite() => {
                (constraints.max_width, constraints.max_width)
            }
            Some(_) => (constraints.min_width, constraints.max_width),
            None => (constraints.min_width, constraints.max_width),
        };
        let (min_h, max_h) = match self.height {
            Some(h) if h.is_finite() => (h, h),
            Some(_) if constraints.max_height.is_finite() => {
                (constraints.max_height, constraints.max_height)
            }
            Some(_) => (constraints.min_height, constraints.max_height),
            None => (constraints.min_height, constraints.max_height),
        };

        let child_constraints = LayoutConstraints {
            min_width: min_w,
            max_width: max_w,
            min_height: min_h,
            max_height: max_h,
        };

        let mut child_max = Size::ZERO;
        for child in children {
            let child_size = child.layout(child_constraints);
            child_max.width = child_max.width.max(child_size.width);
            child_max.height = child_max.height.max(child_size.height);
            // Don't override position - let parent (HStack/VStack) control it
        }

        self.layout_with_child(child_max, constraints)
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
