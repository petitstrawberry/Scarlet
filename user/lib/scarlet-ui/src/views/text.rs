use crate::buffer::Buffer;
use crate::dirty::DirtyFlags;
use crate::event::{Event, EventContext, HitResult};
use crate::geometry::{Point, Rect, Size};
use crate::graphics::{draw_text, measure_text};
use crate::layout::LayoutConstraints;
use crate::node_id::NodeId;
use crate::traits::{RenderObject, UpdateResult, View};
use crate::geometry::Color;
use crate::theme::with_theme;
use std::any::Any;
use std::string::String;
use std::println;

#[derive(Clone, PartialEq)]
pub struct Text {
    pub content: String,
    pub color: Color,
    pub size: f32,
}

impl Text {
    pub fn new(content: &str) -> Self {
        println!("[text] Text::new() called with content: {}", content);
        Self {
            content: String::from(content),
            color: with_theme(|theme| theme.text_primary),
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

    fn build(&self) -> std::boxed::Box<dyn RenderObject> {
        println!("[text] Text::build() called");
        std::boxed::Box::new(TextRenderObject::new(self.clone()))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct TextRenderObject {
    id: NodeId,
    parent: Option<NodeId>,
    view: Text,
    buffer: Option<Buffer>,
    frame: Rect,
    dirty_flags: DirtyFlags,
}

impl TextRenderObject {
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

impl RenderObject for TextRenderObject {
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
                    // Check if size changed
                    let old_size = self.estimated_size();
                    self.view = new_text.clone();
                    let new_size = self.estimated_size();

                    if old_size != new_size {
                        // Size changed, need layout
                        Some(UpdateResult::Changed(DirtyFlags::PAINT | DirtyFlags::LAYOUT))
                    } else {
                        // Only content changed, just repaint
                        Some(UpdateResult::Changed(DirtyFlags::PAINT))
                    }
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
        std::println!("[Text::layout] content: '{}', font_size: {}", self.view.content, self.view.size);

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

        std::println!("[Text::render] creating buffer with size: {:?}", self.frame.size);
        self.buffer = Some(Buffer::new(self.frame.size));

        // Use graphics module to draw text
        let buffer = self.buffer.as_mut().unwrap();
        let width = buffer.width();
        let height = buffer.height();
        let data = buffer.as_mut_slice();

        std::println!("[Text::render] drawing text '{}' at {}x{}", &self.view.content, width, height);

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

        std::println!("[Text::render] done rendering '{}'", &self.view.content);
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
