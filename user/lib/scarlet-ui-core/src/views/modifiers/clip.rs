//! Clip View Modifier
//!
//! Clips child content to a rounded rectangle.

use crate::element::LayoutConstraints;
use crate::element::{Element, ElementRenderObject, RenderElement};
use crate::geometry::{Point, Size};
use crate::view::View;
use alloc::boxed::Box;
use alloc::vec;
use core::any::Any;

/// Clip view modifier - clips child content to a rounded rect.
#[derive(Clone)]
pub struct Clip<V: View> {
    inner: V,
    radius: f32,
}

impl<V: View> Clip<V> {
    pub fn new(inner: V, radius: f32) -> Self {
        Self { inner, radius }
    }

    pub fn radius(&self) -> f32 {
        self.radius
    }
}

impl<V: View + Clone> View for Clip<V> {
    fn create_element(&self) -> Box<dyn Element> {
        Box::new(RenderElement::with_children(
            self.clone(),
            ClipRenderObject::new(self.radius),
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

/// Clip RenderObject
pub struct ClipRenderObject {
    radius: f32,
    size: Size,
}

impl ClipRenderObject {
    pub fn new(radius: f32) -> Self {
        Self {
            radius,
            size: Size::ZERO,
        }
    }

    pub fn radius(&self) -> f32 {
        self.radius
    }

    fn point_in_rounded_rect(&self, point: Point) -> bool {
        let size = self.size;
        if point.x < 0.0 || point.y < 0.0 || point.x >= size.width || point.y >= size.height {
            return false;
        }
        let max_radius = size.width.min(size.height) / 2.0;
        let radius = self.radius.max(0.0).min(max_radius);
        if radius <= 0.0 {
            return true;
        }

        let left = radius;
        let right = size.width - radius;
        let top = radius;
        let bottom = size.height - radius;

        if (point.x >= left && point.x < right) || (point.y >= top && point.y < bottom) {
            return true;
        }

        let corner_x = if point.x < left { left } else { right };
        let corner_y = if point.y < top { top } else { bottom };
        let dx = point.x - corner_x;
        let dy = point.y - corner_y;
        (dx * dx + dy * dy) <= radius * radius
    }
}

impl ElementRenderObject for ClipRenderObject {
    fn layout(&mut self, _constraints: LayoutConstraints) -> Size {
        self.size
    }

    fn layout_with_children(
        &mut self,
        constraints: LayoutConstraints,
        children: &mut [Box<dyn Element>],
    ) -> Size {
        if let Some(child) = children.first_mut() {
            let size = child.layout(constraints);
            child.set_position(Point::ZERO);
            self.size = size;
            size
        } else {
            self.size = Size::ZERO;
            Size::ZERO
        }
    }

    fn size(&self) -> Size {
        self.size
    }

    fn hit_test(&self, point: Point) -> bool {
        self.point_in_rounded_rect(point)
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
