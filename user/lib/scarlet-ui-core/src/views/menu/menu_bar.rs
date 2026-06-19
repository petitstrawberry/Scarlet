//! MenuBar - Container for menu items
//!
//! MenuBar displays menu items horizontally, similar to macOS menu bar.

use crate::element::{Element, ElementId, LayoutConstraints};
use crate::event::Event;
use crate::geometry::{Point, Rect, Size};
use crate::view::View;
use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;

/// MenuBar View - displays menu items horizontally
#[derive(Clone)]
pub struct MenuBar {
    items: Vec<MenuItem>,
    spacing: f32,
    on_hover_index: Option<Arc<dyn Fn(usize) + 'static>>,
}

impl MenuBar {
    /// Create a new MenuBar with the given items
    pub fn new(items: Vec<MenuItem>) -> Self {
        Self {
            items,
            spacing: 0.0, // No spacing for menu items (they touch)
            on_hover_index: None,
        }
    }

    /// Set the spacing between menu items
    pub fn spacing(mut self, spacing: f32) -> Self {
        self.spacing = spacing;
        self
    }

    /// Set hover callback with the hovered item index.
    pub fn on_hover_index(mut self, callback: impl Fn(usize) + 'static) -> Self {
        self.on_hover_index = Some(Arc::new(callback));
        self
    }

    /// Get the menu items
    pub fn items(&self) -> &[MenuItem] {
        &self.items
    }
}

impl View for MenuBar {
    fn create_element(&self) -> Box<dyn Element> {
        let elements: Vec<Box<dyn Element>> = self
            .items
            .iter()
            .map(|item| item.create_element())
            .collect();

        Box::new(MenuBarElement::new(
            elements,
            self.spacing,
            self.on_hover_index.clone(),
        ))
    }

    fn listenables(&self) -> Vec<&dyn crate::state::Listenable> {
        Vec::new()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

use super::menu_item::MenuItem;

/// MenuBarElement - handles horizontal layout of menu items
pub struct MenuBarElement {
    id: ElementId,
    children: Vec<Box<dyn Element>>,
    spacing: f32,
    position: Point,
    size: Size,
    on_hover_index: Option<Arc<dyn Fn(usize) + 'static>>,
    hovered_index: Option<usize>,
}

impl MenuBarElement {
    /// Create a new MenuBarElement
    pub fn new(
        children: Vec<Box<dyn Element>>,
        spacing: f32,
        on_hover_index: Option<Arc<dyn Fn(usize) + 'static>>,
    ) -> Self {
        Self {
            id: ElementId::generate(),
            children,
            spacing,
            position: Point::ZERO,
            size: Size::ZERO,
            on_hover_index,
            hovered_index: None,
        }
    }

    /// Get mutable reference to children
    pub fn children_mut(&mut self) -> &mut [Box<dyn Element>] {
        &mut self.children
    }
}

impl Element for MenuBarElement {
    fn id(&self) -> ElementId {
        self.id
    }

    fn type_name(&self) -> &str {
        "MenuBarElement"
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
        crate::element::UpdateResult::Replaced
    }

    fn rebuild(&mut self) -> crate::element::UpdateResult {
        crate::element::UpdateResult::NoChange
    }

    fn handle_event(&mut self, event: &crate::event::Event, _phase: crate::event::Phase) -> bool {
        let Event::Mouse(mouse_event) = event else {
            return false;
        };

        let point = match mouse_event {
            crate::event::MouseEvent::Moved { x, y }
            | crate::event::MouseEvent::Entered { x, y }
            | crate::event::MouseEvent::ButtonPressed { x, y, .. }
            | crate::event::MouseEvent::ButtonReleased { x, y, .. } => Point {
                x: *x as f32,
                y: *y as f32,
            },
            crate::event::MouseEvent::Exited { .. } | crate::event::MouseEvent::Wheel { .. } => {
                self.hovered_index = None;
                return false;
            }
        };

        let mut new_index = None;
        for (idx, child) in self.children.iter().enumerate() {
            let bounds = child.bounds();
            let rect = Rect {
                origin: bounds.origin,
                size: bounds.size,
            };
            if rect.contains(point) {
                new_index = Some(idx);
                break;
            }
        }

        if new_index != self.hovered_index {
            self.hovered_index = new_index;
            if let (Some(index), Some(callback)) = (new_index, self.on_hover_index.as_ref()) {
                callback(index);
            }
        }

        false
    }

    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        // Calculate total width and max height
        let mut total_width: f32 = 0.0;
        let mut max_height: f32 = 0.0;

        let target_height = if constraints.max_height.is_finite() && constraints.max_height > 0.0 {
            constraints.max_height
        } else {
            constraints.min_height
        };

        let child_count = self.children.len();

        for (i, child) in self.children.iter_mut().enumerate() {
            // Use infinite width constraint for horizontal layout
            let child_constraints = LayoutConstraints {
                min_width: 0.0,
                min_height: target_height,
                max_width: f32::INFINITY,
                max_height: if target_height > 0.0 {
                    target_height
                } else {
                    constraints.max_height
                },
            };

            let child_size = child.layout(child_constraints);
            total_width += child_size.width;
            max_height = max_height.max(child_size.height);

            // Add spacing (except after last item)
            if i < child_count - 1 {
                total_width += self.spacing;
            }
        }

        // Constrain to parent constraints
        let width = if constraints.max_width.is_finite() && constraints.max_width > 0.0 {
            total_width.min(constraints.max_width)
        } else {
            total_width
        };

        let height = if target_height > 0.0 {
            target_height
        } else {
            max_height
        };

        self.size = Size { width, height };

        // Position children
        let mut x = 0.0;
        for child in self.children.iter_mut() {
            child.set_position(Point::new(x, 0.0));
            x += child.bounds().size.width + self.spacing;
        }

        self.size
    }

    fn last_layout_constraints(&self) -> Option<LayoutConstraints> {
        None
    }

    fn set_last_layout_constraints(&mut self, _constraints: LayoutConstraints) {
        // Not implemented
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
        // Render all children
        for child in self.children.iter_mut() {
            child.render();
        }
    }
}
