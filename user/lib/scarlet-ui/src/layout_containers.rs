//! Layout containers - Flexbox-style one-dimensional layout
//!
//! This module provides layout containers that arrange their children
//! in a single dimension (vertical or horizontal), similar to SwiftUI's
//! VStack and HStack, or CSS Flexbox.

extern crate alloc;
use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::context::{ControlFlow, EventCtx, LayoutCtx, PaintCtx, UpdateCtx};
use crate::event::Event;
use crate::layout::{LayoutConstraints, Size, CrossAxisAlignment, MainAxisAlignment};
use crate::graphics::Rect;
use crate::view::id::ViewId;
use crate::view::render::RenderObject;
use crate::view::traits::Container;
use scarlet_std::fmt;

/// Vertical stack - arranges children vertically
///
/// VStack is a one-dimensional layout container that arranges its children
/// from top to bottom. It supports spacing between children and alignment
/// along the cross axis (horizontal).
///
/// # Example
///
/// ```ignore
/// let stack = VStack::new()
///     .spacing(10)
///     .alignment(CrossAxisAlignment::Center)
///     .child(button1)
///     .child(button2);
/// ```
pub struct VStack {
    /// Unique identifier for this view
    id: ViewId,
    /// Child views
    children: Vec<Box<dyn RenderObject>>,
    /// Spacing between children (in pixels)
    spacing: u32,
    /// How to align children along the cross axis (horizontal)
    alignment: CrossAxisAlignment,
    /// Cached size from last layout
    cached_size: Size,
    /// Cached frame from last layout
    cached_frame: Option<Rect>,
    /// Layout constraints from last layout
    constraints: Option<LayoutConstraints>,
    /// Cached child frames from last layout
    child_frames: Vec<Rect>,
}

impl VStack {
    /// Create a new empty VStack
    pub fn new() -> Self {
        Self {
            id: ViewId::new(),
            children: Vec::new(),
            spacing: 0,
            alignment: CrossAxisAlignment::Start,
            cached_size: Size::ZERO,
            cached_frame: None,
            constraints: None,
            child_frames: Vec::new(),
        }
    }

    /// Set the spacing between children
    pub fn spacing(mut self, spacing: u32) -> Self {
        self.spacing = spacing;
        self
    }

    /// Set the cross axis alignment
    pub fn alignment(mut self, alignment: CrossAxisAlignment) -> Self {
        self.alignment = alignment;
        self
    }

    /// Add a child view
    pub fn child<V: RenderObject + 'static>(mut self, child: V) -> Self {
        self.children.push(Box::new(child));
        self
    }

    /// Get the number of children
    pub fn child_count(&self) -> usize {
        self.children.len()
    }

    /// Get a reference to a child by index
    pub fn get_child(&self, index: usize) -> Option<&dyn RenderObject> {
        self.children.get(index).map(|b| b.as_ref())
    }
}

impl Default for VStack {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderObject for VStack {
    fn id(&self) -> ViewId {
        self.id
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn layout(&mut self, ctx: &mut LayoutCtx, constraints: LayoutConstraints) -> Size {
        self.constraints = Some(constraints);
        self.child_frames.clear();

        if self.children.is_empty() {
            self.cached_size = Size::new(
                constraints.min_width.max(0),
                constraints.min_height.max(0),
            );
            return self.cached_size;
        }

        let mut total_height = 0;
        let mut max_width = 0;
        let mut child_sizes = Vec::with_capacity(self.children.len());

        // First pass: measure each child
        for child in &mut self.children {
            let child_constraints = LayoutConstraints {
                min_width: constraints.min_width,
                max_width: constraints.max_width,
                min_height: 0,
                max_height: if constraints.max_height >= total_height {
                    constraints.max_height - total_height
                } else {
                    0
                },
            };

            let child_size = child.layout(ctx, child_constraints);
            total_height += child_size.height;
            max_width = max_width.max(child_size.width);
            child_sizes.push(child_size);
        }

        // Add spacing
        if self.children.len() > 1 {
            total_height += self.spacing * (self.children.len() - 1) as u32;
        }

        // Apply constraints
        let width = max_width.clamp(constraints.min_width, constraints.max_width);
        let height = total_height.clamp(constraints.min_height, constraints.max_height);

        self.cached_size = Size::new(width, height);

        // Calculate child frames (will be positioned relative to (0, 0) in draw)
        let mut y = 0;
        for child_size in child_sizes.iter() {
            let child_width = child_size.width;
            let x = match self.alignment {
                CrossAxisAlignment::Start => 0,
                CrossAxisAlignment::Center => (width as i32 - child_width as i32) / 2,
                CrossAxisAlignment::End => width as i32 - child_width as i32,
                CrossAxisAlignment::Stretch => 0, // Child will be stretched to width
            };

            self.child_frames.push(Rect::new(x, y, child_width, child_size.height));
            y += child_size.height as i32 + self.spacing as i32;
        }

        self.cached_size
    }

    fn draw(&self, _ctx: &mut PaintCtx, _frame: Rect) {
        // In a full implementation, we would:
        // 1. Get the ViewRegistry to access child views
        // 2. For each child, offset its frame by our frame position
        // 3. Call child.draw() with the offset frame

        // For now, child frames are stored in self.child_frames
        // The actual drawing will be done by the framework using the ViewRegistry
    }

    fn event(&mut self, _ctx: &mut EventCtx, _event: &Event) -> ControlFlow {
        // Propagate events to children
        // For now, just continue
        ControlFlow::Continue
    }

    fn update(&mut self, _ctx: &mut UpdateCtx) {
        // Propagate updates to children
    }
}

impl Container for VStack {
    fn add_child(&mut self, child: Box<dyn RenderObject>) {
        self.children.push(child);
    }

