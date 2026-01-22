use crate::buffer::Buffer;
use crate::dirty::DirtyFlags;
use crate::event::{Event, EventContext, EventPhase, HitResult, MouseEventKind};
use crate::geometry::{Point, Rect, Size};
use crate::layout::LayoutConstraints;
use crate::node_id::NodeId;
use crate::traits::{RenderObject, UpdateResult, View};
use crate::views::text::Text;
use crate::geometry::Color;
use crate::theme::with_theme;
use std::any::Any;
use std::sync::Arc;
use std::println;

#[derive(Clone)]
pub struct Button {
    pub label: Text,
    pub action: Option<Arc<dyn Fn() + Send + Sync>>,
    pub colors: ButtonColors,
}

#[derive(Clone, Copy, PartialEq)]
pub struct ButtonColors {
    pub normal: Color,
    pub hovered: Color,
    pub pressed: Color,
}

impl Default for ButtonColors {
    fn default() -> Self {
        // Use theme colors as defaults with fallback colors
        with_theme(|theme| Self {
            normal: theme.button_background,
            hovered: theme.button_background_hovered,
            pressed: theme.button_background_pressed,
        })
    }
}

impl Button {
    pub fn new(text: &str) -> Self {
        // println!("[button] Button::new() called with text: {}", text);
        Self {
            label: Text::new(text),
            action: None,
            colors: ButtonColors::default(),
        }
    }

    pub fn on_click(mut self, callback: impl Fn() + Send + Sync + 'static) -> Self {
        // println!("[button] Button::on_click() called");
        self.action = Some(Arc::new(callback));
        self
    }

    pub fn colors(mut self, colors: ButtonColors) -> Self {
        self.colors = colors;
        self
    }

    fn handle_click(&self) {
        // println!("[button] handle_click() called, action exists: {}", self.action.is_some());
        if let Some(ref action) = self.action {
            // println!("[button] Executing action callback");
            // println!("[button] About to call action()");
            action();
            // println!("[button] action() returned");
        } else {
            // println!("[button] No action callback!");
        }
    }
}

impl View for Button {
    fn type_id(&self) -> std::any::TypeId {
        std::any::TypeId::of::<Self>()
    }

    fn type_name(&self) -> &'static str {
        "Button"
    }

    fn build(&self) -> std::boxed::Box<dyn RenderObject> {
        // println!("[button] Button::build() called");
        std::boxed::Box::new(ButtonRenderObject::new(self.clone()))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Default)]
struct ButtonInteractionState {
    hovered: bool,
    pressed: bool,
    focused: bool,
}

pub struct ButtonRenderObject {
    id: NodeId,
    parent: Option<NodeId>,
    view: Button,
    label_node: std::boxed::Box<dyn RenderObject>,
    buffer: Option<Buffer>,
    frame: Rect,
    interaction_state: ButtonInteractionState,
    dirty_flags: DirtyFlags,
}

impl ButtonRenderObject {
    pub fn new(view: Button) -> Self {
        let label_node = view.label.build();

        Self {
            id: NodeId::new(),
            parent: None,
            view,
            label_node,
            buffer: None,
            frame: Rect::ZERO,
            interaction_state: ButtonInteractionState::default(),
            dirty_flags: DirtyFlags::PAINT | DirtyFlags::LAYOUT,
        }
    }
}

impl RenderObject for ButtonRenderObject {
    fn id(&self) -> NodeId {
        self.id
    }

    fn parent(&self) -> Option<NodeId> {
        self.parent
    }

    fn set_parent(&mut self, parent: NodeId) {
        self.parent = Some(parent);
    }

    fn children(&self) -> &[std::boxed::Box<dyn RenderObject>] {
        &[]
    }

    fn type_id(&self) -> std::any::TypeId {
        std::any::TypeId::of::<Button>()
    }

