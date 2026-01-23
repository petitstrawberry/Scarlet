//! Window container with optional titlebar and background

use crate::buffer::Buffer;
use crate::dirty::DirtyFlags;
use crate::event::{Event, EventContext, EventPhase, HitResult, MouseEventKind};
use crate::geometry::{Point, Rect, Size};
use crate::layout::LayoutConstraints;
use crate::node_id::NodeId;
use crate::theme::with_theme;
use crate::traits::{RenderObject, UpdateResult, View};
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
    pub const CLOSE_BUTTON_SIZE: f32 = 18.0;
    pub const CLOSE_BUTTON_MARGIN: f32 = 8.0;
}

impl View for Window {
    fn type_id(&self) -> std::any::TypeId {
        std::any::TypeId::of::<Window>()
    }

    fn type_name(&self) -> &'static str {
        "Window"
    }

    fn build(&self) -> std::boxed::Box<dyn RenderObject> {
        println!("[window] Window::build() called, title: {}, decorated: {}", self.title, self.decorated);
        // Window doesn't need Clone - we consume it here
        let child = self.child.build();
        println!("[window] child built");
        std::boxed::Box::new(WindowRenderObject::new(self, child))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct WindowRenderObject {
    id: NodeId,
    parent: Option<NodeId>,
    title: std::string::String,
    decorated: bool,
    child: Box<dyn RenderObject>,
    buffer: Option<Buffer>,
    frame: Rect,
    dirty_flags: DirtyFlags,
    minimize_button_hovered: bool,
    maximize_button_hovered: bool,
    close_button_hovered: bool,
    // Titlebar drag state
    pub is_dragging_titlebar: bool,
    drag_start_position: Point,
    // Maximize state tracking
    pub is_maximized: bool,
    // Button click requests (for Application to handle)
    pub request_minimize: bool,
    pub request_maximize: bool,
    pub request_restore: bool,
    pub request_close: bool,
    pub request_move: bool,
}

impl WindowRenderObject {
    pub fn new(view: &Window, child: Box<dyn RenderObject>) -> Self {
        println!("[window] WindowRenderObject::new() called");
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
            is_dragging_titlebar: false,
            drag_start_position: Point::ZERO,
            is_maximized: false,
            request_minimize: false,
            request_maximize: false,
            request_restore: false,
            request_close: false,
            request_move: false,
        };
        println!("[window] WindowRenderObject created");
        node
    }

    /// Composite a child buffer into the window's buffer at the specified position
    fn composite_child_buffer(&mut self, src: &Buffer, dest_frame: Rect) {
        let target = self.buffer.as_mut().unwrap();
        let src_width = src.width();
        let src_height = src.height();
        let src_data = src.as_slice();

        // Get target dimensions before mutable borrow
        let target_width = target.width();
        let target_height = target.height();
        let target_stride = target.stride();

        let dest_data = target.as_mut_slice();

        let dest_x = libm::ceilf(dest_frame.origin.x) as usize;
        let dest_y = libm::ceilf(dest_frame.origin.y) as usize;

        // Clamp to buffer bounds
        let dest_x = dest_x.clamp(0, target_width);
        let dest_y = dest_y.clamp(0, target_height);

        let copy_width = src_width.min(target_width - dest_x);
        let copy_height = src_height.min(target_height - dest_y);

        for y in 0..copy_height {
            for x in 0..copy_width {
                let src_offset = y * src.stride() + x * 4;
                let dest_offset = (dest_y + y) * target_stride + (dest_x + x) * 4;

                let src_b = src_data[src_offset];
                let src_g = src_data[src_offset + 1];
                let src_r = src_data[src_offset + 2];
                let src_a = src_data[src_offset + 3];

                // Alpha blending
                if src_a == 255 {
                    // Opaque: copy directly
                    dest_data[dest_offset] = src_b;
                    dest_data[dest_offset + 1] = src_g;
                    dest_data[dest_offset + 2] = src_r;
                    dest_data[dest_offset + 3] = src_a;
                } else if src_a > 0 {
                    // Semi-transparent: blend with destination
                    let dst_a = dest_data[dest_offset + 3];

                    if dst_a == 0 {
                        // Destination is fully transparent, just copy source
                        dest_data[dest_offset] = src_b;
                        dest_data[dest_offset + 1] = src_g;
                        dest_data[dest_offset + 2] = src_r;
                        dest_data[dest_offset + 3] = src_a;
                    } else {
                        // Both have some alpha, proper over compositing
                        let src_a_f = src_a as f32 / 255.0;
                        let dst_a_f = dst_a as f32 / 255.0;

                        // Final alpha (over operator)
                        let out_a_f = src_a_f + dst_a_f * (1.0 - src_a_f);
                        let out_a = (out_a_f * 255.0).min(255.0) as u8;

                        // Blend colors
                        let src_b_f = src_b as f32;
                        let src_g_f = src_g as f32;
                        let src_r_f = src_r as f32;

                        let dst_b_f = dest_data[dest_offset] as f32;
                        let dst_g_f = dest_data[dest_offset + 1] as f32;
                        let dst_r_f = dest_data[dest_offset + 2] as f32;

                        let out_b = (src_b_f * src_a_f + dst_b_f * dst_a_f * (1.0 - src_a_f)) / out_a_f.max(0.01);
                        let out_g = (src_g_f * src_a_f + dst_g_f * dst_a_f * (1.0 - src_a_f)) / out_a_f.max(0.01);
                        let out_r = (src_r_f * src_a_f + dst_r_f * dst_a_f * (1.0 - src_a_f)) / out_a_f.max(0.01);

                        dest_data[dest_offset] = out_b.min(255.0) as u8;
                        dest_data[dest_offset + 1] = out_g.min(255.0) as u8;
                        dest_data[dest_offset + 2] = out_r.min(255.0) as u8;
                        dest_data[dest_offset + 3] = out_a;
                    }
                }
                // If src_a == 0, keep destination pixel unchanged
            }
        }
    }

    fn get_close_button_rect(&self) -> Rect {
        let seg_w = Window::CLOSE_BUTTON_SIZE + Window::CLOSE_BUTTON_MARGIN * 2.0;
        let total_w = seg_w * 3.0;
        let right_x = self.frame.size.width - total_w;
        let x = right_x + seg_w * 2.0;  // Close is rightmost (index 0)
        Rect::new(
            Point::new(x, 0.0),
            Size::new(seg_w, Window::TITLEBAR_HEIGHT),
        )
    }

    fn get_maximize_button_rect(&self) -> Rect {
        let seg_w = Window::CLOSE_BUTTON_SIZE + Window::CLOSE_BUTTON_MARGIN * 2.0;
        let total_w = seg_w * 3.0;
        let right_x = self.frame.size.width - total_w;
        let x = right_x + seg_w;  // Maximize is middle (index 1)
        Rect::new(
            Point::new(x, 0.0),
            Size::new(seg_w, Window::TITLEBAR_HEIGHT),
        )
    }

    fn get_minimize_button_rect(&self) -> Rect {
        let seg_w = Window::CLOSE_BUTTON_SIZE + Window::CLOSE_BUTTON_MARGIN * 2.0;
        let total_w = seg_w * 3.0;
        let right_x = self.frame.size.width - total_w;
        let x = right_x;  // Minimize is leftmost (index 2)
        Rect::new(
            Point::new(x, 0.0),
            Size::new(seg_w, Window::TITLEBAR_HEIGHT),
        )
    }

    fn get_titlebar_rect(&self) -> Rect {
        Rect::new(
            Point::ZERO,
            Size::new(self.frame.size.width, Window::TITLEBAR_HEIGHT),
        )
    }
}

