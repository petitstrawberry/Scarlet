use crate::buffer::Buffer;
use crate::dirty::DirtyFlags;
use crate::event::{Event, EventContext, EventPhase, HitResult};
use crate::geometry::{Point, Rect, Size};
use crate::layout::LayoutConstraints;
use crate::node_id::NodeId;
use crate::state::State;
use crate::traits::{RenderNode, UpdateResult, View};
use std::any::Any;
use std::string::String;

#[derive(Clone)]
pub struct TextField {
    pub text: State<String>,
    pub placeholder: String,
    pub width: f32,
}

impl TextField {
    pub fn new(text: State<String>) -> Self {
        Self {
            text,
            placeholder: String::new(),
            width: 200.0,
        }
    }

    pub fn placeholder(mut self, placeholder: &str) -> Self {
        self.placeholder = String::from(placeholder);
        self
    }

    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }
}

impl View for TextField {
    fn type_id(&self) -> std::any::TypeId {
        std::any::TypeId::of::<Self>()
    }

    fn type_name(&self) -> &'static str {
        "TextField"
    }

    fn build(&self) -> std::boxed::Box<dyn RenderNode> {
        std::boxed::Box::new(TextFieldRenderNode::new(self.clone()))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Default)]
struct TextFieldInteractionState {
    focused: bool,
    cursor_pos: usize,
}

pub struct TextFieldRenderNode {
    id: NodeId,
    parent: Option<NodeId>,
    view: TextField,
    buffer: Option<Buffer>,
    frame: Rect,
    interaction_state: TextFieldInteractionState,
    dirty_flags: DirtyFlags,
    last_known_text: String,
}

impl TextFieldRenderNode {
    pub fn new(view: TextField) -> Self {
        let last_known_text = view.text.get();

        Self {
            id: NodeId::new(),
            parent: None,
            view,
            buffer: None,
            frame: Rect::ZERO,
            interaction_state: TextFieldInteractionState::default(),
            dirty_flags: DirtyFlags::PAINT | DirtyFlags::LAYOUT,
            last_known_text,
        }
    }

    fn insert_char(&mut self, c: char) {
        let mut text = self.view.text.get();
        if self.interaction_state.cursor_pos < text.len() {
            text.insert(self.interaction_state.cursor_pos, c);
        } else {
            text.push(c);
        }
        self.interaction_state.cursor_pos += 1;
        self.view.text.set(text);
        self.mark_dirty(DirtyFlags::PAINT);
    }

    fn delete_char(&mut self) {
        let mut text = self.view.text.get();
        if self.interaction_state.cursor_pos > 0 && !text.is_empty() {
            text.remove(self.interaction_state.cursor_pos - 1);
            self.interaction_state.cursor_pos -= 1;
            self.view.text.set(text);
            self.mark_dirty(DirtyFlags::PAINT);
        }
    }
}

impl RenderNode for TextFieldRenderNode {
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
        std::any::TypeId::of::<TextField>()
    }

    fn type_name(&self) -> &'static str {
        "TextField"
    }

    fn try_update(&mut self, new_view: &dyn View) -> Option<UpdateResult> {
        new_view
            .as_any()
            .downcast_ref::<TextField>()
            .map(|new_field| {
                let current_text = self.view.text.get();
                let new_text = new_field.text.get();

                if current_text != new_text {
                    self.view = new_field.clone();
                    self.last_known_text = new_text;
                    Some(UpdateResult::Changed(DirtyFlags::PAINT))
                } else {
                    Some(UpdateResult::Unchanged)
                }
            })
            .flatten()
    }

    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        let height: f32 = 24.0;
        let width = self.view.width;

        let size = Size::new(
            width.clamp(constraints.min.width, constraints.max.width),
            height.clamp(constraints.min.height, constraints.max.height),
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

        let current_text = self.view.text.get();
        self.last_known_text = current_text.clone();

        self.buffer = Some(Buffer::new(self.frame.size));

        // Draw background
        let bg_color = if self.interaction_state.focused {
            [60, 60, 60, 255]
        } else {
            [50, 50, 50, 255]
        };

        self.buffer
            .as_mut()
            .unwrap()
            .fill_rect(self.frame, bg_color);

        // Draw border
        let border_color = if self.interaction_state.focused {
            [100, 150, 255, 255]
        } else {
            [100, 100, 100, 255]
        };

        let border_rect = Rect::new(Point::ZERO, self.frame.size);
        self.buffer
            .as_mut()
            .unwrap()
            .fill_rect(border_rect, border_color);

        // TODO: Render text content
        // For now, just show placeholder background

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
                    crate::event::MouseEventKind::Press => {
                        if self.frame.contains(e.position) {
                            self.interaction_state.focused = true;
                            self.mark_dirty(DirtyFlags::PAINT);
                        } else {
                            self.interaction_state.focused = false;
                            self.mark_dirty(DirtyFlags::PAINT);
                        }
                    }
                    _ => {}
                }
            }
            Event::Key(key) if ctx.phase == EventPhase::Target && self.interaction_state.focused => {
                // TODO: Handle keyboard input
                // For now, placeholder implementation
            }
            _ => {}
        }
    }

    fn mark_dirty(&mut self, flags: DirtyFlags) {
        self.dirty_flags |= flags;
    }

    fn is_dirty(&self) -> bool {
        let current_state = self.view.text.get();
        !self.dirty_flags.is_empty() || current_state != self.last_known_text
    }

    fn clear_dirty(&mut self) {
        self.dirty_flags = DirtyFlags::empty();
    }
}