    fn clear_children(&mut self) {
        self.children.clear();
    }
}

/// Horizontal stack - arranges children horizontally
///
/// HStack is a one-dimensional layout container that arranges its children
/// from left to right. It supports spacing between children and alignment
/// along the cross axis (vertical).
///
/// # Example
///
/// ```ignore
/// let stack = HStack::new()
///     .spacing(10)
///     .alignment(CrossAxisAlignment::Center)
///     .child(label1)
///     .child(label2);
/// ```
pub struct HStack {
    /// Unique identifier for this view
    id: ViewId,
    /// Child views
    children: Vec<Box<dyn RenderObject>>,
    /// Spacing between children (in pixels)
    spacing: u32,
    /// How to align children along the cross axis (vertical)
    alignment: CrossAxisAlignment,
    /// Cached size from last layout
    cached_size: Size,
    /// Cached frame from last layout
    cached_frame: Option<Rect>,
    /// Layout constraints from last layout
    constraints: Option<LayoutConstraints>,
    /// Cached child frames from last layout
    child_frames: Vec<Rect>,
}

impl HStack {
    /// Create a new empty HStack
    pub fn new() -> Self {
        Self {
            id: ViewId::new(),
            children: Vec::new(),
            spacing: 0,
            alignment: CrossAxisAlignment::Start,
            cached_size: Size::ZERO,
            cached_frame: None,
            constraints: None,
            child_frames: Vec::new(),
        }
    }

    /// Set the spacing between children
    pub fn spacing(mut self, spacing: u32) -> Self {
        self.spacing = spacing;
        self
    }

    /// Set the cross axis alignment
    pub fn alignment(mut self, alignment: CrossAxisAlignment) -> Self {
        self.alignment = alignment;
        self
    }

    /// Add a child view
    pub fn child<V: RenderObject + 'static>(mut self, child: V) -> Self {
        self.children.push(Box::new(child));
        self
    }

    /// Get the number of children
    pub fn child_count(&self) -> usize {
        self.children.len()
    }

