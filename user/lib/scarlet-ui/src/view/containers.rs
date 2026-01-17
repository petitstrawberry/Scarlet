//! Container views for layout

use super::traits::{View, ViewBox, Size};
use crate::graphics::{Canvas, Rect};
use crate::event::{Event, EventKind};
use scarlet_std::boxed::Box;
use scarlet_std::vec::Vec;
use scarlet_std::println;

/// Cross-axis alignment for stacks.
///
/// - `Start`: leading/top
/// - `Center`: centered (SwiftUI default)
/// - `End`: trailing/bottom
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StackAlignment {
    Start,
    Center,
    End,
}

/// Vertical stack - arranges children top to bottom
pub struct VStack {
    children: Vec<(ViewBox, Size)>,
    spacing: u32,
    alignment: StackAlignment,
    cached_size: Size,
}

impl VStack {
    pub fn new() -> Self {
        Self {
            children: Vec::new(),
            spacing: 8,
            alignment: StackAlignment::Center,
            cached_size: Size::ZERO,
        }
    }

    /// Add a child view
    pub fn child<V: View + 'static>(mut self, view: V) -> Self {
        self.children.push((Box::new(view), Size::ZERO));
        self
    }

    /// Set spacing between children
    pub fn spacing(mut self, spacing: u32) -> Self {
        self.spacing = spacing;
        self
    }

    /// Set cross-axis alignment (horizontal).
    pub fn alignment(mut self, alignment: StackAlignment) -> Self {
        self.alignment = alignment;
        self
    }
}

impl Default for VStack {
    fn default() -> Self {
        Self::new()
    }
}

impl View for VStack {
    fn layout(&mut self, available: Size) -> Size {
        let child_count = self.children.len();
        let spacing_total = if child_count > 1 {
            self.spacing * (child_count as u32 - 1)
        } else {
            0
        };

        let mut fixed_total_height = 0u32;
        let mut max_width = 0u32;
        let mut flex_total = 0u32;

        // First pass: measure fixed children, count flex.
        for (child, cached_size) in &mut self.children {
            let flex = child.flex_factor();
            if flex == 0 {
                let child_size = child.layout(available);
                *cached_size = child_size;
                fixed_total_height = fixed_total_height.saturating_add(child_size.height);
                max_width = max_width.max(child_size.width);
            } else {
                flex_total = flex_total.saturating_add(flex);
                *cached_size = Size::ZERO;
            }
        }

        let remaining_height = available
            .height
            .saturating_sub(fixed_total_height.saturating_add(spacing_total));

        // Second pass: distribute remaining height to flex children.
        if flex_total > 0 {
            let mut remainder = remaining_height % flex_total;
            for (child, cached_size) in &mut self.children {
                let flex = child.flex_factor();
                if flex == 0 {
                    continue;
                }

                let mut share = remaining_height / flex_total;
                share = share.saturating_mul(flex);
                if remainder > 0 {
                    share = share.saturating_add(1);
                    remainder = remainder.saturating_sub(1);
                }

                // Give the child its allocated share along the main axis.
                // Pass 0 for cross-axis so Spacer doesn't expand width.
                let child_size = child.layout(Size::new(0, share));
                *cached_size = child_size;
                max_width = max_width.max(child_size.width);
            }
        }

        let mut total_height = spacing_total;
        for (_, cached_size) in &self.children {
            total_height = total_height.saturating_add(cached_size.height);
        }

        let out = Size::new(max_width.min(available.width), total_height.min(available.height));
        self.cached_size = out;
        out
    }

    fn draw(&self, canvas: &mut Canvas, frame: Rect) {
        let mut y = frame.y;

        for (child, cached_size) in &self.children {
            let extra = frame.width.saturating_sub(cached_size.width) as i32;
            let dx = match self.alignment {
                StackAlignment::Start => 0,
                StackAlignment::Center => extra / 2,
                StackAlignment::End => extra,
            };
            let child_frame = Rect::new(frame.x + dx, y, cached_size.width, cached_size.height);
            child.draw(canvas, child_frame);
            y += cached_size.height as i32 + self.spacing as i32;
        }
    }

    fn on_event(&mut self, event: &mut Event, frame: Rect) -> bool {
        let mut y = frame.y;

        for (i, (child, cached_size)) in self.children.iter_mut().enumerate() {
            let extra = frame.width.saturating_sub(cached_size.width) as i32;
            let dx = match self.alignment {
                StackAlignment::Start => 0,
                StackAlignment::Center => extra / 2,
                StackAlignment::End => extra,
            };
            let child_frame = Rect::new(frame.x + dx, y, cached_size.width, cached_size.height);
            if child_frame.contains(event.x(), event.y()) {
                // Only log non-MouseMove events
                // if !matches!(event.kind, EventKind::MouseMove) {
                //     println!("[VStack] dispatching to child {} (event={:?})", i, event.kind);
                // }
                if child.on_event(event, child_frame) {
                    return true;
                }
            }
            y += cached_size.height as i32 + self.spacing as i32;
        }

        false
    }
    
    fn children(&self) -> Vec<(&dyn View, Rect)> {
        let mut result = Vec::new();
        let mut y = 0i32;
        let base_w = self.cached_size.width;
        for (child, cached_size) in &self.children {
            let extra = base_w.saturating_sub(cached_size.width) as i32;
            let dx = match self.alignment {
                StackAlignment::Start => 0,
                StackAlignment::Center => extra / 2,
                StackAlignment::End => extra,
            };
            let child_frame = Rect::new(dx, y, cached_size.width, cached_size.height);
            result.push((child.as_ref() as &dyn View, child_frame));
            y += cached_size.height as i32 + self.spacing as i32;
        }
        result
    }
    