impl RenderObject for WindowRenderObject {
    fn id(&self) -> NodeId {
        self.id
    }

    fn parent(&self) -> Option<NodeId> {
        self.parent
    }

    fn set_parent(&mut self, parent: NodeId) {
        self.parent = Some(parent);
    }

    fn children(&self) -> &[Box<dyn RenderObject>] {
        std::slice::from_ref(&self.child)
    }

    fn children_mut(&mut self) -> &mut [Box<dyn RenderObject>] {
        std::slice::from_mut(&mut self.child)
    }

    fn get_child(&self, id: NodeId) -> Option<&dyn RenderObject> {
        if self.child.id() == id {
            Some(self.child.as_ref())
        } else {
            None
        }
    }

    fn get_child_mut(&mut self, id: NodeId) -> Option<&mut (dyn RenderObject + '_)> {
        if self.child.id() == id {
            Some(self.child.as_mut())
        } else {
            None
        }
    }

    fn type_id(&self) -> std::any::TypeId {
        std::any::TypeId::of::<WindowRenderObject>()
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
        let old_size = self.frame.size;
        self.frame = frame;

        // If size changed, trigger layout and repaint
        if old_size != frame.size {
            self.mark_dirty(DirtyFlags::LAYOUT | DirtyFlags::PAINT);
            println!("[window] Window size changed: {:?} -> {:?}, marking dirty", old_size, frame.size);
        }
    }

