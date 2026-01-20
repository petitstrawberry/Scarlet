use crate::buffer::Buffer;
use crate::dirty::DirtyFlags;
use crate::event::{Event, EventContext, EventPhase, HitResult};
use crate::geometry::{Point, Rect, Size};
use crate::layout::LayoutConstraints;
use crate::node_id::NodeId;
use crate::traits::{RenderNode, UpdateResult, View};
use crate::geometry::Color;
use std::any::Any;

#[derive(Clone, PartialEq)]
pub struct Rectangle {
    pub color: Color,
    pub hover_color: Option<Color>,
}

impl Rectangle {
    pub fn new(color: Color) -> Self {
        Self {
            color,
            hover_color: None,
        }
    }

    pub fn hover_color(mut self, color: Color) -> Self {
        self.hover_color = Some(color);
        self
    }
}

impl View for Rectangle {
    fn type_id(&self) -> std::any::TypeId {
        std::any::TypeId::of::<Self>()
    }

    fn type_name(&self) -> &'static str {
        "Rectangle"
    }

    fn build(&self) -> std::boxed::Box<dyn RenderNode> {
        std::boxed::Box::new(RectangleRenderNode::new(self.clone()))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Default)]
struct InteractionState {
    hovered: bool,
    pressed: bool,
}

pub struct RectangleRenderNode {
    id: NodeId,
    parent: Option<NodeId>,
    view: Rectangle,
    buffer: Option<Buffer>,
    frame: Rect,
    interaction_state: InteractionState,
    dirty_flags: DirtyFlags,
}

impl RectangleRenderNode {
    pub fn new(view: Rectangle) -> Self {
        Self {
            id: NodeId::new(),
            parent: None,
            view,
            buffer: None,
            frame: Rect::ZERO,
            interaction_state: InteractionState::default(),
            dirty_flags: DirtyFlags::PAINT | DirtyFlags::LAYOUT,
        }
    }
}

impl RenderNode for RectangleRenderNode {
    fn id(&self) -> NodeId {
        self.id
    }

    fn parent(&self) -> Option<NodeId> {
        self.parent
    }

    fn set_parent(&mut self, parent: NodeId) {
        self.parent = Some(parent);
    }

    fn type_id(&self) -> std::any::TypeId {
        std::any::TypeId::of::<Rectangle>()
    }

    fn type_name(&self) -> &'static str {
        "Rectangle"
    }

    fn try_update(&mut self, new_view: &dyn View) -> Option<UpdateResult> {
        new_view
            .as_any()
            .downcast_ref::<Rectangle>()
            .map(|new_rect| {
                if self.view != *new_rect {
                    self.view = new_rect.clone();
                    Some(UpdateResult::Changed(DirtyFlags::PAINT))
                } else {
                    Some(UpdateResult::Unchanged)
                }
            })
            .flatten()
    }

    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        // Rectangle fills available space
        let size = Size::new(constraints.max.width, constraints.max.height);
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

        let color = if self.interaction_state.hovered {
            self.view.hover_color.unwrap_or(self.view.color)
        } else {
            self.view.color
        };

        self.buffer = Some(Buffer::new(self.frame.size));
        self.buffer
            .as_mut()
            .unwrap()
            .fill_rect(self.frame, color.as_bgra());
        self.clear_dirty();
    }

    fn get_buffer(&self) -> Option<&Buffer> {
        self.buffer.as_ref()
    }

    fn hit_test(&self, point: Point) -> HitResult {
        if self.frame.contains(point) {
            HitResult::Handled(self.id)
        } else {
            HitResult::Passthrough
        }
    }

    fn handle_event(&mut self, event: &Event, ctx: &mut EventContext) {
        match event {
            Event::Mouse(e) if ctx.phase == EventPhase::Target => {
                match e.kind {
                    crate::event::MouseEventKind::Move => {
                        let was_hovered = self.interaction_state.hovered;
                        self.interaction_state.hovered = self.frame.contains(e.position);
                        if was_hovered != self.interaction_state.hovered {
                            self.mark_dirty(DirtyFlags::PAINT);
                        }
                    }
                    crate::event::MouseEventKind::Press => {
                        if self.interaction_state.hovered {
                            self.interaction_state.pressed = true;
                            self.mark_dirty(DirtyFlags::PAINT);
                        }
                    }
                    crate::event::MouseEventKind::Release => {
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
        self.dirty_flags |= flags;
    }

    fn is_dirty(&self) -> bool {
        !self.dirty_flags.is_empty()
    }

    fn clear_dirty(&mut self) {
        self.dirty_flags = DirtyFlags::empty();
    }
}
