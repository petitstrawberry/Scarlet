//! Window container with optional titlebar and background

use crate::buffer::Buffer;
use crate::dirty::DirtyFlags;
use crate::event::{Event, EventContext, EventPhase, HitResult, MouseEventKind};
use crate::geometry::{Point, Rect, Size};
use crate::layout::LayoutConstraints;
use crate::node_id::NodeId;
use crate::theme::with_theme;
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
    pub const BUTTON_SIZE: f32 = 32.0;
    pub const BUTTON_SPACING: f32 = 0.0;
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
    minimize_button_hovered: bool,
    maximize_button_hovered: bool,
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
            minimize_button_hovered: false,
            maximize_button_hovered: false,
            close_button_hovered: false,
        };
        println!("[window] WindowRenderNode created");
        node
    }

    fn get_minimize_button_rect(&self) -> Rect {
        let x = self.frame.size.width - Window::BUTTON_SIZE * 3.0;
        Rect::new(
            Point::new(x, 0.0),
            Size::new(Window::BUTTON_SIZE, Window::BUTTON_SIZE),
        )
    }

    fn get_maximize_button_rect(&self) -> Rect {
        let x = self.frame.size.width - Window::BUTTON_SIZE * 2.0;
        Rect::new(
            Point::new(x, 0.0),
            Size::new(Window::BUTTON_SIZE, Window::BUTTON_SIZE),
        )
    }

    fn get_close_button_rect(&self) -> Rect {
        let x = self.frame.size.width - Window::BUTTON_SIZE;
        Rect::new(
            Point::new(x, 0.0),
            Size::new(Window::BUTTON_SIZE, Window::BUTTON_SIZE),
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
            // Draw window background using theme
            let bg_color = with_theme(|theme| theme.window_background);
            self.buffer
                .as_mut()
                .unwrap()
                .fill_rect(self.frame, bg_color.as_bgra());

            // Draw titlebar using theme - flat, no gradient
            let titlebar_rect = Rect::new(
                Point::new(0.0, 0.0),
                Size::new(self.frame.size.width, Window::TITLEBAR_HEIGHT),
            );
            let titlebar_color = with_theme(|theme| theme.titlebar_background);
            self.buffer
                .as_mut()
                .unwrap()
                .fill_rect(titlebar_rect, titlebar_color.as_bgra());

            // Draw title text using theme (no shadow)
            use crate::graphics::draw_text;
            if let Some(buf) = self.buffer.as_mut() {
                let width = buf.width();
                let height = buf.height();

                // Simple text, no shadow
                let title_color = with_theme(|theme| theme.titlebar_text);
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

            // Draw window control buttons (1px lines, properly centered)
            let icon_color = with_theme(|theme| theme.titlebar_text).as_bgra();

            // Minimize button
            let minimize_rect = self.get_minimize_button_rect();
            if self.minimize_button_hovered {
                let hover_bg = with_theme(|theme| theme.close_button_background_hovered);
                self.buffer.as_mut().unwrap().fill_rect(minimize_rect, hover_bg.as_bgra());
            }
            // Minimize icon: horizontal line (1px thick, centered)
            let min_cx = minimize_rect.origin.x + Window::BUTTON_SIZE / 2.0;
            let min_cy = minimize_rect.origin.y + Window::BUTTON_SIZE / 2.0;
            let line_width = 10.0;
            self.buffer.as_mut().unwrap().fill_rect(Rect::new(
                Point::new(min_cx - line_width / 2.0, min_cy - 0.5),
                Size::new(line_width, 1.0),
            ), icon_color);

            // Maximize button
            let maximize_rect = self.get_maximize_button_rect();
            if self.maximize_button_hovered {
                let hover_bg = with_theme(|theme| theme.close_button_background_hovered);
                self.buffer.as_mut().unwrap().fill_rect(maximize_rect, hover_bg.as_bgra());
            }
            // Maximize icon: square outline (1px thick, centered)
            let max_cx = maximize_rect.origin.x + Window::BUTTON_SIZE / 2.0;
            let max_cy = maximize_rect.origin.y + Window::BUTTON_SIZE / 2.0;
            let box_size = 10.0;
            let box_half = box_size / 2.0;
            // Top
            self.buffer.as_mut().unwrap().fill_rect(Rect::new(
                Point::new(max_cx - box_half, max_cy - box_half),
                Size::new(box_size, 1.0),
            ), icon_color);
            // Bottom
            self.buffer.as_mut().unwrap().fill_rect(Rect::new(
                Point::new(max_cx - box_half, max_cy + box_half - 1.0),
                Size::new(box_size, 1.0),
            ), icon_color);
            // Left
            self.buffer.as_mut().unwrap().fill_rect(Rect::new(
                Point::new(max_cx - box_half, max_cy - box_half),
                Size::new(1.0, box_size),
            ), icon_color);
            // Right
            self.buffer.as_mut().unwrap().fill_rect(Rect::new(
                Point::new(max_cx + box_half - 1.0, max_cy - box_half),
                Size::new(1.0, box_size),
            ), icon_color);

            // Close button
            let close_rect = self.get_close_button_rect();
            if self.close_button_hovered {
                let hover_bg = with_theme(|theme| theme.close_button_background_hovered);
                self.buffer.as_mut().unwrap().fill_rect(close_rect, hover_bg.as_bgra());
            }
            // Close icon: X (1px thick, centered)
            let close_cx = close_rect.origin.x + Window::BUTTON_SIZE / 2.0;
            let close_cy = close_rect.origin.y + Window::BUTTON_SIZE / 2.0;
            let x_size = 8.0;
            let x_half = x_size / 2.0;
            // Diagonal line 1: top-left to bottom-right
            for i in 0..8 {
                let offset = i as f32;
                let x = close_cx - x_half + offset;
                let y = close_cy - x_half + offset;
                self.buffer.as_mut().unwrap().fill_rect(Rect::new(
                    Point::new(x, y),
                    Size::new(1.0, 1.0),
                ), icon_color);
            }
            // Diagonal line 2: bottom-left to top-right
            for i in 0..8 {
                let offset = i as f32;
                let x = close_cx + x_half - offset;
                let y = close_cy - x_half + offset;
                self.buffer.as_mut().unwrap().fill_rect(Rect::new(
                    Point::new(x, y),
                    Size::new(1.0, 1.0),
                ), icon_color);
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
        // Check titlebar buttons first if decorated
        if self.decorated {
            let minimize_rect = self.get_minimize_button_rect();
            let maximize_rect = self.get_maximize_button_rect();
            let close_button_rect = self.get_close_button_rect();

            if minimize_rect.contains(point) || maximize_rect.contains(point) || close_button_rect.contains(point) {
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
                let minimize_rect = self.get_minimize_button_rect();
                let maximize_rect = self.get_maximize_button_rect();
                let close_button_rect = self.get_close_button_rect();

                match ctx.phase {
                    EventPhase::Target => {
                        match e.kind {
                            MouseEventKind::Move => {
                                let was_min_hovered = self.minimize_button_hovered;
                                let was_max_hovered = self.maximize_button_hovered;
                                let was_close_hovered = self.close_button_hovered;

                                self.minimize_button_hovered = minimize_rect.contains(e.position);
                                self.maximize_button_hovered = maximize_rect.contains(e.position);
                                self.close_button_hovered = close_button_rect.contains(e.position);

                                if was_min_hovered != self.minimize_button_hovered
                                    || was_max_hovered != self.maximize_button_hovered
                                    || was_close_hovered != self.close_button_hovered
                                {
                                    self.mark_dirty(DirtyFlags::PAINT);
                                }
                            }
                            MouseEventKind::Press => {
                                if e.buttons.is_left_pressed() {
                                    if close_button_rect.contains(e.position) {
                                        // Close button clicked - TODO: trigger close
                                        println!("[window] Close button clicked");
                                    } else if minimize_rect.contains(e.position) {
                                        // Minimize button clicked - TODO: trigger minimize
                                        println!("[window] Minimize button clicked");
                                    } else if maximize_rect.contains(e.position) {
                                        // Maximize button clicked - TODO: trigger maximize
                                        println!("[window] Maximize button clicked");
                                    }
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