    fn frame(&self) -> Rect {
        self.frame
    }

    fn render(&mut self) {
        // NOTE: Don't check is_dirty() here!
        // Parent may call render() on us when children are dirty (even if we're not)
        // We need to blit children's buffers even if we're not dirty ourselves

        // However, if LAYOUT flag is set, we need to relayout
        if self.dirty_flags.contains(DirtyFlags::LAYOUT) {
            println!("[window] Relayouting due to LAYOUT dirty flag");
            // Relayout with current frame size
            let constraints = LayoutConstraints::tight(self.frame.size);
            self.layout(constraints);
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

                // Title text at (10, 9) like deprecated
                let title_color = with_theme(|theme| theme.titlebar_text);
                draw_text(
                    buf.as_mut_slice(),
                    width,
                    height,
                    &self.title,
                    10,  // x position (deprecated)
                    9,   // y position (deprecated)
                    13.0, // font size
                    title_color.as_bgra(),
                );
            }

            // Draw window control buttons (deprecated style)
            let close_rect = self.get_close_button_rect();
            let maximize_rect = self.get_maximize_button_rect();
            let minimize_rect = self.get_minimize_button_rect();

            let close_color = if self.close_button_hovered {
                with_theme(|theme| theme.titlebar_button_background_hovered)
            } else {
                with_theme(|theme| theme.titlebar_button_background)
            };

            let maximize_color = if self.maximize_button_hovered {
                with_theme(|theme| theme.titlebar_button_background_hovered)
            } else {
                with_theme(|theme| theme.titlebar_button_background)
            };

            let minimize_color = if self.minimize_button_hovered {
                with_theme(|theme| theme.titlebar_button_background_hovered)
            } else {
                with_theme(|theme| theme.titlebar_button_background)
            };

            // Draw button backgrounds
            self.buffer.as_mut().unwrap().fill_rect(close_rect, close_color.as_bgra());
            self.buffer.as_mut().unwrap().fill_rect(maximize_rect, maximize_color.as_bgra());
            self.buffer.as_mut().unwrap().fill_rect(minimize_rect, minimize_color.as_bgra());

            // Draw icons (deprecated style: using theme)
            let icon_color = with_theme(|theme| theme.titlebar_button_icon).as_bgra();
            let icon_size: i32 = 10;
            let icon_half = icon_size / 2;

            // Close button: X icon
            let cx = close_rect.origin.x as i32 + close_rect.size.width as i32 / 2;
            let cy = close_rect.origin.y as i32 + close_rect.size.height as i32 / 2;
            let x0 = cx - icon_half;
            let x1 = cx + icon_half - 1;
            let y0 = cy - icon_half;
            let y1 = cy + icon_half - 1;

            // Draw X line 1 (diagonal)
            for i in 0..10 {
                let offset = i;
                self.buffer.as_mut().unwrap().fill_rect(Rect::new(
                    Point::new((x0 + offset) as f32, (y0 + offset) as f32),
                    Size::new(1.0, 1.0),
                ), icon_color);
            }
            // Draw X line 2 (diagonal)
            for i in 0..10 {
                let offset = i;
                self.buffer.as_mut().unwrap().fill_rect(Rect::new(
                    Point::new((x1 - offset) as f32, (y0 + offset) as f32),
                    Size::new(1.0, 1.0),
                ), icon_color);
            }

            // Maximize button: square outline
            let mx = maximize_rect.origin.x as i32 + maximize_rect.size.width as i32 / 2;
            let my = maximize_rect.origin.y as i32 + maximize_rect.size.height as i32 / 2;
            let mx0 = mx - icon_half;
            let my0 = my - icon_half;
            let msize = 10;

            // Top
            self.buffer.as_mut().unwrap().fill_rect(Rect::new(
                Point::new(mx0 as f32, my0 as f32),
                Size::new(msize as f32, 1.0),
            ), icon_color);
            // Bottom
            self.buffer.as_mut().unwrap().fill_rect(Rect::new(
                Point::new(mx0 as f32, (my0 + msize - 1) as f32),
                Size::new(msize as f32, 1.0),
            ), icon_color);
            // Left
            self.buffer.as_mut().unwrap().fill_rect(Rect::new(
                Point::new(mx0 as f32, my0 as f32),
                Size::new(1.0, msize as f32),
            ), icon_color);
            // Right
            self.buffer.as_mut().unwrap().fill_rect(Rect::new(
                Point::new((mx0 + msize - 1) as f32, my0 as f32),
                Size::new(1.0, msize as f32),
            ), icon_color);

            // Minimize button: horizontal line
            let nx = minimize_rect.origin.x as i32 + minimize_rect.size.width as i32 / 2;
            let ny = minimize_rect.origin.y as i32 + minimize_rect.size.height as i32 / 2 + 3;
            let nsize = 12;
            let nhalf = nsize / 2;

            self.buffer.as_mut().unwrap().fill_rect(Rect::new(
                Point::new((nx - nhalf) as f32, ny as f32),
                Size::new(nsize as f32, 1.0),
            ), icon_color);
        }

