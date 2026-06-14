//! ZStack - Layered stack layout container
//!
//! Arranges children layered on top of each other.

use super::ViewTuple;
use crate::element::LayoutConstraints;
use crate::element::{Element, ElementRenderObject, RenderElement};
use crate::geometry::{Point, Size};
use crate::view::View;
use alloc::boxed::Box;
use core::any::Any;

/// ZStack View - arranges children in layers
///
/// # Examples
///
/// ```ignore
/// let stack = ZStack::new((
///     Rectangle::new().fill(Color::BLUE),
///     Text::new("Overlay"),
/// ))
/// .alignment(Alignment::Center);
/// ```
pub struct ZStack<C: ViewTuple> {
    content: C,
    alignment: crate::geometry::Alignment,
}

impl<C: ViewTuple> ZStack<C> {
    /// Create a new ZStack with the given content tuple
    pub fn new(content: C) -> Self {
        Self {
            content,
            alignment: crate::geometry::Alignment::Center,
        }
    }

    /// Set alignment for children
    pub fn alignment(mut self, alignment: crate::geometry::Alignment) -> Self {
        self.alignment = alignment;
        self
    }

    /// Get alignment
    pub fn get_alignment(&self) -> crate::geometry::Alignment {
        self.alignment
    }
}

impl<C: ViewTuple + Clone> Clone for ZStack<C> {
    fn clone(&self) -> Self {
        Self {
            content: self.content.clone(),
            alignment: self.alignment,
        }
    }
}

impl<C: ViewTuple + Clone + 'static> View for ZStack<C> {
    fn create_element(&self) -> Box<dyn Element> {
        let children = self.content.create_elements();
        Box::new(RenderElement::with_children(
            self.clone(),
            ZStackRenderObject::new(self.alignment, children.len()),
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

/// ZStack RenderObject
pub struct ZStackRenderObject {
    alignment: crate::geometry::Alignment,
    child_count: usize,
    size: Size,
    child_sizes: alloc::vec::Vec<Size>,
}

impl ZStackRenderObject {
    /// Create a new ZStackRenderObject
    pub fn new(alignment: crate::geometry::Alignment, child_count: usize) -> Self {
        Self {
            alignment,
            child_count,
            size: Size::ZERO,
            child_sizes: alloc::vec::Vec::new(),
        }
    }
}

impl ElementRenderObject for ZStackRenderObject {
    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        if constraints.is_tight() {
            let width = constraints.max_width;
            let height = constraints.max_height;
            self.size = Size { width, height };
            return self.size;
        }

        let width = constraints.min_width.min(constraints.max_width).max(0.0);
        let height = constraints.min_height.min(constraints.max_height).max(0.0);
        self.size = Size { width, height };
        self.size
    }

    fn layout_with_children(
        &mut self,
        constraints: LayoutConstraints,
        children: &mut [Box<dyn Element>],
    ) -> Size {
        let child_count = children.len();
        self.child_count = child_count;
        self.child_sizes.clear();
        self.child_sizes.resize(child_count, Size::ZERO);

        let mut max_width: f32 = 0.0;
        let mut max_height: f32 = 0.0;

        for (index, child) in children.iter_mut().enumerate() {
            let child_constraints =
                LayoutConstraints::new(0.0, constraints.max_width, 0.0, constraints.max_height);
            let child_size = child.layout(child_constraints);
            self.child_sizes[index] = child_size;
            max_width = max_width.max(child_size.width);
            max_height = max_height.max(child_size.height);
        }

        let final_width = if constraints.is_tight_width()
            && constraints.min_width.is_finite()
            && constraints.min_width > 0.0
        {
            constraints.max_width
        } else {
            max_width.clamp(constraints.min_width, constraints.max_width)
        };
        let final_height = if constraints.is_tight_height()
            && constraints.min_height.is_finite()
            && constraints.min_height > 0.0
        {
            constraints.max_height
        } else {
            max_height.clamp(constraints.min_height, constraints.max_height)
        };

        self.size = Size {
            width: final_width,
            height: final_height,
        };

        for (index, child) in children.iter_mut().enumerate() {
            let mut child_size = self.child_sizes[index];

            if child.fill_width() || child.fill_height() {
                let child_constraints = LayoutConstraints {
                    min_width: if child.fill_width() {
                        final_width
                    } else {
                        child_size.width
                    },
                    max_width: if child.fill_width() {
                        final_width
                    } else {
                        child_size.width
                    },
                    min_height: if child.fill_height() {
                        final_height
                    } else {
                        child_size.height
                    },
                    max_height: if child.fill_height() {
                        final_height
                    } else {
                        child_size.height
                    },
                };
                child_size = child.layout(child_constraints);
                self.child_sizes[index] = child_size;
            }

            let origin = self.alignment.apply(self.size, child_size);
            child.set_position(origin);
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
