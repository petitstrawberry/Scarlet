//! Spacer View - Empty space for layout
//!
//! Spacer creates flexible empty space in layouts.

use core::any::Any;
use crate::view::View;
use crate::element::{Element, RenderElement, ElementRenderObject};
use crate::geometry::{Size, Point};
use alloc::boxed::Box;

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
        Box::new(RenderElement::new(
            self.clone(),
            SpacerRenderObject,
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
pub struct SpacerRenderObject;

impl ElementRenderObject for SpacerRenderObject {
    fn layout(&mut self, constraints: crate::element::LayoutConstraints) -> Size {
        // Spacer simply returns the constraints as size
        // The parent (VStack/HStack) is responsible for providing proper constraints
        Size {
            width: constraints.max_width,
            height: constraints.max_height,
        }
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
