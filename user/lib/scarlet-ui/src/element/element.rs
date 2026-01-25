//! Element trait - the core runtime component abstraction
//!
//! Elements are the actual runtime objects that get mounted, laid out, and rendered.

use alloc::boxed::Box;
use core::any::Any;

use crate::geometry::{Point, Rect, Size};
use crate::view::View;

use super::id::ElementId;

/// Result of an Element update operation
///
/// Indicates whether the Element was updated, replaced, or unchanged
/// after attempting to reconcile with a new View.
pub enum UpdateResult {
    /// Properties were updated (Element reused)
    Updated,
    /// Element was replaced (new Element created)
    Replaced,
    /// No changes detected (Element unchanged)
    NoChange,
}

/// Layout constraints for Elements
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct LayoutConstraints {
    pub min_width: f32,
    pub max_width: f32,
    pub min_height: f32,
    pub max_height: f32,
}

impl LayoutConstraints {
    pub const fn new(
        min_width: f32,
        max_width: f32,
        min_height: f32,
        max_height: f32,
    ) -> Self {
        Self {
            min_width,
            max_width,
            min_height,
            max_height,
        }
    }

    /// Create tight constraints (exact size)
    pub const fn tight(width: f32, height: f32) -> Self {
        Self {
            min_width: width,
            max_width: width,
            min_height: height,
            max_height: height,
        }
    }

    /// Create unconstrained constraints (0 to infinity)
    pub const fn unconstrained() -> Self {
        Self {
            min_width: 0.0,
            max_width: f32::INFINITY,
            min_height: 0.0,
            max_height: f32::INFINITY,
        }
    }

    /// Create loose constraints (minimum to infinity)
    pub const fn loose(min_width: f32, min_height: f32) -> Self {
        Self {
            min_width,
            max_width: f32::INFINITY,
            min_height,
            max_height: f32::INFINITY,
        }
    }

    /// Check if width is constrained to a specific value
    pub fn is_tight_width(&self) -> bool {
        self.min_width == self.max_width
    }

    /// Check if height is constrained to a specific value
    pub fn is_tight_height(&self) -> bool {
        self.min_height == self.max_height
    }

    /// Check if both dimensions are tight
    pub fn is_tight(&self) -> bool {
        self.is_tight_width() && self.is_tight_height()
    }

    /// Constrain a size to fit within these constraints
    pub fn constrain(&self, size: Size) -> Size {
        Size {
            width: size.width.clamp(self.min_width, self.max_width),
            height: size.height.clamp(self.min_height, self.max_height),
        }
    }
}

/// Core Element trait - runtime objects in the element tree
///
/// Elements are mutable and handle layout, rendering, and event handling.
/// They are created by Views and managed by the ElementTree.
pub trait Element {
    /// Get the unique ID of this Element
    fn id(&self) -> ElementId;

    /// Get the type name of this Element for debugging
    fn type_name(&self) -> &str {
        "Element"
    }

    /// Get detailed type information for debugging
    ///
    /// Returns a String with detailed type information.
    /// Default implementation calls type_name(), but concrete types
    /// can override to provide more detailed information.
    fn type_name_debug(&self) -> alloc::string::String {
        alloc::string::String::from(self.type_name())
    }

    /// Get this Element as Any for downcasting
    fn as_any(&self) -> &dyn Any;

    /// Get this Element as Any mut for downcasting (mutable)
    fn as_any_mut(&mut self) -> &mut dyn Any;

    /// Get child Elements
    fn children(&self) -> &[Box<dyn Element>];

    /// Get mutable child Elements
    fn children_mut(&mut self) -> &mut [Box<dyn Element>];

    /// Update this Element from a new View
    ///
    /// Called when a View changes due to State updates.
    /// Returns UpdateResult indicating whether the Element was updated,
    /// replaced, or unchanged.
    fn update(&mut self, new_view: &dyn View) -> UpdateResult;

    /// Rebuild this Element from its stored View
    ///
    /// Called when the Element is marked dirty due to State changes.
    /// For ComponentElement, this recreates the child from the stored View.
    /// For RenderElement, this returns NoChange since properties are updated directly.
    ///
    /// Returns UpdateResult indicating whether the Element was rebuilt,
    /// replaced, or unchanged.
    fn rebuild(&mut self) -> UpdateResult;

    /// Mount the Element into the tree
    ///
    /// Called when the Element is first added to the tree.
    /// Subscribers should be set up here.
    fn mount(&mut self) {}

    /// Unmount the Element from the tree
    ///
    /// Called when the Element is removed from the tree.
    /// Clean up resources here.
    fn unmount(&mut self) {}

    /// Layout the Element and return its size
    ///
    /// parent_size is the available space from the parent.
    fn layout(&mut self, constraints: LayoutConstraints) -> Size;

    /// Get the current position of this Element
    fn position(&self) -> Point {
        Point::ZERO
    }

    /// Set the position of this Element
    fn set_position(&mut self, _position: Point) {}

    /// Get the current bounds of this Element
    fn bounds(&self) -> Rect {
        Rect {
            origin: self.position(),
            size: Size::ZERO, // Subclasses should override
        }
    }

    /// Hit test - check if a point is within this Element
    fn hit_test(&self, point: Point) -> bool {
        self.bounds().contains(point)
    }

    /// Render the Element to its buffer
    ///
    /// This is called during the paint phase to update the Element's buffer.
    /// For RenderElement, this calls render_object.render().
    /// For other Elements, this does nothing by default.
    fn render(&mut self) {}

    /// Get the buffer for this Element (if any)
    ///
    /// Returns the buffer if this Element has one to composite.
    /// For RenderElement, this delegates to render_object.get_buffer().
    /// For other Elements, returns None.
    fn get_buffer(&self) -> Option<&crate::buffer::Buffer> {
        None
    }

    /// Handle input events
    ///
    /// Returns true if the event was handled.
    ///
    /// The phase parameter indicates which phase of event dispatch is occurring:
    /// - Capture: Event is traveling from root to target
    /// - Target: Event is at the target element
    /// - Bubble: Event is traveling from target back to root
    fn handle_event(&mut self, _event: &crate::event::Event, _phase: crate::event::Phase) -> bool {
        false
    }

    /// Flex factor for space distribution in stacks
    ///
    /// - `0` (default): the element is laid out at its natural size
    /// - `>0`: the element participates in distributing remaining space in VStack/HStack
    fn flex_factor(&self) -> u32 {
        0
    }

    /// Get window information if this Element represents a Window
    ///
    /// Returns Some((app_id, title, size)) if this Element is a WindowElement,
    /// None otherwise. Default implementation returns None.
    fn get_window_info(&self) -> Option<(alloc::string::String, alloc::string::String, Size)> {
        None
    }
}


