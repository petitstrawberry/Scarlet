use crate::buffer::Buffer;
use crate::dirty::DirtyFlags;
use crate::event::{Event, EventContext, HitResult};
use crate::geometry::{Point, Rect, Size};
use crate::graphics::{draw_text, measure_text};
use crate::layout::LayoutConstraints;
use crate::node_id::NodeId;
use crate::traits::{RenderNode, UpdateResult, View};
use crate::geometry::Color;
use std::any::Any;
use std::string::String;

#[derive(Clone, PartialEq)]
pub struct Text {
    pub content: String,
    pub color: Color,
    pub size: f32,
}

impl Text {
    pub fn new(content: &str) -> Self {
        Self {
            content: String::from(content),
            color: Color::rgb(255, 255, 255),
            size: 16.0,
        }
    }

    pub fn color(mut self, color: Color) -> Self {
        self.color = color;
        self
    }

    pub fn size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }
}

impl View for Text {
    fn type_id(&self) -> std::any::TypeId {
        std::any::TypeId::of::<Self>()
    }

    fn type_name(&self) -> &'static str {
        "Text"
    }

    fn build(&self) -> std::boxed::Box<dyn RenderNode> {
        std::boxed::Box::new(TextRenderNode::new(self.clone()))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct TextRenderNode {
    id: NodeId,
    parent: Option<NodeId>,
    view: Text,
    buffer: Option<Buffer>,
    frame: Rect,
    dirty_flags: DirtyFlags,
}

impl TextRenderNode {
    pub fn new(view: Text) -> Self {
        Self {
            id: NodeId::new(),
            parent: None,
            view,
            buffer: None,
            frame: Rect::ZERO,
            dirty_flags: DirtyFlags::PAINT | DirtyFlags::LAYOUT,
        }
    }

    fn estimated_size(&self) -> Size {
        let (width, height) = measure_text(&self.view.content, self.view.size);
        Size::new(width as f32, height as f32)
    }
}

impl RenderNode for TextRenderNode {
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
        std::any::TypeId::of::<Text>()
    }

    fn type_name(&self) -> &'static str {
        "Text"
    }

    fn try_update(&mut self, new_view: &dyn View) -> Option<UpdateResult> {
        new_view
            .as_any()
            .downcast_ref::<Text>()
            .map(|new_text| {
                if self.view != *new_text {
                    self.view = new_text.clone();
                    Some(UpdateResult::Changed(DirtyFlags::PAINT))
                } else {
                    Some(UpdateResult::Unchanged)
                }
            })
            .flatten()
    }

    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        let estimated = self.estimated_size();
        std::println!("[Text::layout] estimated: {:?}", estimated);
        std::println!("[Text::layout] constraints: {:?} {:?}", constraints.min, constraints.max);

        let size = Size::new(
            estimated.width.clamp(constraints.min.width, constraints.max.width),
            estimated.height.clamp(constraints.min.height, constraints.max.height),
        );
        std::println!("[Text::layout] final size: {:?}", size);

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

        self.buffer = Some(Buffer::new(self.frame.size));

        // Use graphics module to draw text
        let buffer = self.buffer.as_mut().unwrap();
        let width = buffer.width();
        let height = buffer.height();
        let data = buffer.as_mut_slice();

        draw_text(
            data,
            width,
            height,
            &self.view.content,
            0,
            0,
            self.view.size,
            self.view.color.as_bgra(),
        );

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

    fn handle_event(&mut self, _event: &Event, _ctx: &mut EventContext) {
        // Text doesn't handle events
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
