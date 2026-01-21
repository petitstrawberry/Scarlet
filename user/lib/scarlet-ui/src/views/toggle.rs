use crate::buffer::Buffer;
use crate::dirty::DirtyFlags;
use crate::event::{Event, EventContext, EventPhase, HitResult, MouseEventKind};
use crate::geometry::{Point, Rect, Size};
use crate::layout::LayoutConstraints;
use crate::node_id::NodeId;
use crate::state::State;
use crate::traits::{RenderNode, UpdateResult, View};
use crate::geometry::Color;
use crate::theme::with_theme;
use std::any::Any;

#[derive(Clone)]
pub struct Toggle {
    pub is_on: State<bool>,
    pub on_color: Color,
    pub off_color: Color,
}

impl Toggle {
    pub fn new(is_on: State<bool>) -> Self {
        // Use theme colors as defaults
        let (on_color, off_color) = with_theme(|theme| {
            (Color::rgb(0, 200, 0), Color::rgb(200, 0, 0)) // Keep default green/red as fallback
        });

        Self {
            is_on,
            on_color,
            off_color,
        }
    }

    pub fn colors(mut self, on_color: Color, off_color: Color) -> Self {
        self.on_color = on_color;
        self.off_color = off_color;
        self
    }
}

impl View for Toggle {
    fn type_id(&self) -> std::any::TypeId {
        std::any::TypeId::of::<Self>()
    }

    fn type_name(&self) -> &'static str {
        "Toggle"
    }

    fn build(&self) -> std::boxed::Box<dyn RenderNode> {
        std::boxed::Box::new(ToggleRenderNode::new(self.clone()))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Default)]
struct ToggleInteractionState {
    hovered: bool,
    pressed: bool,
}

pub struct ToggleRenderNode {
    id: NodeId,
    parent: Option<NodeId>,
    view: Toggle,
    buffer: Option<Buffer>,
    frame: Rect,
    interaction_state: ToggleInteractionState,
    dirty_flags: DirtyFlags,
    last_known_state: bool,
}

impl ToggleRenderNode {
    pub fn new(view: Toggle) -> Self {
        let last_known_state = view.is_on.get();

        Self {
            id: NodeId::new(),
            parent: None,
            view,
            buffer: None,
            frame: Rect::ZERO,
            interaction_state: ToggleInteractionState::default(),
            dirty_flags: DirtyFlags::PAINT | DirtyFlags::LAYOUT,
            last_known_state,
        }
    }
}

impl RenderNode for ToggleRenderNode {
    fn id(&self) -> NodeId {
        self.id
    }

    fn parent(&self) -> Option<NodeId> {
        self.parent
    }

    fn set_parent(&mut self, parent: NodeId) {
        self.parent = Some(parent);
    }

    fn children(&self) -> &[std::boxed::Box<dyn RenderNode>] {
        &[]
    }

    fn type_id(&self) -> std::any::TypeId {
        std::any::TypeId::of::<Toggle>()
    }

    fn type_name(&self) -> &'static str {
        "Toggle"
    }

    fn try_update(&mut self, new_view: &dyn View) -> Option<UpdateResult> {
        new_view
            .as_any()
            .downcast_ref::<Toggle>()
            .map(|new_toggle| {
                let current_state = self.view.is_on.get();
                let new_state = new_toggle.is_on.get();

                // Check if state reference changed or value changed
                if current_state != new_state {
                    self.view = new_toggle.clone();
                    self.last_known_state = new_state;
                    Some(UpdateResult::Changed(DirtyFlags::PAINT))
                } else {
                    Some(UpdateResult::Unchanged)
                }
            })
            .flatten()
    }

    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        // Toggle is a fixed size square
        let size: f32 = 24.0;
        let size = Size::new(
            size.clamp(constraints.min.width, constraints.max.width),
            size.clamp(constraints.min.height, constraints.max.height),
        );

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

        let current_state = self.view.is_on.get();
        self.last_known_state = current_state;

        let color = if current_state {
            self.view.on_color
        } else {
            self.view.off_color
        };

        self.buffer = Some(Buffer::new(self.frame.size));

        // Draw toggle background
        self.buffer
            .as_mut()
            .unwrap()
            .fill_rect(self.frame, color.as_bgra());

        // Draw toggle indicator (circle)
        let padding = 2.0;
        let indicator_size = self.frame.size.width - padding * 2.0;
        let indicator_x = if current_state {
            self.frame.size.width - indicator_size - padding
        } else {
            padding
        };

        let indicator_rect = Rect::new(
            Point::new(indicator_x, padding),
            Size::new(indicator_size, indicator_size),
        );

        self.buffer
            .as_mut()
            .unwrap()
            .fill_rect(indicator_rect, Color::rgb(255, 255, 255).as_bgra());

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
        if local_frame.contains(point) {
            HitResult::Handled(self.id)
        } else {
            HitResult::Passthrough
        }
    }

    fn handle_event(&mut self, event: &Event, ctx: &mut EventContext) {
        match event {
            Event::Mouse(e) if ctx.phase == EventPhase::Target => {
                // Use local frame for bounds checking (point is in local coordinates)
                let local_frame = Rect::new(Point::ZERO, self.frame.size);
                match e.kind {
                    MouseEventKind::Move => {
                        let was_hovered = self.interaction_state.hovered;
                        self.interaction_state.hovered = local_frame.contains(e.position);
                        if was_hovered != self.interaction_state.hovered {
                            self.mark_dirty(DirtyFlags::PAINT);
                        }
                    }
                    MouseEventKind::Press => {
                        if self.interaction_state.hovered {
                            self.interaction_state.pressed = true;
                        }
                    }
                    MouseEventKind::Release => {
                        if self.interaction_state.pressed && self.interaction_state.hovered {
                            // Toggle the state
                            let current = self.view.is_on.get();
                            self.view.is_on.set(!current);
                            self.mark_dirty(DirtyFlags::PAINT);
                        }
                        self.interaction_state.pressed = false;
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
        // Also check if state changed
        let current_state = self.view.is_on.get();
        !self.dirty_flags.is_empty() || current_state != self.last_known_state
    }

    fn clear_dirty(&mut self) {
        self.dirty_flags = DirtyFlags::empty();
    }
}
