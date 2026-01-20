//! Window container with optional titlebar and background

use crate::buffer::Buffer;
use crate::dirty::DirtyFlags;
use crate::event::{Event, EventContext, EventPhase, HitResult, MouseEventKind};
use crate::geometry::{Point, Rect, Size};
use crate::layout::LayoutConstraints;
use crate::node_id::NodeId;
use crate::traits::{RenderNode, UpdateResult, View};
use std::any::Any;
use std::boxed::Box;
use std::println;

pub struct Window {
    pub title: std::string::String,
    pub child: Box<dyn View>,
    pub decorated: bool,
}

impl Window {
    pub fn new(title: &str, child: impl View) -> Self {
        println!("[window] Window::new() called with title: {}", title);
        Self {
            title: std::string::String::from(title),
            child: Box::new(child),
            decorated: true,
        }
    }

    pub fn decorated(mut self, decorated: bool) -> Self {
        println!("[window] Window::decorated({}) called", decorated);
        self.decorated = decorated;
        self
    }

    pub const TITLEBAR_HEIGHT: f32 = 32.0;
    pub const CLOSE_BUTTON_SIZE: f32 = 24.0;
}

impl View for Window {
    fn type_id(&self) -> std::any::TypeId {
        std::any::TypeId::of::<Window>()
    }

