//! HStackElement - Horizontal stack layout element
//!
//! Arranges children in a horizontal row with spacing and alignment.

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::any::Any;

use crate::element::{Element, ElementId, LayoutConstraints};
use crate::geometry::{Alignment, Point, Rect, Size};

/// HStackElement - arranges children horizontally
pub struct HStackElement {
    id: ElementId,
    children: Vec<Box<dyn Element>>,
    spacing: f32,
    alignment: Alignment,
    position: Point,
    size: Size,
    last_constraints: Option<LayoutConstraints>,
}

impl HStackElement {
    /// Create a new HStackElement
    pub fn new(children: Vec<Box<dyn Element>>, spacing: f32, alignment: Alignment) -> Self {
        Self {
            id: ElementId::generate(),
            children,
            spacing,
            alignment,
            position: Point::ZERO,
            size: Size::ZERO,
            last_constraints: None,
        }
    }

    /// Get spacing
    pub fn spacing(&self) -> f32 {
        self.spacing
    }

    /// Get alignment
    pub fn alignment(&self) -> Alignment {
        self.alignment
    }
}

impl Element for HStackElement {
    fn id(&self) -> ElementId {
        self.id
    }

    fn type_name(&self) -> &str {
        "HStackElement"
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn children(&self) -> &[Box<dyn Element>] {
        &self.children
    }

    fn children_mut(&mut self) -> &mut [Box<dyn Element>] {
        &mut self.children
    }

    fn update(&mut self, _new_view: &dyn crate::view::View) -> crate::element::UpdateResult {
        // HStack doesn't support view-based updates
        crate::element::UpdateResult::Replaced
    }

    fn rebuild(&mut self) -> crate::element::UpdateResult {
        crate::element::UpdateResult::NoChange
    }

    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        self.last_constraints = Some(constraints);
        if crate::debug::is_enabled() {
            crate::logln!(
                "[HStackElement::layout] START: constraints=({:?}, {:?}) -> ({:?}, {:?})",
                constraints.min_width,
                constraints.min_height,
                constraints.max_width,
                constraints.max_height
            );
        }
        let child_count = self.children.len();
        let spacing_total = if child_count > 1 {
            (child_count - 1) as f32 * self.spacing
        } else {
            0.0
        };

        // First pass: measure fixed children (flex_factor == 0), count flex children
        let mut fixed_total_width: f32 = 0.0;
        let mut max_height: f32 = 0.0;
        let mut flex_total: u32 = 0;
        let mut child_sizes: Vec<Size> = Vec::with_capacity(self.children.len());

        // Collect flex factors and layout fixed children
        for child in self.children.iter_mut() {
            let flex = child.flex_factor();
            flex_total += flex;

            if flex == 0 {
                // Layout fixed child to measure its size
                let child_constraints =
                    LayoutConstraints::loose(constraints.max_width, constraints.max_height);
                let child_size = child.layout(child_constraints);
                child_sizes.push(child_size);
                fixed_total_width += child_size.width;
                max_height = max_height.max(child_size.height);
            } else {
                child_sizes.push(Size::ZERO);
            }
        }

        // Second pass: layout flex children and position all children
        let remaining_width = (constraints.max_width - fixed_total_width - spacing_total).max(0.0);
        let mut child_x_offset = 0.0;

        for (i, child) in self.children.iter_mut().enumerate() {
            let flex = child.flex_factor();
            let child_size = if flex == 0 {
                // Fixed child: use cached size
                child_sizes[i]
            } else {
                // Flex child: allocate remaining width
                let share = if flex_total > 0 {
                    remaining_width / flex_total as f32 * flex as f32
                } else {
                    remaining_width
                };
                // Pass tight constraints for main axis (width), allow available space for cross-axis (height)
                let child_constraints = LayoutConstraints {
                    min_width: share,
                    max_width: share,
                    min_height: 0.0,
                    max_height: constraints.max_height,
                };
                let child_size = child.layout(child_constraints);
                // Don't update max_height for flex children - they don't contribute to cross-axis size
                child_size
            };

            // Apply alignment on y-axis (cross-axis for HStack)
            let child_y = self.alignment.align_y(max_height, child_size.height);

            child.set_position(Point::new(child_x_offset, child_y));
            child_x_offset += child_size.width;
            // Add spacing after each child except the last
            if i < child_count.saturating_sub(1) {
                child_x_offset += self.spacing;
            }
        }

        // Calculate final size
        // Tight constraints (min == max && min > 0 && finite): Frame explicitly set size
        // Loose constraints: fit to content size (do NOT expand to max)
        if crate::debug::is_enabled() {
            crate::logln!(
                "[HStackElement::layout] child_x_offset={}, max_height={}",
                child_x_offset,
                max_height
            );
        }
        let final_height = if constraints.min_height == constraints.max_height
            && constraints.min_height.is_finite()
            && constraints.min_height > 0.0
        {
            if crate::debug::is_enabled() {
                crate::logln!(
                    "[HStackElement::layout] tight height detected, using constraint max_height"
                );
            }
            constraints.max_height // Frame指定サイズ
        } else if constraints.max_height.is_finite() {
            if crate::debug::is_enabled() {
                crate::logln!(
                    "[HStackElement::layout] loose height with finite max, using min(max_height, max_height)"
                );
            }
            max_height.min(constraints.max_height)
        } else {
            if crate::debug::is_enabled() {
                crate::logln!(
                    "[HStackElement::layout] loose height, using max_height from content"
                );
            }
            max_height // コンテンツサイズ
        };

        // Width calculation: use content width (sum of children), cap at max_width if needed
        let final_width = if constraints.min_width == constraints.max_width
            && constraints.min_width.is_finite()
            && constraints.min_width > 0.0
        {
            if crate::debug::is_enabled() {
                crate::logln!(
                    "[HStackElement::layout] tight width detected, using constraint max_width"
                );
            }
            constraints.max_width
        } else {
            if crate::debug::is_enabled() {
                crate::logln!(
                    "[HStackElement::layout] loose width, using child_x_offset from content"
                );
            }
            child_x_offset.min(constraints.max_width)
        };

        self.size = Size {
            width: final_width,
            height: final_height,
        };
        if crate::debug::is_enabled() {
            crate::logln!(
                "[HStackElement::layout] FINAL: size={}x{}",
                self.size.width,
                self.size.height
            );
        }
        self.size
    }

    fn last_layout_constraints(&self) -> Option<LayoutConstraints> {
        self.last_constraints
    }

    fn set_last_layout_constraints(&mut self, constraints: LayoutConstraints) {
        self.last_constraints = Some(constraints);
    }

    fn position(&self) -> Point {
        self.position
    }

    fn set_position(&mut self, position: Point) {
        self.position = position;
    }

    fn bounds(&self) -> Rect {
        Rect {
            origin: self.position,
            size: self.size,
        }
    }

    fn render(&mut self) {
        for child in &mut self.children {
            child.render();
        }
    }
}
