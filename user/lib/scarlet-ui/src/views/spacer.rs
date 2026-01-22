use crate::dirty::DirtyFlags;
use crate::event::{Event, EventContext, HitResult};
use crate::geometry::{Point, Rect, Size};
use crate::layout::LayoutConstraints;
use crate::node_id::NodeId;
use crate::traits::{RenderNode, UpdateResult, View};
use std::any::Any;

/// Spacer - Takes up available space in layouts
///
/// When used in HStack: expands horizontally to fill available space
/// When used in VStack: expands vertically to fill available space
#[derive(Clone, PartialEq, Copy, Default)]
pub struct Spacer {
    pub min_width: f32,
    pub min_height: f32,
}

impl Spacer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn min_width(mut self, width: f32) -> Self {
        self.min_width = width;
        self
    }

    pub fn min_height(mut self, height: f32) -> Self {
        self.min_height = height;
        self
    }
}

impl View for Spacer {
    fn type_id(&self) -> std::any::TypeId {
        std::any::TypeId::of::<Self>()
    }

    fn type_name(&self) -> &'static str {
        "Spacer"
    }

    fn build(&self) -> std::boxed::Box<dyn RenderNode> {
        std::boxed::Box::new(SpacerRenderNode::new(*self))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct SpacerRenderNode {
    id: NodeId,
    parent: Option<NodeId>,
    view: Spacer,
    frame: Rect,
}

impl SpacerRenderNode {
    pub fn new(view: Spacer) -> Self {
        Self {
            id: NodeId::new(),
            parent: None,
            view,
            frame: Rect::ZERO,
        }
    }
}

impl RenderNode for SpacerRenderNode {
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
        std::any::TypeId::of::<Spacer>()
    }

    fn type_name(&self) -> &'static str {
        "Spacer"
    }

    fn try_update(&mut self, new_view: &dyn View) -> Option<UpdateResult> {
        new_view
            .as_any()
            .downcast_ref::<Spacer>()
            .map(|new_spacer| {
                if self.view != *new_spacer {
                    self.view = *new_spacer;
                    Some(UpdateResult::Changed(DirtyFlags::LAYOUT))
                } else {
                    Some(UpdateResult::Unchanged)
                }
            })
            .flatten()
    }

    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        // Spacer expands to fill available space (within reasonable limits)
        // Use a reasonable maximum to prevent overflow issues
        const MAX_SIZE: f32 = 65536.0;

        let max_width = constraints.max.width.min(MAX_SIZE);
        let max_height = constraints.max.height.min(MAX_SIZE);

        let size = Size::new(
            self.view.min_width.max(constraints.min.width).min(max_width),
            self.view.min_height.max(constraints.min.height).min(max_height),
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
        // Spacer doesn't render anything - no buffer created
    }

    fn get_buffer(&self) -> Option<&crate::buffer::Buffer> {
        None
    }

    fn get_buffer_mut(&mut self) -> Option<&mut crate::buffer::Buffer> {
        None
    }

    fn hit_test(&self, _point: Point) -> HitResult {
        HitResult::Passthrough
    }

    fn handle_event(&mut self, _event: &Event, _ctx: &mut EventContext) {
        // Spacer doesn't handle events
    }

    fn mark_dirty(&mut self, _flags: DirtyFlags) {
        // Spacer never becomes dirty
    }

    fn is_dirty(&self) -> bool {
        false
    }

    fn clear_dirty(&mut self) {
        // Spacer never becomes dirty
    }
}
