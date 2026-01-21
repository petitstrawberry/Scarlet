use crate::buffer::Buffer;
use crate::dirty::DirtyFlags;
use crate::event::{Event, EventContext, EventPhase, HitResult};
use crate::geometry::{Point, Rect, Size};
use crate::layout::Alignment;
use crate::layout::LayoutConstraints;
use crate::node_id::NodeId;
use crate::traits::{RenderNode, UpdateResult, View};
use std::any::Any;
use std::boxed::Box;
use std::vec::Vec;

/// Trait to convert tuples into RenderNodes
pub trait IntoRenderNodes {
    fn into_nodes(self) -> Vec<Box<dyn RenderNode>>;
}

// Macro to implement IntoRenderNodes for tuples of various sizes
macro_rules! impl_into_nodes {
    ($idx:tt: $ty:ident) => {
        impl<$ty: View + Clone> IntoRenderNodes for ($ty,) {
            fn into_nodes(self) -> Vec<Box<dyn RenderNode>> {
                let mut nodes = Vec::new();
                nodes.push(self.$idx.build());
                nodes
            }
        }
    };
    ($($idx:tt: $ty:ident),+) => {
        impl<$($ty: View + Clone),+> IntoRenderNodes for ($($ty,)+) {
            fn into_nodes(self) -> Vec<Box<dyn RenderNode>> {
                let mut nodes = Vec::new();
                $(
                    nodes.push(self.$idx.build());
                )+
                nodes
            }
        }
    };
}

// Implement for tuples of size 1-16
impl_into_nodes!(0: A);
impl_into_nodes!(0: A, 1: B);
impl_into_nodes!(0: A, 1: B, 2: C);
impl_into_nodes!(0: A, 1: B, 2: C, 3: D);
impl_into_nodes!(0: A, 1: B, 2: C, 3: D, 4: E);
impl_into_nodes!(0: A, 1: B, 2: C, 3: D, 4: E, 5: F);
impl_into_nodes!(0: A, 1: B, 2: C, 3: D, 4: E, 5: F, 6: G);
impl_into_nodes!(0: A, 1: B, 2: C, 3: D, 4: E, 5: F, 6: G, 7: H);
impl_into_nodes!(0: A, 1: B, 2: C, 3: D, 4: E, 5: F, 6: G, 7: H, 8: I);
impl_into_nodes!(0: A, 1: B, 2: C, 3: D, 4: E, 5: F, 6: G, 7: H, 8: I, 9: J);
impl_into_nodes!(0: A, 1: B, 2: C, 3: D, 4: E, 5: F, 6: G, 7: H, 8: I, 9: J, 10: K);
impl_into_nodes!(0: A, 1: B, 2: C, 3: D, 4: E, 5: F, 6: G, 7: H, 8: I, 9: J, 10: K, 11: L);
impl_into_nodes!(0: A, 1: B, 2: C, 3: D, 4: E, 5: F, 6: G, 7: H, 8: I, 9: J, 10: K, 11: L, 12: M);
impl_into_nodes!(0: A, 1: B, 2: C, 3: D, 4: E, 5: F, 6: G, 7: H, 8: I, 9: J, 10: K, 11: L, 12: M, 13: N);
impl_into_nodes!(0: A, 1: B, 2: C, 3: D, 4: E, 5: F, 6: G, 7: H, 8: I, 9: J, 10: K, 11: L, 12: M, 13: N, 14: O);
impl_into_nodes!(0: A, 1: B, 2: C, 3: D, 4: E, 5: F, 6: G, 7: H, 8: I, 9: J, 10: K, 11: L, 12: M, 13: N, 14: O, 15: P);

#[derive(Clone)]
pub struct ZStack<V: View + Clone + IntoRenderNodes> {
    pub children: V,
}

impl<V: View + Clone + IntoRenderNodes> ZStack<V> {
    pub fn new(children: V) -> Self {
        Self { children }
    }
}

impl<V: View + Clone + IntoRenderNodes> View for ZStack<V> {
    fn type_id(&self) -> std::any::TypeId {
        std::any::TypeId::of::<Self>()
    }