    fn children_mut(&mut self) -> Vec<(&mut dyn View, Rect)> {
        let mut result = Vec::new();
        let mut y = 0i32;
        let spacing = self.spacing;
        let base_w = self.cached_size.width;
        for (child, cached_size) in &mut self.children {
            let extra = base_w.saturating_sub(cached_size.width) as i32;
            let dx = match self.alignment {
                StackAlignment::Start => 0,
                StackAlignment::Center => extra / 2,
                StackAlignment::End => extra,
            };
            let child_frame = Rect::new(dx, y, cached_size.width, cached_size.height);
            result.push((child.as_mut() as &mut dyn View, child_frame));
            y += cached_size.height as i32 + spacing as i32;
        }
        result
    }

    fn visit_children(&self, visitor: &mut dyn FnMut(&dyn View, Rect) -> bool) {
        let mut y = 0i32;
        let base_w = self.cached_size.width;
        for (child, cached_size) in &self.children {
            let extra = base_w.saturating_sub(cached_size.width) as i32;
            let dx = match self.alignment {
                StackAlignment::Start => 0,
                StackAlignment::Center => extra / 2,
                StackAlignment::End => extra,
            };
            let child_frame = Rect::new(dx, y, cached_size.width, cached_size.height);
            if visitor(child.as_ref() as &dyn View, child_frame) {
                break;
            }
            y += cached_size.height as i32 + self.spacing as i32;
        }
    }

    fn visit_children_mut(&mut self, visitor: &mut dyn FnMut(&mut dyn View, Rect) -> bool) {
        let mut y = 0i32;
        let spacing = self.spacing;
        let base_w = self.cached_size.width;
        for (child, cached_size) in &mut self.children {
            let extra = base_w.saturating_sub(cached_size.width) as i32;
            let dx = match self.alignment {
                StackAlignment::Start => 0,
                StackAlignment::Center => extra / 2,
                StackAlignment::End => extra,
            };
            let child_frame = Rect::new(dx, y, cached_size.width, cached_size.height);
            if visitor(child.as_mut() as &mut dyn View, child_frame) {
                break;
            }
            y += cached_size.height as i32 + spacing as i32;
        }
    }
}

/// Horizontal stack - arranges children left to right
pub struct HStack {
    children: Vec<(ViewBox, Size)>,
    spacing: u32,
    alignment: StackAlignment,
    cached_size: Size,
}

impl HStack {
    pub fn new() -> Self {
        Self {
            children: Vec::new(),
            spacing: 8,
            alignment: StackAlignment::Center,
            cached_size: Size::ZERO,
        }
    }

    /// Add a child view
    pub fn child<V: View + 'static>(mut self, view: V) -> Self {
        self.children.push((Box::new(view), Size::ZERO));
        self
    }

    /// Set spacing between children
    pub fn spacing(mut self, spacing: u32) -> Self {
        self.spacing = spacing;
        self
    }

    /// Set cross-axis alignment (vertical).
    pub fn alignment(mut self, alignment: StackAlignment) -> Self {
        self.alignment = alignment;
        self
    }
}

impl Default for HStack {
    fn default() -> Self {
        Self::new()
    }
}

impl View for HStack {
    fn layout(&mut self, available: Size) -> Size {
        let child_count = self.children.len();
        let spacing_total = if child_count > 1 {
            self.spacing * (child_count as u32 - 1)
        } else {
            0
        };

        let mut fixed_total_width = 0u32;
        let mut max_height = 0u32;
        let mut flex_total = 0u32;

        // First pass: measure fixed children, count flex.
        for (child, cached_size) in &mut self.children {
            let flex = child.flex_factor();
            if flex == 0 {
                let child_size = child.layout(available);
                *cached_size = child_size;
                fixed_total_width = fixed_total_width.saturating_add(child_size.width);
                max_height = max_height.max(child_size.height);
            } else {
                flex_total = flex_total.saturating_add(flex);
                *cached_size = Size::ZERO;
            }
        }

        let remaining_width = available
            .width
            .saturating_sub(fixed_total_width.saturating_add(spacing_total));

        // Second pass: distribute remaining width to flex children.
        if flex_total > 0 {
            let mut remainder = remaining_width % flex_total;
            for (child, cached_size) in &mut self.children {
                let flex = child.flex_factor();
                if flex == 0 {
                    continue;
                }

                let mut share = remaining_width / flex_total;
                share = share.saturating_mul(flex);
                if remainder > 0 {
                    share = share.saturating_add(1);
                    remainder = remainder.saturating_sub(1);
                }

                // Pass 0 for cross-axis so Spacer doesn't expand height.
                let child_size = child.layout(Size::new(share, 0));
                *cached_size = child_size;
                max_height = max_height.max(child_size.height);
            }
        }

        let mut total_width = spacing_total;
        for (_, cached_size) in &self.children {
            total_width = total_width.saturating_add(cached_size.width);
        }

        let out = Size::new(total_width.min(available.width), max_height.min(available.height));
        self.cached_size = out;
        out
    }