    /// Get a reference to a child by index
    pub fn get_child(&self, index: usize) -> Option<&dyn RenderObject> {
        self.children.get(index).map(|b| b.as_ref())
    }
}

impl Default for HStack {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderObject for HStack {
    fn id(&self) -> ViewId {
        self.id
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn layout(&mut self, ctx: &mut LayoutCtx, constraints: LayoutConstraints) -> Size {
        self.constraints = Some(constraints);
        self.child_frames.clear();

        if self.children.is_empty() {
            self.cached_size = Size::new(
                constraints.min_width.max(0),
                constraints.min_height.max(0),
            );
            return self.cached_size;
        }

        let mut total_width = 0;
        let mut max_height = 0;
        let mut child_sizes = Vec::with_capacity(self.children.len());

        // First pass: measure each child
        for child in &mut self.children {
            let child_constraints = LayoutConstraints {
                min_width: 0,
                max_width: if constraints.max_width >= total_width {
                    constraints.max_width - total_width
                } else {
                    0
                },
                min_height: constraints.min_height,
                max_height: constraints.max_height,
            };

            let child_size = child.layout(ctx, child_constraints);
            total_width += child_size.width;
            max_height = max_height.max(child_size.height);
            child_sizes.push(child_size);
        }

        // Add spacing
        if self.children.len() > 1 {
            total_width += self.spacing * (self.children.len() - 1) as u32;
        }

        // Apply constraints
        let width = total_width.clamp(constraints.min_width, constraints.max_width);
        let height = max_height.clamp(constraints.min_height, constraints.max_height);

        self.cached_size = Size::new(width, height);

        // Calculate child frames (will be positioned relative to (0, 0) in draw)
        let mut x = 0;
        for child_size in child_sizes.iter() {
            let child_height = child_size.height;
            let y = match self.alignment {
                CrossAxisAlignment::Start => 0,
                CrossAxisAlignment::Center => (height as i32 - child_height as i32) / 2,
                CrossAxisAlignment::End => height as i32 - child_height as i32,
                CrossAxisAlignment::Stretch => 0, // Child will be stretched to height
            };

            self.child_frames.push(Rect::new(x, y, child_size.width, child_height));
            x += child_size.width as i32 + self.spacing as i32;
        }

        self.cached_size
    }

    fn draw(&self, _ctx: &mut PaintCtx, _frame: Rect) {
        // In a full implementation, we would:
        // 1. Get the ViewRegistry to access child views
        // 2. For each child, offset its frame by our frame position
        // 3. Call child.draw() with the offset frame

        // For now, child frames are stored in self.child_frames
        // The actual drawing will be done by the framework using the ViewRegistry
    }

    fn event(&mut self, _ctx: &mut EventCtx, _event: &Event) -> ControlFlow {
        // Propagate events to children
        ControlFlow::Continue
    }

    fn update(&mut self, _ctx: &mut UpdateCtx) {
        // Propagate updates to children
    }
}

impl Container for HStack {
    fn add_child(&mut self, child: Box<dyn RenderObject>) {
        self.children.push(child);
    }

    fn clear_children(&mut self) {
        self.children.clear();
    }
}

/// Z-stack - layers children on top of each other
///
/// ZStack is a layout container that layers its children on top of each other,
/// aligned along both axes. All children occupy the same space.
///
/// # Example
///
/// ```ignore
/// let stack = ZStack::new()
///     .alignment_main(MainAxisAlignment::Center)
///     .alignment_cross(CrossAxisAlignment::Center)
///     .child(background)
///     .child(text);
/// ```
pub struct ZStack {
    /// Unique identifier for this view
    id: ViewId,
    /// Child views
    children: Vec<Box<dyn RenderObject>>,
    /// How to align children along the main axis
    alignment_main: MainAxisAlignment,
    /// How to align children along the cross axis
    alignment_cross: CrossAxisAlignment,
    /// Cached size from last layout
    cached_size: Size,
    /// Cached frame from last layout
    cached_frame: Option<Rect>,
    /// Layout constraints from last layout
    constraints: Option<LayoutConstraints>,
    /// Cached child frames from last layout
    child_frames: Vec<Rect>,
}

impl ZStack {
    /// Create a new empty ZStack
    pub fn new() -> Self {
        Self {
            id: ViewId::new(),
            children: Vec::new(),
            alignment_main: MainAxisAlignment::Start,
            alignment_cross: CrossAxisAlignment::Start,
            cached_size: Size::ZERO,
            cached_frame: None,
            constraints: None,
            child_frames: Vec::new(),
        }
    }

    /// Set the main axis alignment
    pub fn alignment_main(mut self, alignment: MainAxisAlignment) -> Self {
        self.alignment_main = alignment;
        self
    }

    /// Set the cross axis alignment
    pub fn alignment_cross(mut self, alignment: CrossAxisAlignment) -> Self {
        self.alignment_cross = alignment;
        self
    }

    /// Add a child view
    pub fn child<V: RenderObject + 'static>(mut self, child: V) -> Self {
        self.children.push(Box::new(child));
        self
    }

    /// Get the number of children
    pub fn child_count(&self) -> usize {
        self.children.len()
    }