    fn type_name(&self) -> &'static str {
        "ZStack"
    }

    fn build(&self) -> Box<dyn RenderNode> {
        Box::new(ZStackRenderNode::new(self.children.clone().into_nodes()))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct ZStackRenderNode {
    id: NodeId,
    parent: Option<NodeId>,
    children: Vec<Box<dyn RenderNode>>,
    buffer: Option<Buffer>,
    frame: Rect,
    dirty_flags: DirtyFlags,
}

impl ZStackRenderNode {
    pub fn new(mut children: Vec<Box<dyn RenderNode>>) -> Self {
        let id = NodeId::new();

        // Set parent for each child
        for child in &mut children {
            child.set_parent(id);
        }

        Self {
            id,
            parent: None,
            children,
            buffer: None,
            frame: Rect::ZERO,
            dirty_flags: DirtyFlags::all(),
        }
    }
}

impl RenderNode for ZStackRenderNode {
    fn id(&self) -> NodeId {
        self.id
    }

    fn parent(&self) -> Option<NodeId> {
        self.parent
    }

    fn set_parent(&mut self, parent: NodeId) {
        self.parent = Some(parent);
    }

    fn children(&self) -> &[Box<dyn RenderNode>] {
        &self.children
    }

    fn children_mut(&mut self) -> &mut [Box<dyn RenderNode>] {
        &mut self.children
    }

    fn get_child(&self, id: NodeId) -> Option<&dyn RenderNode> {
        self.children.iter().find(|c| c.id() == id).map(|c| c.as_ref())
    }

    fn get_child_mut(&mut self, id: NodeId) -> Option<&mut (dyn RenderNode + '_)> {
        for child in self.children.iter_mut() {
            if child.id() == id {
                return Some(child.as_mut());
            }
        }
        None
    }

    fn type_id(&self) -> std::any::TypeId {
        std::any::TypeId::of::<ZStackRenderNode>()
    }

    fn type_name(&self) -> &'static str {
        "ZStack"
    }

    fn try_update(&mut self, _new_view: &dyn View) -> Option<UpdateResult> {
        // Containers always rebuild children on update
        Some(UpdateResult::Changed(DirtyFlags::CHILDREN))
    }

    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        if self.children.is_empty() {
            return Size::ZERO;
        }

        let size = Size::new(
            constraints.max.width,
            constraints.max.height,
        );

        // All children get the same size (the max available)
        for child in &mut self.children {
            child.layout(constraints);
            // Set child frame to Point::ZERO for consistency with VStack/HStack
            let child_size = child.frame().size;
            child.set_frame(Rect::new(Point::ZERO, child_size));
        }

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
        // NOTE: Don't check is_dirty() here!
        // Parent may call render() on us when children are dirty (even if we're not)
        // We need to blit children's buffers even if we're not dirty ourselves

        // Create buffer and composite children from back to front
        let mut buffer = Buffer::new(self.frame.size);

        for child in &mut self.children {
            // Don't modify child.frame - it's already Point::ZERO from layout()
            child.render();

            if let Some(child_buffer) = child.get_buffer() {
                // Blit child at Point::ZERO (all children in ZStack are at origin)
                let blit_rect = Rect::new(Point::ZERO, child.frame().size);
                buffer.blit_from(child_buffer, blit_rect);
            }
        }

        self.buffer = Some(buffer);
        self.frame = Rect::new(Point::ZERO, self.frame.size);
        self.clear_dirty();
    }

    fn get_buffer(&self) -> Option<&Buffer> {
        self.buffer.as_ref()
    }

    fn get_buffer_mut(&mut self) -> Option<&mut Buffer> {
        self.buffer.as_mut()
    }

    fn hit_test(&self, point: Point) -> HitResult {
        // Check children from front to back (reverse order)
        // All children are at Point::ZERO in ZStack
        for child in self.children.iter().rev() {
            // Child is at origin, so local_point is the same as point
            match child.hit_test(point) {
                HitResult::Handled(id) => return HitResult::Handled(id),
                HitResult::Stop => return HitResult::Stop,
                HitResult::Passthrough => continue,
            }
        }
        HitResult::Passthrough
    }

    fn handle_event(&mut self, _event: &Event, _ctx: &mut EventContext) {
        // Events are routed by the dispatcher through Capture/Target/Bubble phases
        // Containers don't redistribute events - dispatcher handles routing
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