    fn draw(&self, canvas: &mut Canvas, frame: Rect) {
        let mut x = frame.x;

        for (child, cached_size) in &self.children {
            let extra = frame.height.saturating_sub(cached_size.height) as i32;
            let dy = match self.alignment {
                StackAlignment::Start => 0,
                StackAlignment::Center => extra / 2,
                StackAlignment::End => extra,
            };
            let child_frame = Rect::new(x, frame.y + dy, cached_size.width, cached_size.height);
            child.draw(canvas, child_frame);
            x += cached_size.width as i32 + self.spacing as i32;
        }
    }

    fn on_event(&mut self, event: &mut Event, frame: Rect) -> bool {
        let mut x = frame.x;

        for (i, (child, cached_size)) in self.children.iter_mut().enumerate() {
            let extra = frame.height.saturating_sub(cached_size.height) as i32;
            let dy = match self.alignment {
                StackAlignment::Start => 0,
                StackAlignment::Center => extra / 2,
                StackAlignment::End => extra,
            };
            let child_frame = Rect::new(x, frame.y + dy, cached_size.width, cached_size.height);
            if child_frame.contains(event.x(), event.y()) {
                // Only log non-MouseMove events
                // if !matches!(event.kind, EventKind::MouseMove) {
                //     println!("[HStack] dispatching to child {} (event={:?})", i, event.kind);
                // }
                if child.on_event(event, child_frame) {
                    return true;
                }
            }
            x += cached_size.width as i32 + self.spacing as i32;
        }

        false
    }
    
    fn children(&self) -> Vec<(&dyn View, Rect)> {
        let mut result = Vec::new();
        let mut x = 0i32;
        let base_h = self.cached_size.height;
        for (child, cached_size) in &self.children {
            let extra = base_h.saturating_sub(cached_size.height) as i32;
            let dy = match self.alignment {
                StackAlignment::Start => 0,
                StackAlignment::Center => extra / 2,
                StackAlignment::End => extra,
            };
            let child_frame = Rect::new(x, dy, cached_size.width, cached_size.height);
            result.push((child.as_ref() as &dyn View, child_frame));
            x += cached_size.width as i32 + self.spacing as i32;
        }
        result
    }
    
    fn children_mut(&mut self) -> Vec<(&mut dyn View, Rect)> {
        let mut result = Vec::new();
        let mut x = 0i32;
        let spacing = self.spacing;
        let base_h = self.cached_size.height;
        for (child, cached_size) in &mut self.children {
            let extra = base_h.saturating_sub(cached_size.height) as i32;
            let dy = match self.alignment {
                StackAlignment::Start => 0,
                StackAlignment::Center => extra / 2,
                StackAlignment::End => extra,
            };
            let child_frame = Rect::new(x, dy, cached_size.width, cached_size.height);
            result.push((child.as_mut() as &mut dyn View, child_frame));
            x += cached_size.width as i32 + spacing as i32;
        }
        result
    }

    fn visit_children(&self, visitor: &mut dyn FnMut(&dyn View, Rect) -> bool) {
        let mut x = 0i32;
        let base_h = self.cached_size.height;
        for (child, cached_size) in &self.children {
            let extra = base_h.saturating_sub(cached_size.height) as i32;
            let dy = match self.alignment {
                StackAlignment::Start => 0,
                StackAlignment::Center => extra / 2,
                StackAlignment::End => extra,
            };
            let child_frame = Rect::new(x, dy, cached_size.width, cached_size.height);
            if visitor(child.as_ref() as &dyn View, child_frame) {
                break;
            }
            x += cached_size.width as i32 + self.spacing as i32;
        }
    }

    fn visit_children_mut(&mut self, visitor: &mut dyn FnMut(&mut dyn View, Rect) -> bool) {
        let mut x = 0i32;
        let spacing = self.spacing;
        let base_h = self.cached_size.height;
        for (child, cached_size) in &mut self.children {
            let extra = base_h.saturating_sub(cached_size.height) as i32;
            let dy = match self.alignment {
                StackAlignment::Start => 0,
                StackAlignment::Center => extra / 2,
                StackAlignment::End => extra,
            };
            let child_frame = Rect::new(x, dy, cached_size.width, cached_size.height);
            if visitor(child.as_mut() as &mut dyn View, child_frame) {
                break;
            }
            x += cached_size.width as i32 + spacing as i32;
        }
    }
}

/// ZStack - overlays children on top of each other
pub struct ZStack {
    children: Vec<(ViewBox, Size)>,
}

impl ZStack {
    pub fn new() -> Self {
        Self {
            children: Vec::new(),
        }
    }

    /// Add a child view (later children are drawn on top)
    pub fn child<V: View + 'static>(mut self, view: V) -> Self {
        self.children.push((Box::new(view), Size::ZERO));
        self
    }
}

impl Default for ZStack {
    fn default() -> Self {
        Self::new()
    }
}

impl View for ZStack {
    fn layout(&mut self, available: Size) -> Size {
        let mut max_size = Size::ZERO;

        for (child, cached_size) in &mut self.children {
            let child_size = child.layout(available);
            *cached_size = child_size;
            max_size.width = max_size.width.max(child_size.width);
            max_size.height = max_size.height.max(child_size.height);
        }

        max_size
    }

    fn draw(&self, canvas: &mut Canvas, frame: Rect) {
        for (child, cached_size) in &self.children {
            // Center each child in the frame
            let x = frame.x + (frame.width as i32 - cached_size.width as i32) / 2;
            let y = frame.y + (frame.height as i32 - cached_size.height as i32) / 2;
            let child_frame = Rect::new(x, y, cached_size.width, cached_size.height);
            child.draw(canvas, child_frame);
        }
    }