    /// Get a reference to a child by index
    pub fn get_child(&self, index: usize) -> Option<&dyn RenderObject> {
        self.children.get(index).map(|b| b.as_ref())
    }
}

impl Default for ZStack {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderObject for ZStack {
    fn id(&self) -> ViewId {
        self.id
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn layout(&mut self, ctx: &mut LayoutCtx, constraints: LayoutConstraints) -> Size {
        self.constraints = Some(constraints);
        self.child_frames.clear();

        if self.children.is_empty() {
            self.cached_size = Size::new(
                constraints.min_width.max(0),
                constraints.min_height.max(0),
            );
            return self.cached_size;
        }

        let mut max_width = 0;
        let mut max_height = 0;
        let mut child_sizes = Vec::with_capacity(self.children.len());

        // Measure all children and find the maximum size
        for child in &mut self.children {
            let child_size = child.layout(ctx, constraints);
            max_width = max_width.max(child_size.width);
            max_height = max_height.max(child_size.height);
            child_sizes.push(child_size);
        }

        // Apply constraints
        let width = max_width.clamp(constraints.min_width, constraints.max_width);
        let height = max_height.clamp(constraints.min_height, constraints.max_height);

        self.cached_size = Size::new(width, height);

        // Calculate child frames (all children are positioned based on alignment)
        for child_size in child_sizes.iter() {
            // Calculate position based on main axis alignment
            let x = match self.alignment_main {
                MainAxisAlignment::Start => 0,
                MainAxisAlignment::Center => (width as i32 - child_size.width as i32) / 2,
                MainAxisAlignment::End => width as i32 - child_size.width as i32,
                MainAxisAlignment::SpaceBetween => 0, // Not applicable for ZStack
                MainAxisAlignment::SpaceAround => 0, // Not applicable for ZStack
                MainAxisAlignment::SpaceEvenly => 0, // Not applicable for ZStack
            };

            // Calculate position based on cross axis alignment
            let y = match self.alignment_cross {
                CrossAxisAlignment::Start => 0,
                CrossAxisAlignment::Center => (height as i32 - child_size.height as i32) / 2,
                CrossAxisAlignment::End => height as i32 - child_size.height as i32,
                CrossAxisAlignment::Stretch => 0, // Child will be stretched to height
            };

            self.child_frames.push(Rect::new(x, y, child_size.width, child_size.height));
        }

        self.cached_size
    }

    fn draw(&self, _ctx: &mut PaintCtx, _frame: Rect) {

        // In a full implementation, we would:
        // 1. Get the ViewRegistry to access child views
        // 2. For each child, offset its frame by our frame position
        // 3. Call child.draw() with the offset frame
        // Drawing order matters - last child is on top

        // For now, child frames are stored in self.child_frames
        // The actual drawing will be done by the framework using the ViewRegistry
    }

    fn event(&mut self, _ctx: &mut EventCtx, _event: &Event) -> ControlFlow {
        // Propagate events to children (top-most first)
        ControlFlow::Continue
    }

    fn update(&mut self, _ctx: &mut UpdateCtx) {
        // Propagate updates to children
    }
}

impl ZStack {
    fn calculate_child_position(&self, frame: Rect, _child_id: ViewId) -> (i32, i32) {
        // Get child's cached size
        let child_size = Size::ZERO; // In practice, get from ViewRegistry

        // Calculate position based on main axis alignment
        let x = match self.alignment_main {
            MainAxisAlignment::Start => frame.x,
            MainAxisAlignment::Center => frame.x + (frame.width as i32 - child_size.width as i32) / 2,
            MainAxisAlignment::End => frame.x + frame.width as i32 - child_size.width as i32,
            MainAxisAlignment::SpaceBetween => frame.x, // Not applicable for single child
            MainAxisAlignment::SpaceAround => frame.x, // Not applicable for single child
            MainAxisAlignment::SpaceEvenly => frame.x, // Not applicable for single child
        };

        // Calculate position based on cross axis alignment
        let y = match self.alignment_cross {
            CrossAxisAlignment::Start => frame.y,
            CrossAxisAlignment::Center => frame.y + (frame.height as i32 - child_size.height as i32) / 2,
            CrossAxisAlignment::End => frame.y + frame.height as i32 - child_size.height as i32,
            CrossAxisAlignment::Stretch => frame.y, // Child will be stretched to height
        };

        (x, y)
    }
}

impl Container for ZStack {
    fn add_child(&mut self, child: Box<dyn RenderObject>) {
        self.children.push(child);
    }

