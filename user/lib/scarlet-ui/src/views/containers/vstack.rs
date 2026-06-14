//! VStack - Vertical stack layout container
//!
//! Arranges children in a vertical column with spacing.

use super::ViewTuple;
use crate::element::{Element, ElementRenderObject, LayoutConstraints, RenderElement};
use crate::geometry::{Point, Size};
use crate::view::View;
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::any::Any;

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
            VStackRenderObject::new(self.spacing, self.alignment),
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

/// VStack RenderObject - handles vertical layout
pub struct VStackRenderObject {
    spacing: f32,
    alignment: crate::geometry::Alignment,
    size: Size,
    child_sizes: Vec<Size>,
    greedy_indices: Vec<usize>,
}

impl VStackRenderObject {
    pub fn new(spacing: f32, alignment: crate::geometry::Alignment) -> Self {
        Self {
            spacing,
            alignment,
            size: Size::ZERO,
            child_sizes: Vec::new(),
            greedy_indices: Vec::new(),
        }
    }
}

impl ElementRenderObject for VStackRenderObject {
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
            crate::logln!(
                "[VStackRenderObject::layout] START: constraints=({:?}, {:?}) -> ({:?}, {:?})",
                constraints.min_width,
                constraints.min_height,
                constraints.max_width,
                constraints.max_height
            );
        }
        let child_count = children.len();
        let spacing_total = if child_count > 1 {
            (child_count - 1) as f32 * self.spacing
        } else {
            0.0
        };

        let mut fixed_total_height: f32 = 0.0;
        let mut max_width: f32 = 0.0;
        let mut flex_total: u32 = 0;
        self.greedy_indices.clear();
        self.child_sizes.clear();
        self.child_sizes.resize(child_count, Size::ZERO);

        for (index, child) in children.iter_mut().enumerate() {
            let flex = child.flex_factor();
            flex_total += flex;

            if flex == 0 {
                let fill_cross = child.fill_width();
                let child_constraints = if fill_cross {
                    LayoutConstraints::new(0.0, f32::INFINITY, 0.0, constraints.max_height)
                } else {
                    LayoutConstraints::new(0.0, constraints.max_width, 0.0, constraints.max_height)
                };
                let child_size = child.layout(child_constraints);
                self.child_sizes[index] = child_size;
                if constraints.max_height.is_finite()
                    && constraints.max_height > 0.0
                    && child_size.height + 0.5 >= constraints.max_height
                {
                    self.greedy_indices.push(index);
                } else {
                    fixed_total_height += child_size.height;
                    max_width = max_width.max(child_size.width);
                }
            }
        }

        if flex_total == 0
            && !self.greedy_indices.is_empty()
            && constraints.max_height.is_finite()
            && constraints.max_height > 0.0
        {
            let share = (constraints.max_height - fixed_total_height - spacing_total).max(0.0)
                / self.greedy_indices.len() as f32;
            for &index in self.greedy_indices.iter() {
                let child = &mut children[index];
                let child_constraints = LayoutConstraints {
                    min_width: 0.0,
                    max_width: constraints.max_width,
                    min_height: share,
                    max_height: share,
                };
                let child_size = child.layout(child_constraints);
                self.child_sizes[index] = child_size;
                fixed_total_height += child_size.height;
                max_width = max_width.max(child_size.width);
            }
        }

        let max_height_finite = constraints.max_height.is_finite() && constraints.max_height > 0.0;
        let remaining_height = if max_height_finite {
            (constraints.max_height - fixed_total_height - spacing_total).max(0.0)
        } else {
            0.0
        };
        let mut total_height = fixed_total_height + spacing_total;

        for (i, child) in children.iter_mut().enumerate() {
            let flex = child.flex_factor();
            if flex > 0 {
                let share = if flex_total > 0 {
                    remaining_height / flex_total as f32 * flex as f32
                } else {
                    remaining_height
                };
                let child_constraints = LayoutConstraints {
                    min_width: 0.0,
                    max_width: constraints.max_width,
                    min_height: if max_height_finite { share } else { 0.0 },
                    max_height: if max_height_finite { share } else { 0.0 },
                };
                let child_size = child.layout(child_constraints);
                self.child_sizes[i] = child_size;
                total_height += child_size.height;
                max_width = max_width.max(child_size.width);
            }
        }

        if crate::debug::is_enabled() {
            crate::logln!(
                "[VStackRenderObject::layout] after measuring: total_height={}, max_width={}",
                total_height,
                max_width
            );
        }

        let final_width = if constraints.min_width == constraints.max_width
            && constraints.min_width.is_finite()
            && constraints.min_width > 0.0
        {
            if crate::debug::is_enabled() {
                crate::logln!(
                    "[VStackRenderObject::layout] tight width detected, using constraint max_width"
                );
            }
            constraints.max_width
        } else {
            if crate::debug::is_enabled() {
                crate::logln!(
                    "[VStackRenderObject::layout] loose width, using max_width from content"
                );
            }
            max_width
        };

        let final_height = if constraints.min_height == constraints.max_height
            && constraints.min_height.is_finite()
            && constraints.min_height > 0.0
        {
            if crate::debug::is_enabled() {
                crate::logln!(
                    "[VStackRenderObject::layout] tight height detected, using constraint max_height"
                );
            }
            constraints.max_height
        } else if constraints.max_height.is_finite() {
            if crate::debug::is_enabled() {
                crate::logln!(
                    "[VStackRenderObject::layout] loose height with finite max, using min(total_height, max_height)"
                );
            }
            total_height.min(constraints.max_height)
        } else {
            if crate::debug::is_enabled() {
                crate::logln!(
                    "[VStackRenderObject::layout] loose height with inf max, using total_height"
                );
            }
            total_height
        };

        for (index, child) in children.iter_mut().enumerate() {
            if !child.fill_width() {
                continue;
            }
            let height = self.child_sizes[index].height;
            let child_constraints = LayoutConstraints {
                min_width: final_width,
                max_width: final_width,
                min_height: height,
                max_height: height,
            };
            let child_size = child.layout(child_constraints);
            self.child_sizes[index] = child_size;
        }

        let mut child_y_offset = 0.0;
        for (i, child) in children.iter_mut().enumerate() {
            let child_size = self.child_sizes[i];
            let child_x = self.alignment.align_x(final_width, child_size.width);
            child.set_position(Point::new(child_x, child_y_offset));
            child_y_offset += child_size.height;
            if i < child_count.saturating_sub(1) {
                child_y_offset += self.spacing;
            }
        }

        self.size = Size {
            width: final_width,
            height: final_height,
        };
        if crate::debug::is_enabled() {
            crate::logln!(
                "[VStackRenderObject::layout] FINAL: size={}x{}",
                final_width,
                final_height
            );
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
