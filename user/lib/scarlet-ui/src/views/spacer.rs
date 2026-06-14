//! Spacer View - Empty space for layout
//!
//! Spacer creates flexible empty space in layouts.

use crate::element::{Element, ElementRenderObject, RenderElement};
use crate::geometry::{Point, Size};
use crate::view::View;
use alloc::boxed::Box;
use core::any::Any;

/// Spacer View - creates empty space
#[derive(Clone)]
pub struct Spacer;

impl Spacer {
    /// Create a new Spacer that expands to fill available space
    pub fn new() -> Self {
        Self
    }
}

impl Default for Spacer {
    fn default() -> Self {
        Self::new()
    }
}

impl View for Spacer {
    fn create_element(&self) -> Box<dyn Element> {
        Box::new(RenderElement::new(self.clone(), SpacerRenderObject))
    }

    fn listenables(&self) -> alloc::vec::Vec<&dyn crate::state::Listenable> {
        alloc::vec::Vec::new()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Spacer RenderObject - handles space allocation
pub struct SpacerRenderObject;

impl ElementRenderObject for SpacerRenderObject {
    fn layout(&mut self, constraints: crate::element::LayoutConstraints) -> Size {
        // Spacer only expands on the axis where constraints are tight (min == max).
        let width = if constraints.min_width == constraints.max_width {
            constraints.max_width
        } else {
            0.0
        };
        let height = if constraints.min_height == constraints.max_height {
            constraints.max_height
        } else {
            0.0
        };
        Size { width, height }
    }

    fn size(&self) -> Size {
        Size::ZERO
    }

    fn hit_test(&self, _point: Point) -> bool {
        false
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn render(&mut self) {
        // Spacer is invisible
    }
}