    fn clear_children(&mut self) {
        self.children.clear();
    }
}

impl fmt::Debug for VStack {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VStack")
            .field("id", &self.id)
            .field("child_count", &self.children.len())
            .field("spacing", &self.spacing)
            .field("alignment", &self.alignment)
            .finish()
    }
}

impl fmt::Debug for HStack {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HStack")
            .field("id", &self.id)
            .field("child_count", &self.children.len())
            .field("spacing", &self.spacing)
            .field("alignment", &self.alignment)
            .finish()
    }
}

impl fmt::Debug for ZStack {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ZStack")
            .field("id", &self.id)
            .field("child_count", &self.children.len())
            .field("alignment_main", &self.alignment_main)
            .field("alignment_cross", &self.alignment_cross)
            .finish()
    }
}

/// Spacer - Expands to fill available space
///
/// Spacer is a special View that takes up as much space as possible
/// in the layout direction of its container. In HStack, it expands
/// horizontally; in VStack, it expands vertically.
///
/// # Example
///
/// ```ignore
/// HStack::new()
///     .child(Text::new("Left"))
///     .child(Spacer::new())  // Pushes everything to the right
///     .child(Text::new("Right"))
/// ```
pub struct Spacer {
    id: ViewId,
}

impl Spacer {
    pub fn new() -> Self {
        Self {
            id: ViewId::new(),
        }
    }
}

impl Default for Spacer {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderObject for Spacer {
    fn id(&self) -> ViewId {
        self.id
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn layout(&mut self, _ctx: &mut LayoutCtx, constraints: LayoutConstraints) -> Size {
        // Spacer takes all available space
        Size::new(constraints.max_width, constraints.max_height)
    }

    fn draw(&self, _ctx: &mut PaintCtx, _frame: Rect) {
        // Spacer doesn't draw anything
    }

    fn event(&mut self, _ctx: &mut EventCtx, _event: &Event) -> ControlFlow {
        ControlFlow::Continue
    }

    fn update(&mut self, _ctx: &mut UpdateCtx) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestView {
        id: ViewId,
        size: Size,
    }

    impl TestView {
        fn new(width: u32, height: u32) -> Self {
            Self {
                id: ViewId::new(),
                size: Size::new(width, height),
            }
        }
    }

    impl View for TestView {
        fn id(&self) -> ViewId {
            self.id
        }

        fn layout(&mut self, _ctx: &mut LayoutCtx, _constraints: LayoutConstraints) -> Size {
            self.size
        }

        fn draw(&self, _ctx: &mut PaintCtx, _frame: Rect) {}

        fn event(&mut self, _ctx: &mut EventCtx, _event: &Event) -> ControlFlow {
            ControlFlow::Continue
        }

        fn update(&mut self, _ctx: &mut UpdateCtx) {}
    }

    #[test]
    fn test_vstack_empty() {
        let mut stack = VStack::new();
        let ctx = LayoutCtx::new(stack.id());
        let constraints = LayoutConstraints::new(0, 100, 0, 100);

        let size = stack.layout(&mut ctx, constraints);
        assert_eq!(size, Size::new(0, 0));
    }

    #[test]
    fn test_vstack_with_children() {
        let mut stack = VStack::new()
            .child(Box::new(TestView::new(50, 20)))
            .child(Box::new(TestView::new(60, 30)));

        let ctx = LayoutCtx::new(stack.id());
        let constraints = LayoutConstraints::new(0, 100, 0, 100);

        let size = stack.layout(&mut ctx, constraints);
        assert_eq!(size.width, 60); // max child width
        assert_eq!(size.height, 50); // sum of heights
    }

    #[test]
    fn test_vstack_with_spacing() {
        let mut stack = VStack::new()
            .spacing(10)
            .child(Box::new(TestView::new(50, 20)))
            .child(Box::new(TestView::new(60, 30)));

        let ctx = LayoutCtx::new(stack.id());
        let constraints = LayoutConstraints::new(0, 100, 0, 100);

        let size = stack.layout(&mut ctx, constraints);
        assert_eq!(size.width, 60);
        assert_eq!(size.height, 60); // 20 + 10 + 30
    }

    #[test]
    fn test_hstack_with_children() {
        let mut stack = HStack::new()
            .child(Box::new(TestView::new(50, 20)))
            .child(Box::new(TestView::new(60, 30)));

        let ctx = LayoutCtx::new(stack.id());
        let constraints = LayoutConstraints::new(0, 100, 0, 100);

        let size = stack.layout(&mut ctx, constraints);
        assert_eq!(size.width, 110); // sum of widths
        assert_eq!(size.height, 30); // max child height
    }

    #[test]
    fn test_zstack_with_children() {
        let mut stack = ZStack::new()
            .child(Box::new(TestView::new(50, 20)))
            .child(Box::new(TestView::new(60, 30)));

        let ctx = LayoutCtx::new(stack.id());
        let constraints = LayoutConstraints::new(0, 100, 0, 100);

        let size = stack.layout(&mut ctx, constraints);
        assert_eq!(size.width, 60); // max child width
        assert_eq!(size.height, 30); // max child height
    }
}
