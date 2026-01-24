//! VStack - Vertical stack layout container
//!
//! Arranges children in a vertical column with spacing.

use alloc::vec::Vec;
use core::any::Any;
use crate::view::View;
use crate::element::{Element, RenderElement, ElementRenderObject};
use crate::geometry::{Size, Point};
use crate::element::LayoutConstraints;
use alloc::boxed::Box;
use super::ViewTuple;

/// VStack View - arranges children vertically
///
/// # Examples
///
/// ```ignore
/// let stack = VStack::new((
///     Text::new("Hello"),
///     Text::new("World"),
/// ))
/// .spacing(10.0)
/// .alignment(Alignment::Center);
/// ```
pub struct VStack<C: ViewTuple> {
    content: C,
    spacing: f32,
    alignment: crate::geometry::Alignment,
}

impl<C: ViewTuple> VStack<C> {
    /// Create a new VStack with the given content tuple
    pub fn new(content: C) -> Self {
        Self {
            content,
            spacing: 0.0,
            alignment: crate::geometry::Alignment::Center,
        }
    }

    /// Set spacing between children
    pub fn spacing(mut self, spacing: f32) -> Self {
        self.spacing = spacing;
        self
    }

    /// Set alignment for children
    pub fn alignment(mut self, alignment: crate::geometry::Alignment) -> Self {
        self.alignment = alignment;
        self
    }

    /// Get spacing
    pub fn get_spacing(&self) -> f32 {
        self.spacing
    }

    /// Get alignment
    pub fn get_alignment(&self) -> crate::geometry::Alignment {
        self.alignment
    }
}

impl<C: ViewTuple + Clone> Clone for VStack<C> {
    fn clone(&self) -> Self {
        Self {
            content: self.content.clone(),
            spacing: self.spacing,
            alignment: self.alignment,
        }
    }
}

impl<C: ViewTuple + Clone + 'static> View for VStack<C> {
    fn create_element(&self) -> Box<dyn Element> {
        let children = self.content.create_elements();
        Box::new(RenderElement::with_children(
            self.clone(),
            VStackRenderObject::new(self.spacing, self.alignment, children.len()),
            children,
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

/// VStack RenderObject
pub struct VStackRenderObject {
    spacing: f32,
    alignment: crate::geometry::Alignment,
    child_count: usize,
    size: Size,
    child_positions: Vec<Point>,
}

impl VStackRenderObject {
    /// Create a new VStackRenderObject
    pub fn new(spacing: f32, alignment: crate::geometry::Alignment, child_count: usize) -> Self {
        Self {
            spacing,
            alignment,
            child_count,
            size: Size::ZERO,
            child_positions: Vec::new(),
        }
    }

    /// Calculate child positions
    fn calculate_positions(&mut self, child_sizes: &[Size]) {
        self.child_positions.clear();
        let mut y_offset = 0.0;

        for size in child_sizes {
            self.child_positions.push(Point { x: 0.0, y: y_offset });
            y_offset += size.height + self.spacing;
        }
    }
}

impl ElementRenderObject for VStackRenderObject {
    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        // For now, calculate based on child count (actual child layout would require element tree)
        let spacing_total = if self.child_count > 0 {
            (self.child_count - 1) as f32 * self.spacing
        } else {
            0.0
        };

        let width = constraints.min_width.max(constraints.max_width.min(400.0));
        let height = (self.child_count as f32 * 20.0) + spacing_total; // Approximate 20px per child

        self.size = Size { width, height };

        // Calculate positions for children
        let mut y_offset = 0.0;
        self.child_positions.clear();
        for _ in 0..self.child_count {
            self.child_positions.push(Point { x: 0.0, y: y_offset });
            y_offset += 20.0 + self.spacing;
        }

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
