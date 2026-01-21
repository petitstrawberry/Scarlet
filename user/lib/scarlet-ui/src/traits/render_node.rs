use std::boxed::Box;

use crate::buffer::BufferRef;
use crate::dirty::DirtyFlags;
use crate::event::{Event, EventContext, HitResult};
use crate::geometry::{Point, Rect, Size};
use crate::layout::LayoutConstraints;
use crate::node_id::NodeId;
use crate::traits::view::View;

pub enum UpdateResult {
    Unchanged,
    Changed(DirtyFlags),
    Replaced(Box<dyn RenderNode>),
}

pub trait RenderNode {
    // Identity
    fn id(&self) -> NodeId;

    // Tree structure
    fn parent(&self) -> Option<NodeId>;
    fn set_parent(&mut self, parent: NodeId);

    fn children(&self) -> &[Box<dyn RenderNode>] {
        &[]
    }

    fn get_child(&self, id: NodeId) -> Option<&dyn RenderNode> {
        self.children().iter().find(|c| c.id() == id).map(|c| c.as_ref())
    }

    fn get_child_mut(&mut self, id: NodeId) -> Option<&mut (dyn RenderNode + '_)> {
        for child in self.children_mut().iter_mut() {
            if child.id() == id {
                return Some(child.as_mut());
            }
        }
        None
    }

    // Helper for default implementations
    fn children_mut(&mut self) -> &mut [Box<dyn RenderNode>] {
        &mut []
    }

    // Lifecycle
    fn render(&mut self);
    fn layout(&mut self, constraints: LayoutConstraints) -> Size;
    fn set_frame(&mut self, frame: Rect);
    fn frame(&self) -> Rect;

    // Update
    fn type_id(&self) -> std::any::TypeId;
    fn type_name(&self) -> &'static str;

    fn update(&mut self, new_view: &dyn View) -> UpdateResult {
        // Default implementation: TypeId check + try_update
        if self.type_id() != new_view.type_id() {
            return UpdateResult::Replaced(new_view.build());
        }
        self.try_update(new_view)
            .unwrap_or_else(|| UpdateResult::Replaced(new_view.build()))
    }

    fn try_update(&mut self, new_view: &dyn View) -> Option<UpdateResult>;

    // Event handling
    fn hit_test(&self, point: Point) -> HitResult;
    fn handle_event(&mut self, event: &Event, ctx: &mut EventContext);

    // Focus
    fn is_focusable(&self) -> bool {
        false
    }

    fn request_focus(&mut self) -> bool {
        false
    }

    fn lose_focus(&mut self) {}

    // Dirty tracking
    fn mark_dirty(&mut self, flags: DirtyFlags);
    fn is_dirty(&self) -> bool;
    fn clear_dirty(&mut self);

    // Buffer
    fn get_buffer(&self) -> Option<&BufferRef>;
    fn get_buffer_mut(&mut self) -> Option<&mut BufferRef>;
}