    fn on_event(&mut self, event: &mut Event, frame: Rect) -> bool {
        // Handle events in reverse order (top to bottom)
        for (child, cached_size) in self.children.iter_mut().rev() {
            let x = frame.x + (frame.width as i32 - cached_size.width as i32) / 2;
            let y = frame.y + (frame.height as i32 - cached_size.height as i32) / 2;
            let child_frame = Rect::new(x, y, cached_size.width, cached_size.height);
            if child_frame.contains(event.x(), event.y()) {
                if child.on_event(event, child_frame) {
                    return true;
                }
            }
        }
        false
    }
}

/// Padding wrapper - adds space around a child
pub struct Padding {
    child: ViewBox,
    top: u32,
    right: u32,
    bottom: u32,
    left: u32,
    cached_size: Size,
}

impl Padding {
    pub fn new<V: View + 'static>(child: V) -> Self {
        Self {
            child: Box::new(child),
            top: 0,
            right: 0,
            bottom: 0,
            left: 0,
            cached_size: Size::ZERO,
        }
    }

    /// Set uniform padding on all sides
    pub fn all(mut self, padding: u32) -> Self {
        self.top = padding;
        self.right = padding;
        self.bottom = padding;
        self.left = padding;
        self
    }

    /// Set horizontal padding
    pub fn horizontal(mut self, padding: u32) -> Self {
        self.left = padding;
        self.right = padding;
        self
    }

    /// Set vertical padding
    pub fn vertical(mut self, padding: u32) -> Self {
        self.top = padding;
        self.bottom = padding;
        self
    }
}

impl View for Padding {
    fn layout(&mut self, available: Size) -> Size {
        let inner_available = Size::new(
            available.width.saturating_sub(self.left + self.right),
            available.height.saturating_sub(self.top + self.bottom),
        );

        let child_size = self.child.layout(inner_available);
        self.cached_size = child_size;

        Size::new(
            child_size.width + self.left + self.right,
            child_size.height + self.top + self.bottom,
        )
    }

    fn draw(&self, canvas: &mut Canvas, frame: Rect) {
        let child_frame = Rect::new(
            frame.x + self.left as i32,
            frame.y + self.top as i32,
            self.cached_size.width,
            self.cached_size.height,
        );
        self.child.draw(canvas, child_frame);
    }

    fn on_event(&mut self, event: &mut Event, frame: Rect) -> bool {
        let child_frame = Rect::new(
            frame.x + self.left as i32,
            frame.y + self.top as i32,
            self.cached_size.width,
            self.cached_size.height,
        );
        if child_frame.contains(event.x(), event.y()) {
            self.child.on_event(event, child_frame)
        } else {
            false
        }
    }
    
    fn children(&self) -> Vec<(&dyn View, Rect)> {
        let child_frame = Rect::new(
            self.left as i32,
            self.top as i32,
            self.cached_size.width,
            self.cached_size.height,
        );
        let mut v = Vec::new();
        v.push((self.child.as_ref() as &dyn View, child_frame));
        v
    }
    
    fn children_mut(&mut self) -> Vec<(&mut dyn View, Rect)> {
        let child_frame = Rect::new(
            self.left as i32,
            self.top as i32,
            self.cached_size.width,
            self.cached_size.height,
        );
        let mut v = Vec::new();
        v.push((self.child.as_mut() as &mut dyn View, child_frame));
        v
    }

    fn visit_children(&self, visitor: &mut dyn FnMut(&dyn View, Rect) -> bool) {
        let child_frame = Rect::new(
            self.left as i32,
            self.top as i32,
            self.cached_size.width,
            self.cached_size.height,
        );
        let _ = visitor(self.child.as_ref() as &dyn View, child_frame);
    }

    fn visit_children_mut(&mut self, visitor: &mut dyn FnMut(&mut dyn View, Rect) -> bool) {
        let child_frame = Rect::new(
            self.left as i32,
            self.top as i32,
            self.cached_size.width,
            self.cached_size.height,
        );
        let _ = visitor(self.child.as_mut() as &mut dyn View, child_frame);
    }
}

/// Center wrapper - centers child in available space
pub struct Center {
    child: ViewBox,
    cached_size: Size,
}

impl Center {
    pub fn new<V: View + 'static>(child: V) -> Self {
        Self {
            child: Box::new(child),
            cached_size: Size::ZERO,
        }
    }
}

impl View for Center {
    fn layout(&mut self, available: Size) -> Size {
        self.cached_size = self.child.layout(available);
        self.cached_size
    }

    fn draw(&self, canvas: &mut Canvas, frame: Rect) {
        let x = frame.x + (frame.width as i32 - self.cached_size.width as i32) / 2;
        let y = frame.y + (frame.height as i32 - self.cached_size.height as i32) / 2;
        let child_frame = Rect::new(x, y, self.cached_size.width, self.cached_size.height);
        self.child.draw(canvas, child_frame);
    }

    fn on_event(&mut self, event: &mut Event, frame: Rect) -> bool {
        let x = frame.x + (frame.width as i32 - self.cached_size.width as i32) / 2;
        let y = frame.y + (frame.height as i32 - self.cached_size.height as i32) / 2;
        let child_frame = Rect::new(x, y, self.cached_size.width, self.cached_size.height);
        if child_frame.contains(event.x(), event.y()) {
            self.child.on_event(event, child_frame)
        } else {
            false
        }
    }
    
