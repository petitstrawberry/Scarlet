//! Container views for layout

use super::traits::{View, ViewBox, Size};
use crate::graphics::{Canvas, Rect};
use crate::event::Event;
use scarlet_std::boxed::Box;
use scarlet_std::vec::Vec;

/// Vertical stack - arranges children top to bottom
pub struct VStack {
    children: Vec<(ViewBox, Size)>,
    spacing: u32,
}

impl VStack {
    pub fn new() -> Self {
        Self {
            children: Vec::new(),
            spacing: 8,
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
    
    /// Calculate child frame at index
    fn child_frame(&self, index: usize, base: Rect) -> Rect {
        let mut y = base.y;
        for i in 0..index {
            y += self.children[i].1.height as i32 + self.spacing as i32;
        }
        let size = self.children[index].1;
        Rect::new(base.x, y, size.width, size.height)
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
                let child_size = child.layout(Size::new(available.width, share));
                *cached_size = child_size;
                max_width = max_width.max(child_size.width);
            }
        }

        let mut total_height = spacing_total;
        for (_, cached_size) in &self.children {
            total_height = total_height.saturating_add(cached_size.height);
        }

        Size::new(max_width.min(available.width), total_height.min(available.height))
    }

    fn draw(&self, canvas: &mut Canvas, frame: Rect) {
        let mut y = frame.y;

        for (child, cached_size) in &self.children {
            let child_frame = Rect::new(frame.x, y, cached_size.width, cached_size.height);
            child.draw(canvas, child_frame);
            y += cached_size.height as i32 + self.spacing as i32;
        }
    }

    fn on_event(&mut self, event: &mut Event, frame: Rect) -> bool {
        let mut y = frame.y;

        for (child, cached_size) in &mut self.children {
            let child_frame = Rect::new(frame.x, y, cached_size.width, cached_size.height);
            if child_frame.contains(event.x(), event.y()) {
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
        for (child, cached_size) in &self.children {
            let child_frame = Rect::new(0, y, cached_size.width, cached_size.height);
            result.push((child.as_ref() as &dyn View, child_frame));
            y += cached_size.height as i32 + self.spacing as i32;
        }
        result
    }
    
    fn children_mut(&mut self) -> Vec<(&mut dyn View, Rect)> {
        let mut result = Vec::new();
        let mut y = 0i32;
        let spacing = self.spacing;
        for (child, cached_size) in &mut self.children {
            let child_frame = Rect::new(0, y, cached_size.width, cached_size.height);
            result.push((child.as_mut() as &mut dyn View, child_frame));
            y += cached_size.height as i32 + spacing as i32;
        }
        result
    }

    fn visit_children(&self, visitor: &mut dyn FnMut(&dyn View, Rect) -> bool) {
        let mut y = 0i32;
        for (child, cached_size) in &self.children {
            let child_frame = Rect::new(0, y, cached_size.width, cached_size.height);
            if visitor(child.as_ref() as &dyn View, child_frame) {
                break;
            }
            y += cached_size.height as i32 + self.spacing as i32;
        }
    }

    fn visit_children_mut(&mut self, visitor: &mut dyn FnMut(&mut dyn View, Rect) -> bool) {
        let mut y = 0i32;
        let spacing = self.spacing;
        for (child, cached_size) in &mut self.children {
            let child_frame = Rect::new(0, y, cached_size.width, cached_size.height);
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
}

impl HStack {
    pub fn new() -> Self {
        Self {
            children: Vec::new(),
            spacing: 8,
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

                let child_size = child.layout(Size::new(share, available.height));
                *cached_size = child_size;
                max_height = max_height.max(child_size.height);
            }
        }

        let mut total_width = spacing_total;
        for (_, cached_size) in &self.children {
            total_width = total_width.saturating_add(cached_size.width);
        }

        Size::new(total_width.min(available.width), max_height.min(available.height))
    }

    fn draw(&self, canvas: &mut Canvas, frame: Rect) {
        let mut x = frame.x;

        for (child, cached_size) in &self.children {
            let child_frame = Rect::new(x, frame.y, cached_size.width, cached_size.height);
            child.draw(canvas, child_frame);
            x += cached_size.width as i32 + self.spacing as i32;
        }
    }

    fn on_event(&mut self, event: &mut Event, frame: Rect) -> bool {
        let mut x = frame.x;

        for (child, cached_size) in &mut self.children {
            let child_frame = Rect::new(x, frame.y, cached_size.width, cached_size.height);
            if child_frame.contains(event.x(), event.y()) {
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
        for (child, cached_size) in &self.children {
            let child_frame = Rect::new(x, 0, cached_size.width, cached_size.height);
            result.push((child.as_ref() as &dyn View, child_frame));
            x += cached_size.width as i32 + self.spacing as i32;
        }
        result
    }
    
    fn children_mut(&mut self) -> Vec<(&mut dyn View, Rect)> {
        let mut result = Vec::new();
        let mut x = 0i32;
        let spacing = self.spacing;
        for (child, cached_size) in &mut self.children {
            let child_frame = Rect::new(x, 0, cached_size.width, cached_size.height);
            result.push((child.as_mut() as &mut dyn View, child_frame));
            x += cached_size.width as i32 + spacing as i32;
        }
        result
    }

    fn visit_children(&self, visitor: &mut dyn FnMut(&dyn View, Rect) -> bool) {
        let mut x = 0i32;
        for (child, cached_size) in &self.children {
            let child_frame = Rect::new(x, 0, cached_size.width, cached_size.height);
            if visitor(child.as_ref() as &dyn View, child_frame) {
                break;
            }
            x += cached_size.width as i32 + self.spacing as i32;
        }
    }

    fn visit_children_mut(&mut self, visitor: &mut dyn FnMut(&mut dyn View, Rect) -> bool) {
        let mut x = 0i32;
        let spacing = self.spacing;
        for (child, cached_size) in &mut self.children {
            let child_frame = Rect::new(x, 0, cached_size.width, cached_size.height);
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
