//! HStack - Horizontal stack layout container
//!
//! Arranges children in a horizontal row with spacing.

use alloc::vec::Vec;
use core::any::Any;
use crate::view::View;
use crate::element::{Element, RenderElement, ElementRenderObject, LayoutConstraints};
use crate::geometry::{Size, Point};
use alloc::boxed::Box;
use super::ViewTuple;

/// HStack View - arranges children horizontally
///
/// # Examples
///
/// ```ignore
/// let stack = HStack::new((
///     Text::new("Left"),
///     Spacer::new(),
///     Text::new("Right"),
/// ))
/// .spacing(10.0);
/// ```
pub struct HStack<C: ViewTuple> {
    content: C,
    spacing: f32,
    alignment: crate::geometry::Alignment,
}

impl<C: ViewTuple> HStack<C> {
    /// Create a new HStack with the given content tuple
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

impl<C: ViewTuple + Clone> Clone for HStack<C> {
    fn clone(&self) -> Self {
        Self {
            content: self.content.clone(),
            spacing: self.spacing,
            alignment: self.alignment,
        }
    }
}

impl<C: ViewTuple + Clone + 'static> View for HStack<C> {
    fn create_element(&self) -> Box<dyn Element> {
        let children = self.content.create_elements();
        Box::new(RenderElement::with_children(
            self.clone(),
            HStackRenderObject::new(self.spacing, self.alignment),
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

/// HStack RenderObject - handles horizontal layout
pub struct HStackRenderObject {
    spacing: f32,
    alignment: crate::geometry::Alignment,
    size: Size,
}

impl HStackRenderObject {
    pub fn new(spacing: f32, alignment: crate::geometry::Alignment) -> Self {
        Self {
            spacing,
            alignment,
            size: Size::ZERO,
        }
    }
}

impl ElementRenderObject for HStackRenderObject {
    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        self.size = Size {
            width: constraints.min_width.min(constraints.max_width),
            height: constraints.min_height.min(constraints.max_height),
        };
        self.size
    }

    fn layout_with_children(
        &mut self,
        constraints: LayoutConstraints,
        children: &mut [Box<dyn Element>],
    ) -> Size {
        if crate::debug::is_enabled() {
            scarlet_std::println!("[HStackRenderObject::layout] START: constraints=({:?}, {:?}) -> ({:?}, {:?})",
                constraints.min_width, constraints.min_height, constraints.max_width, constraints.max_height);
        }
        let child_count = children.len();
        let spacing_total = if child_count > 1 {
            (child_count - 1) as f32 * self.spacing
        } else {
            0.0
        };

        let mut fixed_total_width: f32 = 0.0;
        let mut max_height: f32 = 0.0;
        let mut flex_total: u32 = 0;
        let mut child_sizes: Vec<Size> = Vec::with_capacity(children.len());

        for child in children.iter_mut() {
            let flex = child.flex_factor();
            flex_total += flex;

            if flex == 0 {
                let child_constraints = LayoutConstraints::loose(constraints.max_width, constraints.max_height);
                let child_size = child.layout(child_constraints);
                child_sizes.push(child_size);
                fixed_total_width += child_size.width;
                max_height = max_height.max(child_size.height);
            } else {
                child_sizes.push(Size::ZERO);
            }
        }

        let remaining_width = (constraints.max_width - fixed_total_width - spacing_total).max(0.0);
        let mut child_x_offset = 0.0;

        for (i, child) in children.iter_mut().enumerate() {
            let flex = child.flex_factor();
            let child_size = if flex == 0 {
                child_sizes[i]
            } else {
                let share = if flex_total > 0 {
                    remaining_width / flex_total as f32 * flex as f32
                } else {
                    remaining_width
                };
                let child_constraints = LayoutConstraints {
                    min_width: share,
                    max_width: share,
                    min_height: 0.0,
                    max_height: 0.0,
                };
                child.layout(child_constraints)
            };

            let child_y = self.alignment.align_y(max_height, child_size.height);
            child.set_position(Point::new(child_x_offset, child_y));
            child_x_offset += child_size.width;
            if i < child_count.saturating_sub(1) {
                child_x_offset += self.spacing;
            }
        }

        let final_height = if constraints.min_height == constraints.max_height && constraints.min_height.is_finite() && constraints.min_height > 0.0 {
            if crate::debug::is_enabled() {
                scarlet_std::println!("[HStackRenderObject::layout] tight height detected, using constraint max_height");
            }
            constraints.max_height
        } else if constraints.max_height.is_finite() {
            if crate::debug::is_enabled() {
                scarlet_std::println!("[HStackRenderObject::layout] loose height with finite max, using min(max_height, max_height)");
            }
            max_height.min(constraints.max_height)
        } else {
            if crate::debug::is_enabled() {
                scarlet_std::println!("[HStackRenderObject::layout] loose height, using max_height from content");
            }
            max_height
        };

        let final_width = if constraints.min_width == constraints.max_width && constraints.min_width.is_finite() && constraints.min_width > 0.0 {
            if crate::debug::is_enabled() {
                scarlet_std::println!("[HStackRenderObject::layout] tight width detected, using constraint max_width");
            }
            constraints.max_width
        } else {
            if crate::debug::is_enabled() {
                scarlet_std::println!("[HStackRenderObject::layout] loose width, using child_x_offset from content");
            }
            child_x_offset.min(constraints.max_width)
        };

        self.size = Size {
            width: final_width,
            height: final_height,
        };
        if crate::debug::is_enabled() {
            scarlet_std::println!("[HStackRenderObject::layout] FINAL: size={}x{}", self.size.width, self.size.height);
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
        // Container doesn't directly render.
    }
}