    fn children(&self) -> Vec<(&dyn View, Rect)> {
        let child_frame = Rect::new(0, 0, self.cached_size.width, self.cached_size.height);
        let mut v = Vec::new();
        v.push((self.child.as_ref() as &dyn View, child_frame));
        v
    }
    
    fn children_mut(&mut self) -> Vec<(&mut dyn View, Rect)> {
        let child_frame = Rect::new(0, 0, self.cached_size.width, self.cached_size.height);
        let mut v = Vec::new();
        v.push((self.child.as_mut() as &mut dyn View, child_frame));
        v
    }

    fn visit_children(&self, visitor: &mut dyn FnMut(&dyn View, Rect) -> bool) {
        let child_frame = Rect::new(0, 0, self.cached_size.width, self.cached_size.height);
        let _ = visitor(self.child.as_ref() as &dyn View, child_frame);
    }

    fn visit_children_mut(&mut self, visitor: &mut dyn FnMut(&mut dyn View, Rect) -> bool) {
        let child_frame = Rect::new(0, 0, self.cached_size.width, self.cached_size.height);
        let _ = visitor(self.child.as_mut() as &mut dyn View, child_frame);
    }
}

/// ScrollView - scrollable container for content larger than available space
///
/// Provides both vertical and horizontal scrolling with optional scrollbars.
/// Supports mouse wheel scrolling and drag-to-scroll on scrollbars.
pub struct ScrollView {
    child: ViewBox,
    scroll_offset_x: i32,
    scroll_offset_y: i32,
    cached_size: Size,
    child_size: Size,
    shows_vertical_scrollbar: bool,
    shows_horizontal_scrollbar: bool,
    scrollbar_width: u32,
    scrollbar_color: crate::Color,
    scrollbar_track_color: crate::Color,
    needs_redraw: bool,
    // Drag state for scrollbar thumb dragging
    dragging_vertical_thumb: bool,
    dragging_horizontal_thumb: bool,
    drag_start_y: i32,
    drag_start_x: i32,
    drag_start_scroll_y: i32,
    drag_start_scroll_x: i32,
    // Scroll speed multiplier for mouse wheel
    wheel_scroll_speed: i32,
    // Cached scrollbar calculations
    cached_vertical_thumb: Option<(u32, i32)>,
    cached_max_scroll_y: i32,
    cached_vertical_track_height: u32,
    cached_horizontal_thumb: Option<(u32, i32)>,
    cached_max_scroll_x: i32,
    cached_horizontal_track_width: u32,
    cached_content_width: u32,
    cached_content_height: u32,
    cache_valid: bool,
}

