use crate::buffer::Buffer;
use crate::dirty::DirtyFlags;
use crate::event::{Event, EventContext, EventPhase, HitResult, MouseEventKind};
use crate::geometry::{Point, Rect, Size};
use crate::layout::LayoutConstraints;
use crate::node_id::NodeId;
use crate::traits::{RenderNode, UpdateResult, View};
use crate::views::text::Text;
use std::any::Any;
use std::sync::Arc;

#[derive(Clone)]
pub struct Button {
    pub label: Text,
    pub action: Option<Arc<dyn Fn() + Send + Sync>>,
    pub colors: ButtonColors,
}

#[derive(Clone, Copy, PartialEq)]
pub struct ButtonColors {
    pub normal: [u8; 4],
    pub hovered: [u8; 4],
    pub pressed: [u8; 4],
}

impl Default for ButtonColors {
    fn default() -> Self {
        Self {
            normal: [100, 100, 100, 255],
            hovered: [120, 120, 120, 255],
            pressed: [80, 80, 80, 255],
        }
    }
}

impl Button {
    pub fn new(text: &str) -> Self {
        Self {
            label: Text::new(text),
            action: None,
            colors: ButtonColors::default(),
        }
    }

    pub fn on_click(mut self, callback: impl Fn() + Send + Sync + 'static) -> Self {
        self.action = Some(Arc::new(callback));
        self
    }

    pub fn colors(mut self, colors: ButtonColors) -> Self {
        self.colors = colors;
        self
    }

    fn handle_click(&self) {
        if let Some(ref action) = self.action {
            action();
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

    fn build(&self) -> std::boxed::Box<dyn RenderNode> {
        std::boxed::Box::new(ButtonRenderNode::new(self.clone()))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Default)]
struct ButtonInteractionState {
    hovered: bool,
    pressed: bool,
}

pub struct ButtonRenderNode {
    id: NodeId,
    parent: Option<NodeId>,
    view: Button,
    label_node: std::boxed::Box<dyn RenderNode>,
    buffer: Option<Buffer>,
    frame: Rect,
    interaction_state: ButtonInteractionState,
    dirty_flags: DirtyFlags,
}

impl ButtonRenderNode {
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

impl RenderNode for ButtonRenderNode {
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
        // Layout label first
        let label_size = self.label_node.layout(constraints);

        // Button is slightly larger than label
        let padding = 8.0;
        let size = Size::new(
            label_size.width + padding * 2.0,
            label_size.height + padding * 2.0,
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

        let color = if self.interaction_state.pressed {
            self.view.colors.pressed
        } else if self.interaction_state.hovered {
            self.view.colors.hovered
        } else {
            self.view.colors.normal
        };

        self.buffer = Some(Buffer::new(self.frame.size));
        self.buffer
            .as_mut()
            .unwrap()
            .fill_rect(self.frame, color);

        // Render label centered
        let padding = 8.0;
        let label_frame = Rect::new(
            Point::new(padding, padding),
            self.label_node.frame().size,
        );
        self.label_node.set_frame(label_frame);
        self.label_node.render();

        if let Some(label_buffer) = self.label_node.get_buffer() {
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
                    MouseEventKind::Move => {
                        let was_hovered = self.interaction_state.hovered;
                        self.interaction_state.hovered = self.frame.contains(e.position);
                        if was_hovered != self.interaction_state.hovered {
                            self.mark_dirty(DirtyFlags::PAINT);
                        }
                    }
                    MouseEventKind::Press => {
                        if self.interaction_state.hovered {
                            self.interaction_state.pressed = true;
                            self.mark_dirty(DirtyFlags::PAINT);
                        }
                    }
                    MouseEventKind::Release => {
                        if self.interaction_state.pressed && self.interaction_state.hovered {
                            self.view.handle_click();
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
        self.dirty_flags |= flags;
    }

    fn is_dirty(&self) -> bool {
        !self.dirty_flags.is_empty()
    }

    fn clear_dirty(&mut self) {
        self.dirty_flags = DirtyFlags::empty();
    }
}