    fn type_name(&self) -> &'static str {
        "Button"
    }

    fn try_update(&mut self, new_view: &dyn View) -> Option<UpdateResult> {
        new_view
            .as_any()
            .downcast_ref::<Button>()
            .map(|new_button| {
                // Always replace on update (actions may change)
                self.view = new_button.clone();
                Some(UpdateResult::Changed(DirtyFlags::PAINT))
            })
            .flatten()
    }

    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        // Button has intrinsic size based on label content
        // Layout label with truly loose constraints to get its actual intrinsic size
        let label_constraints = LayoutConstraints::loose(Size::new(f32::MAX, f32::MAX));
        let label_size = self.label_node.layout(label_constraints);
        println!("[button] ButtonRenderObject::layout() label_size: {:?}", label_size);
        println!("[button] ButtonRenderObject::layout() constraints: min={:?} max={:?}", constraints.min, constraints.max);

        // Button is slightly larger than label (intrinsic size)
        let padding = 12.0;
        let intrinsic_size = Size::new(
            label_size.width + padding * 2.0,
            label_size.height + padding * 2.0,
        );

        // Clamp to constraints (don't exceed available space, but don't grow)
        let size = Size::new(
            intrinsic_size.width.clamp(constraints.min.width, constraints.max.width),
            intrinsic_size.height.clamp(constraints.min.height, constraints.max.height),
        );

        // NOTE: Don't re-layout the label with tight constraints!
        // Keep label at its intrinsic size, let it be clipped during rendering if button is smaller
        // This ensures text is not cut off

        println!("[button] ButtonRenderObject::layout() intrinsic_size: {:?}, final size: {:?}", intrinsic_size, size);
        // Update frame.size but NOT frame.origin (parent controls origin)
        self.frame.size = size;
        println!("[button] ButtonRenderObject::layout() updated frame.size to {:?}", self.frame.size);
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

        println!("[button] render: self.frame.size={:}x{:}", self.frame.size.width, self.frame.size.height);
        println!("[button] render: label_node.frame.size={:}x{:}",
                 self.label_node.frame().size.width,
                 self.label_node.frame().size.height);

        let color = if self.interaction_state.pressed {
            // println!("[button] render: using PRESSED color");
            self.view.colors.pressed
        } else if self.interaction_state.hovered {
            // println!("[button] render: using HOVERED color");
            self.view.colors.hovered
        } else {
            // println!("[button] render: using NORMAL color");
            self.view.colors.normal
        };

        // println!("[button] ButtonRenderObject::render() creating buffer with size: {:?}", self.frame.size);
        self.buffer = Some(Buffer::new(self.frame.size));
        self.buffer
            .as_mut()
            .unwrap()
            .fill_rect(Rect::new(Point::ZERO, self.frame.size), color.as_bgra());

        // Draw border (always visible, using theme)
        let border_color = with_theme(|theme| theme.button_border);
        let border_width = 1.0;

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

        // Draw focus indicator (additional border when focused)
        if self.interaction_state.focused {
            let focus_color = Color::rgb(0, 120, 215); // Blue focus ring
            let focus_width = 2.0;

            // Inner focus border
            self.buffer.as_mut().unwrap().fill_rect(Rect::new(
                Point::new(1.0, 1.0),
                Size::new(self.frame.size.width - 2.0, focus_width),
            ), focus_color.as_bgra());

            self.buffer.as_mut().unwrap().fill_rect(Rect::new(
                Point::new(1.0, self.frame.size.height - 1.0 - focus_width),
                Size::new(self.frame.size.width - 2.0, focus_width),
            ), focus_color.as_bgra());

            self.buffer.as_mut().unwrap().fill_rect(Rect::new(
                Point::new(1.0, 1.0),
                Size::new(focus_width, self.frame.size.height - 2.0),
            ), focus_color.as_bgra());

            self.buffer.as_mut().unwrap().fill_rect(Rect::new(
                Point::new(self.frame.size.width - 1.0 - focus_width, 1.0),
                Size::new(focus_width, self.frame.size.height - 2.0),
            ), focus_color.as_bgra());
        }

        // Render label centered
        let padding = 12.0;

        // Label was already laid out in layout() with correct size
        let label_size = self.label_node.frame().size;

        // Center label in button
        let label_frame = Rect::new(
            Point::new(padding, padding),
            label_size,
        );
        // println!("[button] ButtonRenderObject::render() label_frame: {:?}", label_frame);
        self.label_node.set_frame(label_frame);
        self.label_node.render();

        if let Some(label_buffer) = self.label_node.get_buffer() {
            // println!("[button] ButtonRenderObject::render() blitting label buffer");
            self.buffer
                .as_mut()
                .unwrap()
                .blit_from(label_buffer, label_frame);
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
        // Check against local frame (origin at 0,0) since point is in local coordinates
        let local_frame = Rect::new(Point::ZERO, self.frame.size);
        // println!("[button] hit_test: point={:?}, local_frame={:?} (Button local coords), contains={}",
        //     point, local_frame, local_frame.contains(point));
        if local_frame.contains(point) {
            HitResult::Handled(self.id)
        } else {
            HitResult::Passthrough
        }
    }

    fn handle_event(&mut self, event: &Event, ctx: &mut EventContext) {
        // println!("[button] handle_event: phase={:?}, event={:?}", ctx.phase, event);
        match event {
            Event::Mouse(e) => {
                // Handle hover state changes (Leave/Enter) in all phases
                if e.kind == MouseEventKind::Leave {
                    // println!("[button] Mouse Leave! was_hovered={}", self.interaction_state.hovered);
                    if self.interaction_state.hovered {
                        self.interaction_state.hovered = false;
                        self.mark_dirty(DirtyFlags::PAINT);
                    }
                    return;  // Leave is handled, don't process further
                }

                if e.kind == MouseEventKind::Enter {
                    // println!("[button] Mouse Enter! was_hovered={}", self.interaction_state.hovered);
                    if !self.interaction_state.hovered {
                        self.interaction_state.hovered = true;
                        self.mark_dirty(DirtyFlags::PAINT);
                    }
                    return;  // Enter is handled, don't process further
                }

                // Only handle other mouse events in Target phase (clicks, etc.)
                if ctx.phase != EventPhase::Target {
                    return;
                }

                // Use local frame for bounds checking (point is in local coordinates)
                let local_frame = Rect::new(Point::ZERO, self.frame.size);
                // println!("[button] Mouse event at {:?}, local_frame={:?}, contains={}", e.position, local_frame, local_frame.contains(e.position));
                match e.kind {
                    MouseEventKind::Press => {
                        if self.interaction_state.hovered {
                            self.interaction_state.pressed = true;
                            // println!("[button] Pressed!");
                            // Note: Don't set focused directly here
                            // Focus is managed by FocusManager via request_focus()/lose_focus()
                            self.mark_dirty(DirtyFlags::PAINT);
                        }
                    }
                    MouseEventKind::Release => {
                        println!("[button] Release: pressed={}, hovered={}", self.interaction_state.pressed, self.interaction_state.hovered);
                        if self.interaction_state.pressed && self.interaction_state.hovered {
                            println!("[button] Clicked! Executing callback");
                            self.view.handle_click();
                        } else {
                            println!("[button] Release ignored: not clicked (pressed={}, hovered={})", self.interaction_state.pressed, self.interaction_state.hovered);
                        }
                        self.interaction_state.pressed = false;
                        self.mark_dirty(DirtyFlags::PAINT);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn mark_dirty(&mut self, flags: DirtyFlags) {
        // println!("[button] mark_dirty({:?}), dirty_flags was: {:?}", flags, self.dirty_flags);
        self.dirty_flags |= flags;
        // println!("[button] mark_dirty() done, dirty_flags now: {:?}", self.dirty_flags);
    }

    fn is_dirty(&self) -> bool {
        let dirty = !self.dirty_flags.is_empty();
        // if dirty {
        //     println!("[button] is_dirty() = TRUE, dirty_flags: {:?}", self.dirty_flags);
        // }
        dirty
    }

    fn clear_dirty(&mut self) {
        self.dirty_flags = DirtyFlags::empty();
    }

    fn is_focusable(&self) -> bool {
        true
    }

    fn request_focus(&mut self) -> bool {
        self.interaction_state.focused = true;
        self.mark_dirty(DirtyFlags::PAINT);
        true
    }

    fn lose_focus(&mut self) {
        if self.interaction_state.focused {
            self.interaction_state.focused = false;
            self.mark_dirty(DirtyFlags::PAINT);
        }
    }
}