impl ScrollView {
    /// Create a new scroll view
    pub fn new<V: View + 'static>(child: V) -> Self {
        Self {
            child: Box::new(child),
            scroll_offset_x: 0,
            scroll_offset_y: 0,
            cached_size: Size::ZERO,
            child_size: Size::ZERO,
            shows_vertical_scrollbar: true,
            shows_horizontal_scrollbar: false,
            scrollbar_width: 12,
            scrollbar_color: crate::Color::rgb(140, 140, 140),
            scrollbar_track_color: crate::Color::rgb(70, 70, 70),
            needs_redraw: true,
            dragging_vertical_thumb: false,
            dragging_horizontal_thumb: false,
            drag_start_y: 0,
            drag_start_x: 0,
            drag_start_scroll_y: 0,
            drag_start_scroll_x: 0,
            wheel_scroll_speed: 30,
            cached_vertical_thumb: None,
            cached_max_scroll_y: 0,
            cached_vertical_track_height: 0,
            cached_horizontal_thumb: None,
            cached_max_scroll_x: 0,
            cached_horizontal_track_width: 0,
            cached_content_width: 0,
            cached_content_height: 0,
            cache_valid: false,
        }
    }

    /// Set whether to show vertical scrollbar
    pub fn shows_vertical_scrollbar(mut self, shows: bool) -> Self {
        self.shows_vertical_scrollbar = shows;
        self
    }

    /// Set whether to show horizontal scrollbar
    pub fn shows_horizontal_scrollbar(mut self, shows: bool) -> Self {
        self.shows_horizontal_scrollbar = shows;
        self
    }

    /// Set scrollbar width (applies to both scrollbars)
    pub fn scrollbar_width(mut self, width: u32) -> Self {
        self.scrollbar_width = width;
        self
    }

    /// Set scrollbar color (applies to both scrollbars)
    pub fn scrollbar_color(mut self, color: crate::Color) -> Self {
        self.scrollbar_color = color;
        self
    }

    /// Set scrollbar track color (applies to both scrollbars)
    pub fn scrollbar_track_color(mut self, color: crate::Color) -> Self {
        self.scrollbar_track_color = color;
        self
    }

    /// Set mouse wheel scroll speed (pixels per wheel tick)
    pub fn wheel_scroll_speed(mut self, speed: i32) -> Self {
        self.wheel_scroll_speed = speed.abs().max(1);
        self
    }

    /// Invalidate cached scrollbar calculations
    fn invalidate_cache(&mut self) {
        self.cache_valid = false;
    }

    /// Recalculate cached scrollbar values
    fn recalculate_cache(&mut self) {
        if self.cache_valid {
            return;
        }

        // Calculate max scroll values
        self.cached_max_scroll_y = (self.child_size.height as i32 - self.cached_size.height as i32).max(0);
        self.cached_max_scroll_x = (self.child_size.width as i32 - self.cached_size.width as i32).max(0);

        // Calculate content dimensions (subtract scrollbar space if visible)
        self.cached_content_width = if self.shows_vertical_scrollbar
            && self.child_size.height > self.cached_size.height {
            self.cached_size.width.saturating_sub(self.scrollbar_width)
        } else {
            self.cached_size.width
        };

        self.cached_content_height = if self.shows_horizontal_scrollbar
            && self.child_size.width > self.cached_size.width {
            self.cached_size.height.saturating_sub(self.scrollbar_width)
        } else {
            self.cached_size.height
        };

        // Calculate vertical scrollbar thumb
        self.cached_vertical_thumb = if self.shows_vertical_scrollbar
            && self.child_size.height > self.cached_size.height {

            let track_height = self.cached_content_height;
            let thumb_height = ((self.cached_size.height as f32 / self.child_size.height as f32)
                * track_height as f32) as u32;
            let thumb_height = thumb_height.max(20); // Minimum thumb size

            let thumb_y = if self.cached_max_scroll_y > 0 {
                ((self.scroll_offset_y as f32 / self.cached_max_scroll_y as f32)
                    * (track_height - thumb_height) as f32) as i32
            } else {
                0
            };

            self.cached_vertical_track_height = track_height;
            Some((thumb_height, thumb_y))
        } else {
            self.cached_vertical_track_height = self.cached_content_height;
            None
        };

        // Calculate horizontal scrollbar thumb
        self.cached_horizontal_thumb = if self.shows_horizontal_scrollbar
            && self.child_size.width > self.cached_size.width {

            let track_width = self.cached_content_width;
            let thumb_width = ((self.cached_size.width as f32 / self.child_size.width as f32)
                * track_width as f32) as u32;
            let thumb_width = thumb_width.max(20); // Minimum thumb size

            let thumb_x = if self.cached_max_scroll_x > 0 {
                ((self.scroll_offset_x as f32 / self.cached_max_scroll_x as f32)
                    * (track_width - thumb_width) as f32) as i32
            } else {
                0
            };

            self.cached_horizontal_track_width = track_width;
            Some((thumb_width, thumb_x))
        } else {
            self.cached_horizontal_track_width = self.cached_content_width;
            None
        };

        self.cache_valid = true;
    }

    /// Get current vertical scroll offset
    pub fn scroll_offset_y(&self) -> i32 {
        self.scroll_offset_y
    }

    /// Get current horizontal scroll offset
    pub fn scroll_offset_x(&self) -> i32 {
        self.scroll_offset_x
    }

    /// Set vertical scroll offset (clamped to valid range)
    pub fn set_scroll_offset_y(&mut self, offset: i32) {
        self.recalculate_cache();
        let new_offset = offset.clamp(0, self.cached_max_scroll_y);
        if new_offset != self.scroll_offset_y {
            self.scroll_offset_y = new_offset;
            self.needs_redraw = true;
            self.invalidate_cache();
        }
    }

    /// Set horizontal scroll offset (clamped to valid range)
    pub fn set_scroll_offset_x(&mut self, offset: i32) {
        self.recalculate_cache();
        let new_offset = offset.clamp(0, self.cached_max_scroll_x);
        if new_offset != self.scroll_offset_x {
            self.scroll_offset_x = new_offset;
            self.needs_redraw = true;
            self.invalidate_cache();
        }
    }

    /// Scroll vertically by delta (positive = down, negative = up)
    pub fn scroll_by_y(&mut self, delta: i32) {
        self.set_scroll_offset_y(self.scroll_offset_y + delta);
    }

    /// Scroll horizontally by delta (positive = right, negative = left)
    pub fn scroll_by_x(&mut self, delta: i32) {
        self.set_scroll_offset_x(self.scroll_offset_x + delta);
    }

    /// Get vertical scrollbar thumb size and position (cached)
    fn vertical_scrollbar_thumb(&self) -> Option<(u32, i32)> {
        self.cached_vertical_thumb
    }

    /// Get horizontal scrollbar thumb size and position (cached)
    fn horizontal_scrollbar_thumb(&self) -> Option<(u32, i32)> {
        self.cached_horizontal_thumb
    }

    /// Check if a point is on the vertical scrollbar thumb
    fn is_on_vertical_thumb(&self, frame: Rect, y: i32) -> bool {
        if let Some((thumb_height, thumb_y)) = self.vertical_scrollbar_thumb() {
            let scrollbar_x = frame.x + frame.width as i32 - self.scrollbar_width as i32;
            let thumb_rect = crate::graphics::Rect::new(
                scrollbar_x,
                frame.y + thumb_y,
                self.scrollbar_width,
                thumb_height,
            );
            thumb_rect.contains(frame.x + frame.width as i32 - self.scrollbar_width as i32 / 2, y)
        } else {
            false
        }
    }

    /// Check if a point is on the horizontal scrollbar thumb
    fn is_on_horizontal_thumb(&self, frame: Rect, x: i32) -> bool {
        if let Some((thumb_width, thumb_x)) = self.horizontal_scrollbar_thumb() {
            let scrollbar_y = frame.y + frame.height as i32 - self.scrollbar_width as i32;
            let thumb_rect = crate::graphics::Rect::new(
                frame.x + thumb_x,
                scrollbar_y,
                thumb_width,
                self.scrollbar_width,
            );
            thumb_rect.contains(x, scrollbar_y + self.scrollbar_width as i32 / 2)
        } else {
            false
        }
    }

    /// Check if a point is on the vertical scrollbar track
    fn is_on_vertical_track(&self, frame: Rect, x: i32, _y: i32) -> bool {
        if !self.shows_vertical_scrollbar {
            return false;
        }
        let scrollbar_x = frame.x + frame.width as i32 - self.scrollbar_width as i32;
        x >= scrollbar_x && x < scrollbar_x + self.scrollbar_width as i32
    }

    /// Check if a point is on the horizontal scrollbar track
    fn is_on_horizontal_track(&self, frame: Rect, _x: i32, y: i32) -> bool {
        if !self.shows_horizontal_scrollbar {
            return false;
        }
        let scrollbar_y = frame.y + frame.height as i32 - self.scrollbar_width as i32;
        y >= scrollbar_y && y < scrollbar_y + self.scrollbar_width as i32
    }
}