    fn type_name(&self) -> &'static str {
        "Window"
    }

    fn build(&self) -> std::boxed::Box<dyn RenderNode> {
        println!("[window] Window::build() called, title: {}, decorated: {}", self.title, self.decorated);
        // Window doesn't need Clone - we consume it here
        let child = self.child.build();
        println!("[window] child built");
        std::boxed::Box::new(WindowRenderNode::new(self, child))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct WindowRenderNode {
    id: NodeId,
    parent: Option<NodeId>,
    title: std::string::String,
    decorated: bool,
    child: Box<dyn RenderNode>,
    buffer: Option<Buffer>,
    frame: Rect,
    dirty_flags: DirtyFlags,
    close_button_hovered: bool,
}

impl WindowRenderNode {
    pub fn new(view: &Window, child: Box<dyn RenderNode>) -> Self {
        println!("[window] WindowRenderNode::new() called");
        // Set parent for child
        let id = NodeId::new();
        let mut child_owned = child;
        child_owned.set_parent(id);

        let node = Self {
            id,
            parent: None,
            title: view.title.clone(),
            decorated: view.decorated,
            child: child_owned,
            buffer: None,
            frame: Rect::ZERO,
            dirty_flags: DirtyFlags::PAINT | DirtyFlags::LAYOUT,
            close_button_hovered: false,
        };
        println!("[window] WindowRenderNode created");
        node
    }

    fn get_close_button_rect(&self) -> Rect {
        Rect::new(
            Point::new(
                self.frame.size.width - Window::TITLEBAR_HEIGHT - 4.0,
                4.0,
            ),
            Size::new(Window::CLOSE_BUTTON_SIZE, Window::CLOSE_BUTTON_SIZE),
        )
    }
}

impl RenderNode for WindowRenderNode {
    fn id(&self) -> NodeId {
        self.id
    }

    fn parent(&self) -> Option<NodeId> {
        self.parent
    }

    fn set_parent(&mut self, parent: NodeId) {
        self.parent = Some(parent);
    }

    fn children(&self) -> &[Box<dyn RenderNode>] {
        std::slice::from_ref(&self.child)
    }

    fn children_mut(&mut self) -> &mut [Box<dyn RenderNode>] {
        std::slice::from_mut(&mut self.child)
    }

    fn get_child(&self, _id: NodeId) -> Option<&dyn RenderNode> {
        Some(self.child.as_ref())
    }

    fn get_child_mut(&mut self, _id: NodeId) -> Option<&mut (dyn RenderNode + '_)> {
        Some(self.child.as_mut())
    }

    fn type_id(&self) -> std::any::TypeId {
        std::any::TypeId::of::<WindowRenderNode>()
    }

    fn type_name(&self) -> &'static str {
        "Window"
    }

    fn try_update(&mut self, new_view: &dyn View) -> Option<UpdateResult> {
        new_view.as_any().downcast_ref::<Window>().map(|new_window| {
            // Update metadata
            let title_changed = self.title != new_window.title;
            let decorated_changed = self.decorated != new_window.decorated;

            if title_changed {
                self.title = new_window.title.clone();
            }
            if decorated_changed {
                self.decorated = new_window.decorated;
            }

            // Rebuild child if needed
            // Containers always rebuild children on update
            Some(UpdateResult::Changed(DirtyFlags::CHILDREN))
        }).flatten()
    }

    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        // Window fills available space
        let size = Size::new(
            constraints.max.width,
            constraints.max.height,
        );

        // Layout child
        let titlebar_height = if self.decorated {
            Window::TITLEBAR_HEIGHT
        } else {
            0.0
        };

        let child_rect = Rect::new(
            Point::new(0.0, titlebar_height),
            Size::new(
                size.width,
                (size.height - titlebar_height).max(0.0),
            ),
        );

        let _child_size = self.child.layout(LayoutConstraints::tight(child_rect.size));

        self.child.set_frame(child_rect);

        self.frame = Rect::new(Point::ZERO, size);
        size
    }

    fn set_frame(&mut self, frame: Rect) {
        self.frame = frame;
    }

    fn frame(&self) -> Rect {
        self.frame
    }

    fn render(&mut self) {
        if !self.is_dirty() {
            return;
        }

        use crate::geometry::Color;

        self.buffer = Some(Buffer::new(self.frame.size));

        if self.decorated {
            // Draw window background
            let bg_color = Color::rgb(40, 40, 40);
            self.buffer
                .as_mut()
                .unwrap()
                .fill_rect(self.frame, bg_color.as_bgra());

            // Draw titlebar with gradient effect (top to bottom)
            let titlebar_rect = Rect::new(
                Point::new(0.0, 0.0),
                Size::new(self.frame.size.width, Window::TITLEBAR_HEIGHT),
            );

            // Create a subtle gradient from darker (top) to lighter (bottom)
            let titlebar_top = Color::rgb(45, 45, 55);
            let titlebar_bottom = Color::rgb(55, 55, 65);

            // Draw gradient by interpolating colors
            if let Some(buf) = self.buffer.as_mut() {
                let width = buf.width() as usize;
                let height = Window::TITLEBAR_HEIGHT as usize;

                for y in 0..height {
                    let t = y as f32 / height as f32;
                    let r = (titlebar_top.as_bgra()[2] as f32 * (1.0 - t) + titlebar_bottom.as_bgra()[2] as f32 * t) as u8;
                    let g = (titlebar_top.as_bgra()[1] as f32 * (1.0 - t) + titlebar_bottom.as_bgra()[1] as f32 * t) as u8;
                    let b = (titlebar_top.as_bgra()[0] as f32 * (1.0 - t) + titlebar_bottom.as_bgra()[0] as f32 * t) as u8;
                    let color = Color::rgb(r, g, b).as_bgra();

                    for x in 0..width {
                        let idx = (y * width + x) * 4;
                        buf.as_mut_slice()[idx..idx + 4].copy_from_slice(&color);
                    }
                }
            }

            // Draw titlebar bottom border
            let border_color = Color::rgb(80, 80, 90);
            let border_rect = Rect::new(
                Point::new(0.0, Window::TITLEBAR_HEIGHT - 1.0),
                Size::new(self.frame.size.width, 1.0),
            );
            self.buffer
                .as_mut()
                .unwrap()
                .fill_rect(border_rect, border_color.as_bgra());

            // Draw title text with shadow effect
            use crate::graphics::draw_text;
            if let Some(buf) = self.buffer.as_mut() {
                let width = buf.width();
                let height = buf.height();

                // Draw shadow first (offset and darker)
                let shadow_color = Color::rgb(20, 20, 20);
                draw_text(
                    buf.as_mut_slice(),
                    width,
                    height,
                    &self.title,
                    9,  // x padding with offset
                    9,  // y padding with offset
                    13.0, // font size
                    shadow_color.as_bgra(),
                );

                // Draw main text
                let title_color = Color::TITLEBAR_TEXT;
                draw_text(
                    buf.as_mut_slice(),
                    width,
                    height,
                    &self.title,
                    8,  // x padding
                    8,  // y padding
                    13.0, // font size
                    title_color.as_bgra(),
                );
            }

            // Draw close button with rounded appearance and hover effect
            let close_button_rect = self.get_close_button_rect();

            // Close button background with state-based styling
            let close_button_color = if self.close_button_hovered {
                Color::rgb(200, 60, 60)  // Brighter red on hover
            } else {
                Color::rgb(140, 45, 45)  // Muted red normally
            };

            self.buffer
                .as_mut()
                .unwrap()
                .fill_rect(close_button_rect, close_button_color.as_bgra());

            // Draw close button border
            let border_color = if self.close_button_hovered {
                Color::rgb(220, 80, 80)
            } else {
                Color::rgb(160, 55, 55)
            };
            let border_width = 1.0;

            // Top border
            self.buffer.as_mut().unwrap().fill_rect(Rect::new(
                Point::new(close_button_rect.origin.x, close_button_rect.origin.y),
                Size::new(close_button_rect.size.width, border_width),
            ), border_color.as_bgra());

            // Bottom border
            self.buffer.as_mut().unwrap().fill_rect(Rect::new(
                Point::new(close_button_rect.origin.x, close_button_rect.origin.y + close_button_rect.size.height - border_width),
                Size::new(close_button_rect.size.width, border_width),
            ), border_color.as_bgra());

            // Left border
            self.buffer.as_mut().unwrap().fill_rect(Rect::new(
                Point::new(close_button_rect.origin.x, close_button_rect.origin.y),
                Size::new(border_width, close_button_rect.size.height),
            ), border_color.as_bgra());

            // Right border
            self.buffer.as_mut().unwrap().fill_rect(Rect::new(
                Point::new(close_button_rect.origin.x + close_button_rect.size.width - border_width, close_button_rect.origin.y),
                Size::new(border_width, close_button_rect.size.height),
            ), border_color.as_bgra());

            // Draw X icon with better anti-aliased look
            let x_color = Color::WHITE.as_bgra();
            let cx = close_button_rect.origin.x + Window::CLOSE_BUTTON_SIZE / 2.0;
            let cy = close_button_rect.origin.y + Window::CLOSE_BUTTON_SIZE / 2.0;
            let size = 6.0;
            let x_size = 2.0;

            // Draw X line 1: top-left to bottom-right
            for i in 0..6 {
                let offset = i as f32;
                let x1 = cx - size / 2.0 + offset;
                let y1 = cy - size / 2.0 + offset;
                let pixel_rect = Rect::new(Point::new(x1, y1), Size::new(x_size, x_size));
                self.buffer
                    .as_mut()
                    .unwrap()
                    .fill_rect(pixel_rect, x_color);
            }

            // Draw X line 2: bottom-left to top-right
            for i in 0..6 {
                let offset = i as f32;
                let x2 = cx + size / 2.0 - offset;
                let y2 = cy - size / 2.0 + offset;
                let pixel_rect = Rect::new(Point::new(x2, y2), Size::new(x_size, x_size));
                self.buffer
                    .as_mut()
                    .unwrap()
                    .fill_rect(pixel_rect, x_color);
            }
        }

        // Render child
        self.child.render();

        // Blit child buffer
        if let Some(child_buffer) = self.child.get_buffer() {
            let child_frame = self.child.frame();
            self.buffer
                .as_mut()
                .unwrap()
                .blit_from(child_buffer, child_frame);
        }

        self.clear_dirty();
    }

    fn get_buffer(&self) -> Option<&Buffer> {
        self.buffer.as_ref()
    }

    fn hit_test(&self, point: Point) -> HitResult {
        // Check close button first if decorated
        if self.decorated {
            let close_button_rect = self.get_close_button_rect();
            if close_button_rect.contains(point) {
                return HitResult::Handled(self.id);
            }
        }

        // Then check child
        let local_point = point - self.child.frame().origin;
        match self.child.hit_test(local_point) {
            HitResult::Handled(id) => HitResult::Handled(id),
            HitResult::Stop => HitResult::Stop,
            HitResult::Passthrough => {
                // Finally check window background
                if self.frame.contains(point) {
                    HitResult::Handled(self.id)
                } else {
                    HitResult::Passthrough
                }
            }
        }
    }

    fn handle_event(&mut self, event: &Event, ctx: &mut EventContext) {
        if !self.decorated {
            return;
        }

        match event {
            Event::Mouse(e) => {
                let close_button_rect = self.get_close_button_rect();

                match ctx.phase {
                    EventPhase::Target => {
                        match e.kind {
                            MouseEventKind::Move => {
                                let was_hovered = self.close_button_hovered;
                                self.close_button_hovered = close_button_rect.contains(e.position);
                                if was_hovered != self.close_button_hovered {
                                    self.mark_dirty(DirtyFlags::PAINT);
                                }
                            }
                            MouseEventKind::Release => {
                                if close_button_rect.contains(e.position) {
                                    // TODO: Trigger close action
                                    self.mark_dirty(DirtyFlags::PAINT);
                                }
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn mark_dirty(&mut self, flags: DirtyFlags) {
        self.dirty_flags |= flags;
    }

    fn is_dirty(&self) -> bool {
        !self.dirty_flags.is_empty()
    }

    fn clear_dirty(&mut self) {
        self.dirty_flags = DirtyFlags::empty();
    }
}