        // Render child
        self.child.render();

        // Composite child buffer (get frame and buffer before any borrow)
        let child_frame = self.child.frame();
        let child_buffer = self.child.get_buffer().cloned();
        if let Some(child_buffer) = child_buffer {
            self.composite_child_buffer(&child_buffer, child_frame);
        }

        // Draw window border (deprecated style)
        if self.decorated {
            let border_color = with_theme(|theme| theme.window_border);
            let border_width = 1.0;

            // Left border
            self.buffer.as_mut().unwrap().fill_rect(Rect::new(
                Point::new(0.0, 0.0),
                Size::new(border_width, self.frame.size.height),
            ), border_color.as_bgra());

            // Right border
            self.buffer.as_mut().unwrap().fill_rect(Rect::new(
                Point::new(self.frame.size.width - border_width, 0.0),
                Size::new(border_width, self.frame.size.height),
            ), border_color.as_bgra());

            // Top border
            self.buffer.as_mut().unwrap().fill_rect(Rect::new(
                Point::new(0.0, 0.0),
                Size::new(self.frame.size.width, border_width),
            ), border_color.as_bgra());

            // Bottom border
            self.buffer.as_mut().unwrap().fill_rect(Rect::new(
                Point::new(0.0, self.frame.size.height - border_width),
                Size::new(self.frame.size.width, border_width),
            ), border_color.as_bgra());
        }

        self.clear_dirty();
    }

    fn get_buffer(&self) -> Option<&Buffer> {
        self.buffer.as_ref()
    }

    fn get_buffer_mut(&mut self) -> Option<&mut Buffer> {
        self.buffer.as_mut()
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
                let titlebar_rect = self.get_titlebar_rect();

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

                                // Handle dragging
                                if self.is_dragging_titlebar && e.buttons.is_left_pressed() {
                                    // Drag in progress - request will be handled by Application
                                    // via checking self.is_dragging_titlebar flag
                                }
                            }
                            MouseEventKind::Press => {
                                if e.buttons.is_left_pressed() {
                                    if close_button_rect.contains(e.position) {
                                        println!("[window] Close button clicked");
                                        self.request_close = true;
                                    } else if minimize_rect.contains(e.position) {
                                        println!("[window] Minimize button clicked");
                                        self.request_minimize = true;
                                    } else if maximize_rect.contains(e.position) {
                                        // Toggle maximize/restore
                                        if self.is_maximized {
                                            println!("[window] Restore button clicked (window is maximized)");
                                            self.request_restore = true;
                                        } else {
                                            println!("[window] Maximize button clicked (window is not maximized)");
                                            self.request_maximize = true;
                                        }
                                    } else if titlebar_rect.contains(e.position) {
                                        // Clicked on titlebar (not buttons) - start drag
                                        println!("[window] Titlebar clicked at {:?}", e.position);
                                        self.is_dragging_titlebar = true;
                                        self.drag_start_position = e.position;
                                        self.request_move = true;
                                    }
                                }
                            }
                            MouseEventKind::Release => {
                                if self.is_dragging_titlebar {
                                    println!("[window] Titlebar drag released");
                                    self.is_dragging_titlebar = false;
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
