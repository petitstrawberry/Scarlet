//! VStackElement - Vertical stack layout element
//!
//! Arranges children in a vertical column with spacing and alignment.

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::any::Any;

use crate::element::{Element, ElementId, LayoutConstraints};
use crate::geometry::{Alignment, Point, Rect, Size};

/// VStackElement - arranges children vertically
pub struct VStackElement {
    id: ElementId,
    children: Vec<Box<dyn Element>>,
    spacing: f32,
    alignment: Alignment,
    position: Point,
    size: Size,
    last_constraints: Option<LayoutConstraints>,
}

impl VStackElement {
    /// Create a new VStackElement
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

impl Element for VStackElement {
    fn id(&self) -> ElementId {
        self.id
    }

    fn type_name(&self) -> &str {
        "VStackElement"
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
        // VStack doesn't support view-based updates
        crate::element::UpdateResult::Replaced
    }

    fn rebuild(&mut self) -> crate::element::UpdateResult {
        crate::element::UpdateResult::NoChange
    }

    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        self.last_constraints = Some(constraints);
        if crate::debug::is_enabled() {
            crate::logln!(
                "[VStackElement::layout] START: constraints=({:?}, {:?}) -> ({:?}, {:?})",
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

        // First pass: measure all children to calculate total height and max width
        let mut total_height: f32 = 0.0;
        let mut max_width: f32 = 0.0;
        let mut flex_total: u32 = 0;
        let mut child_sizes: Vec<Size> = Vec::with_capacity(self.children.len());

        for child in self.children.iter_mut() {
            let flex = child.flex_factor();
            flex_total += flex;

            // Layout all children with loose constraints to get their sizes
            let child_constraints =
                LayoutConstraints::loose(constraints.max_width, constraints.max_height);
            let child_size = child.layout(child_constraints);
            child_sizes.push(child_size);
            total_height += child_size.height;
            max_width = max_width.max(child_size.width);
        }

        // Add spacing to total height
        total_height += spacing_total;

        crate::logln!(
            "[VStackElement::layout] after measuring: total_height={}, max_width={}",
            total_height,
            max_width
        );

        // VStack size calculation
        // Tight constraints (min == max && min > 0 && finite): Frame explicitly set size
        // Loose constraints: fit to content size (do NOT expand to max)
        let final_width = if constraints.min_width == constraints.max_width
            && constraints.min_width.is_finite()
            && constraints.min_width > 0.0
        {
            crate::logln!(
                "[VStackElement::layout] tight width detected, using constraint max_width"
            );
            constraints.max_width // Frame指定サイズ
        } else {
            crate::logln!("[VStackElement::layout] loose width, using max_width from content");
            max_width // コンテンツサイズ（拡大しない）
        };
        let final_height = if constraints.min_height == constraints.max_height
            && constraints.min_height.is_finite()
            && constraints.min_height > 0.0
        {
            crate::logln!(
                "[VStackElement::layout] tight height detected, using constraint max_height"
            );
            constraints.max_height // Frame指定サイズ
        } else if constraints.max_height.is_finite() {
            crate::logln!(
                "[VStackElement::layout] loose height with finite max, using min(total_height, max_height)"
            );
            total_height.min(constraints.max_height)
        } else {
            crate::logln!("[VStackElement::layout] loose height with inf max, using total_height");
            total_height
        };

        // Second pass: position all children
        let mut child_y_offset = 0.0;

        for (i, child) in self.children.iter_mut().enumerate() {
            let child_size = child_sizes[i];

            // Apply alignment on x-axis (cross-axis for VStack)
            let child_x = self.alignment.align_x(final_width, child_size.width);

            child.set_position(Point::new(child_x, child_y_offset));
            child_y_offset += child_size.height;
            // Add spacing after each child except the last
            if i < child_count.saturating_sub(1) {
                child_y_offset += self.spacing;
            }
        }

        self.size = Size {
            width: final_width,
            height: final_height,
        };
        crate::logln!(
            "[VStackElement::layout] FINAL: size={}x{}",
            final_width,
            final_height
        );
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