impl View for ScrollView {
    fn layout(&mut self, available: Size) -> Size {
        // Layout child with unconstrained size to measure its natural size
        let child_available = Size::new(u32::MAX, u32::MAX);
        self.child_size = self.child.layout(child_available);

        // ScrollView takes all available space
        // Content can be larger (scrollable) or smaller (centered)
        self.cached_size = available;

        // Invalidate cache since sizes changed
        self.invalidate_cache();

        available
    }

    fn flex_factor(&self) -> u32 {
        1 // ScrollView should expand to fill available space
    }

    fn draw(&self, canvas: &mut Canvas, frame: Rect) {
        // Note: Clipping not yet available in Canvas, draw without clipping
        // TODO: Add clipping support to Canvas for proper scroll behavior

        // Draw child with scroll offset
        let child_frame = Rect::new(
            frame.x - self.scroll_offset_x,
            frame.y - self.scroll_offset_y,
            self.child_size.width,
            self.child_size.height,
        );
        self.child.draw(canvas, child_frame);

        // Draw vertical scrollbar if needed
        if let Some((thumb_height, thumb_y)) = self.cached_vertical_thumb {
            let scrollbar_x = frame.x + frame.width as i32 - self.scrollbar_width as i32;
            let scrollbar_y = frame.y;
            let scrollbar_height = self.cached_content_height;

            // Draw scrollbar track
            canvas.fill_rect(
                scrollbar_x,
                scrollbar_y,
                self.scrollbar_width,
                scrollbar_height,
                self.scrollbar_track_color,
            );

            // Draw scrollbar thumb
            canvas.fill_rect(
                scrollbar_x,
                scrollbar_y + thumb_y,
                self.scrollbar_width,
                thumb_height,
                self.scrollbar_color,
            );
        }

        // Draw horizontal scrollbar if needed
        if let Some((thumb_width, thumb_x)) = self.cached_horizontal_thumb {
            let scrollbar_x = frame.x;
            let scrollbar_y = frame.y + frame.height as i32 - self.scrollbar_width as i32;
            let scrollbar_width = self.cached_content_width;

            // Draw scrollbar track
            canvas.fill_rect(
                scrollbar_x,
                scrollbar_y,
                scrollbar_width,
                self.scrollbar_width,
                self.scrollbar_track_color,
            );

            // Draw scrollbar thumb
            canvas.fill_rect(
                scrollbar_x + thumb_x,
                scrollbar_y,
                thumb_width,
                self.scrollbar_width,
                self.scrollbar_color,
            );
        }
    }

