//! Spacer View - Empty space for layout
//!
//! Spacer creates flexible or fixed empty space in layouts.

use core::any::Any;
use crate::view::View;
use crate::element::{Element, RenderElement, ElementRenderObject};
use crate::geometry::{Size, Point};
use alloc::boxed::Box;

/// Spacer View - creates empty space
#[derive(Clone)]
pub struct Spacer {
    min_width: f32,
    min_height: f32,
    expand: bool,
}

impl Spacer {
    /// Create a new Spacer that expands to fill available space
    pub fn new() -> Self {
        Self {
            min_width: 0.0,
            min_height: 0.0,
            expand: true,
        }
    }

    /// Create a Spacer with fixed dimensions
    pub fn fixed(width: f32, height: f32) -> Self {
        Self {
            min_width: width,
            min_height: height,
            expand: false,
        }
    }

    /// Set the minimum width
    pub fn min_width(mut self, width: f32) -> Self {
        self.min_width = width;
        self
    }

    /// Set the minimum height
    pub fn min_height(mut self, height: f32) -> Self {
        self.min_height = height;
        self
    }

    /// Set whether the spacer expands to fill available space
    pub fn expands(mut self, expand: bool) -> Self {
        self.expand = expand;
        self
    }
}

impl Default for Spacer {
    fn default() -> Self {
        Self::new()
    }
}

impl View for Spacer {
    fn create_element(&self) -> Box<dyn Element> {
        Box::new(RenderElement::new(
            SpacerRenderObject::new(self.min_width, self.min_height, self.expand),
        ))
    }

    fn listenables(&self) -> alloc::vec::Vec<&dyn crate::state::Listenable> {
        alloc::vec::Vec::new()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Spacer RenderObject - handles space allocation
pub struct SpacerRenderObject {
    min_width: f32,
    min_height: f32,
    expand: bool,
    size: Size,
}

impl SpacerRenderObject {
    /// Create a new SpacerRenderObject
    pub fn new(min_width: f32, min_height: f32, expand: bool) -> Self {
        Self {
            min_width,
            min_height,
            expand,
            size: Size::ZERO,
        }
    }
}

impl ElementRenderObject for SpacerRenderObject {
    fn layout(&mut self, constraints: crate::element::LayoutConstraints) -> Size {
        let width = if self.expand && constraints.max_width > 0.0 {
            constraints.max_width.max(self.min_width)
        } else {
            self.min_width.max(constraints.min_width)
        };

        let height = if self.expand && constraints.max_height > 0.0 {
            constraints.max_height.max(self.min_height)
        } else {
            self.min_height.max(constraints.min_height)
        };

        self.size = Size { width, height };
        self.size
    }

    fn size(&self) -> Size {
        self.size
    }

    fn hit_test(&self, _point: Point) -> bool {
        // Spacer doesn't accept hits
        false
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