    fn on_event(&mut self, event: &mut Event, frame: Rect) -> bool {
        let x = event.x();
        let y = event.y();

        match event.kind {
            crate::event::EventKind::MouseWheel { delta_x, delta_y } => {
                // Handle mouse wheel scrolling
                let handled = if delta_y != 0 && self.child_size.height > self.cached_size.height {
                    let scroll_delta = (delta_y * self.wheel_scroll_speed / 120).abs();
                    // Positive delta_y means scrolling down (wheel moved away from user)
                    if delta_y > 0 {
                        self.scroll_by_y(scroll_delta);
                    } else {
                        self.scroll_by_y(-scroll_delta);
                    }
                    true
                } else if delta_x != 0 && self.child_size.width > self.cached_size.width {
                    let scroll_delta = (delta_x * self.wheel_scroll_speed / 120).abs();
                    if delta_x > 0 {
                        self.scroll_by_x(scroll_delta);
                    } else {
                        self.scroll_by_x(-scroll_delta);
                    }
                    true
                } else {
                    false
                };

                if handled {
                    event.stop_propagation();
                }
                handled
            }

            crate::event::EventKind::MouseDown { button: crate::event::MouseButton::Left } => {
                // Check if clicking on vertical scrollbar thumb
                if self.is_on_vertical_thumb(frame, y) {
                    self.dragging_vertical_thumb = true;
                    self.drag_start_y = y;
                    self.drag_start_scroll_y = self.scroll_offset_y;
                    event.stop_propagation();
                    return true;
                }

                // Check if clicking on horizontal scrollbar thumb
                if self.is_on_horizontal_thumb(frame, x) {
                    self.dragging_horizontal_thumb = true;
                    self.drag_start_x = x;
                    self.drag_start_scroll_x = self.scroll_offset_x;
                    event.stop_propagation();
                    return true;
                }

                // Check if clicking on vertical scrollbar track (jump to position)
                if self.is_on_vertical_track(frame, x, y) {
                    if let Some((thumb_height, _)) = self.cached_vertical_thumb {
                        let click_y = y - frame.y;

                        if self.cached_max_scroll_y > 0 {
                            // Calculate new scroll position based on click location
                            let new_scroll = ((click_y - thumb_height as i32 / 2) as f32
                                / (self.cached_vertical_track_height - thumb_height) as f32
                                * self.cached_max_scroll_y as f32) as i32;
                            self.set_scroll_offset_y(new_scroll);
                            event.stop_propagation();
                            return true;
                        }
                    }
                }

                // Check if clicking on horizontal scrollbar track (jump to position)
                if self.is_on_horizontal_track(frame, x, y) {
                    if let Some((thumb_width, _)) = self.cached_horizontal_thumb {
                        let click_x = x - frame.x;

                        if self.cached_max_scroll_x > 0 {
                            let new_scroll = ((click_x - thumb_width as i32 / 2) as f32
                                / (self.cached_horizontal_track_width - thumb_width) as f32
                                * self.cached_max_scroll_x as f32) as i32;
                            self.set_scroll_offset_x(new_scroll);
                            event.stop_propagation();
                            return true;
                        }
                    }
                }

                // Forward to child with adjusted coordinates
                let mut adjusted_event = crate::event::Event::new(
                    event.kind,
                    crate::graphics::Point::new(x + self.scroll_offset_x, y + self.scroll_offset_y),
                );
                self.child.on_event(&mut adjusted_event, frame)
            }

            crate::event::EventKind::MouseUp { button: crate::event::MouseButton::Left } => {
                // End dragging
                if self.dragging_vertical_thumb || self.dragging_horizontal_thumb {
                    self.dragging_vertical_thumb = false;
                    self.dragging_horizontal_thumb = false;
                    event.stop_propagation();
                    return true;
                }

                // Forward to child with adjusted coordinates
                let mut adjusted_event = crate::event::Event::new(
                    event.kind,
                    crate::graphics::Point::new(x + self.scroll_offset_x, y + self.scroll_offset_y),
                );
                self.child.on_event(&mut adjusted_event, frame)
            }

            crate::event::EventKind::MouseMove => {
                // Handle dragging
                if self.dragging_vertical_thumb {
                    let delta_y = y - self.drag_start_y;
                    if let Some((thumb_height, _)) = self.cached_vertical_thumb {
                        if self.cached_max_scroll_y > 0
                            && self.cached_vertical_track_height > thumb_height {
                            let scroll_delta = (delta_y as f32
                                / (self.cached_vertical_track_height - thumb_height) as f32
                                * self.cached_max_scroll_y as f32) as i32;
                            self.set_scroll_offset_y(self.drag_start_scroll_y + scroll_delta);
                            event.stop_propagation();
                            return true;
                        }
                    }
                }

                if self.dragging_horizontal_thumb {
                    let delta_x = x - self.drag_start_x;
                    if let Some((thumb_width, _)) = self.cached_horizontal_thumb {
                        if self.cached_max_scroll_x > 0
                            && self.cached_horizontal_track_width > thumb_width {
                            let scroll_delta = (delta_x as f32
                                / (self.cached_horizontal_track_width - thumb_width) as f32
                                * self.cached_max_scroll_x as f32) as i32;
                            self.set_scroll_offset_x(self.drag_start_scroll_x + scroll_delta);
                            event.stop_propagation();
                            return true;
                        }
                    }
                }

                // Forward to child with adjusted coordinates
                let mut adjusted_event = crate::event::Event::new(
                    event.kind,
                    crate::graphics::Point::new(x + self.scroll_offset_x, y + self.scroll_offset_y),
                );
                self.child.on_event(&mut adjusted_event, frame)
            }

            _ => {
                // Forward other events to child with adjusted coordinates
                let mut adjusted_event = crate::event::Event::new(
                    event.kind,
                    crate::graphics::Point::new(x + self.scroll_offset_x, y + self.scroll_offset_y),
                );
                self.child.on_event(&mut adjusted_event, frame)
            }
        }
    }

    fn children(&self) -> Vec<(&dyn View, Rect)> {
        let mut v = Vec::new();
        v.push((self.child.as_ref() as &dyn View, Rect::new(0, 0, self.child_size.width, self.child_size.height)));
        v
    }

    fn children_mut(&mut self) -> Vec<(&mut dyn View, Rect)> {
        let mut v = Vec::new();
        v.push((self.child.as_mut() as &mut dyn View, Rect::new(0, 0, self.child_size.width, self.child_size.height)));
        v
    }

    fn visit_children(&self, visitor: &mut dyn FnMut(&dyn View, Rect) -> bool) {
        let child_frame = Rect::new(0, 0, self.child_size.width, self.child_size.height);
        let _ = visitor(self.child.as_ref() as &dyn View, child_frame);
    }

    fn visit_children_mut(&mut self, visitor: &mut dyn FnMut(&mut dyn View, Rect) -> bool) {
        let child_frame = Rect::new(0, 0, self.child_size.width, self.child_size.height);
        let _ = visitor(self.child.as_mut() as &mut dyn View, child_frame);
    }

    fn needs_draw(&self) -> bool {
        self.needs_redraw || self.child.needs_draw()
    }

    fn set_needs_draw(&mut self) {
        self.needs_redraw = true;
    }

    fn clear_needs_draw(&mut self) {
        self.needs_redraw = false;
        self.child.clear_needs_draw();
    }
}

