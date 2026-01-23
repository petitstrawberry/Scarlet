use crate::buffer::Buffer;
use crate::dirty::DirtyFlags;
use crate::event::{Event, EventContext, EventPhase, HitResult};
use crate::geometry::{Point, Rect, Size};
use crate::layout::Alignment;
use crate::layout::LayoutConstraints;
use crate::node_id::NodeId;
use crate::traits::{RenderObject, UpdateResult, View};
use std::any::Any;
use std::boxed::Box;
use std::vec::Vec;

/// Trait to convert tuples into child Views
pub trait IntoChildViews {
    fn into_views(self) -> Vec<Box<dyn View>>;
}

/// Trait to convert tuples into RenderObjects
pub trait IntoRenderObjects {
    fn into_nodes(self) -> Vec<Box<dyn RenderObject>>;
}

// Macro to implement IntoRenderObjects for tuples of various sizes
macro_rules! impl_into_nodes {
    ($idx:tt: $ty:ident) => {
        impl<$ty: View + Clone> IntoRenderObjects for ($ty,) {
            fn into_nodes(self) -> Vec<Box<dyn RenderObject>> {
                let mut nodes = Vec::new();
                nodes.push(self.$idx.build());
                nodes
            }
        }
    };
    ($($idx:tt: $ty:ident),+) => {
        impl<$($ty: View + Clone),+> IntoRenderObjects for ($($ty,)+) {
            fn into_nodes(self) -> Vec<Box<dyn RenderObject>> {
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

// Macro to implement IntoChildViews for tuples of various sizes
macro_rules! impl_into_child_views {
    ($idx:tt: $ty:ident) => {
        impl<$ty: View + Clone> IntoChildViews for ($ty,) {
            fn into_views(self) -> Vec<Box<dyn View>> {
                let mut views: Vec<Box<dyn View>> = Vec::new();
                views.push(Box::new(self.$idx) as Box<dyn View>);
                views
            }
        }
    };
    ($($idx:tt: $ty:ident),+) => {
        impl<$($ty: View + Clone),+> IntoChildViews for ($($ty,)+) {
            fn into_views(self) -> Vec<Box<dyn View>> {
                let mut views: Vec<Box<dyn View>> = Vec::new();
                $(
                    views.push(Box::new(self.$idx) as Box<dyn View>);
                )+
                views
            }
        }
    };
}

impl_into_child_views!(0: A);
impl_into_child_views!(0: A, 1: B);
impl_into_child_views!(0: A, 1: B, 2: C);
impl_into_child_views!(0: A, 1: B, 2: C, 3: D);
impl_into_child_views!(0: A, 1: B, 2: C, 3: D, 4: E);
impl_into_child_views!(0: A, 1: B, 2: C, 3: D, 4: E, 5: F);
impl_into_child_views!(0: A, 1: B, 2: C, 3: D, 4: E, 5: F, 6: G);
impl_into_child_views!(0: A, 1: B, 2: C, 3: D, 4: E, 5: F, 6: G, 7: H);
impl_into_child_views!(0: A, 1: B, 2: C, 3: D, 4: E, 5: F, 6: G, 7: H, 8: I);
impl_into_child_views!(0: A, 1: B, 2: C, 3: D, 4: E, 5: F, 6: G, 7: H, 8: I, 9: J);
impl_into_child_views!(0: A, 1: B, 2: C, 3: D, 4: E, 5: F, 6: G, 7: H, 8: I, 9: J, 10: K);
impl_into_child_views!(0: A, 1: B, 2: C, 3: D, 4: E, 5: F, 6: G, 7: H, 8: I, 9: J, 10: K, 11: L);
impl_into_child_views!(0: A, 1: B, 2: C, 3: D, 4: E, 5: F, 6: G, 7: H, 8: I, 9: J, 10: K, 11: L, 12: M);
impl_into_child_views!(0: A, 1: B, 2: C, 3: D, 4: E, 5: F, 6: G, 7: H, 8: I, 9: J, 10: K, 11: L, 12: M, 13: N);
impl_into_child_views!(0: A, 1: B, 2: C, 3: D, 4: E, 5: F, 6: G, 7: H, 8: I, 9: J, 10: K, 11: L, 12: M, 13: N, 14: O);
impl_into_child_views!(0: A, 1: B, 2: C, 3: D, 4: E, 5: F, 6: G, 7: H, 8: I, 9: J, 10: K, 11: L, 12: M, 13: N, 14: O, 15: P);

#[derive(Clone)]
pub struct ZStack<V: View + Clone + IntoChildViews + IntoRenderObjects> {
    pub children: V,
}

impl<V: View + Clone + IntoChildViews + IntoRenderObjects> ZStack<V> {
    pub fn new(children: V) -> Self {
        Self { children }
    }
}

impl<V: View + Clone + IntoChildViews + IntoRenderObjects> View for ZStack<V> {
    fn type_id(&self) -> std::any::TypeId {
        std::any::TypeId::of::<Self>()
    }

    fn type_name(&self) -> &'static str {
        "ZStack"
    }

    fn build(&self) -> Box<dyn RenderObject> {
        Box::new(ZStackRenderObject::new(self.children.clone().into_nodes()))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn children(&self) -> Vec<Box<dyn View>> {
        self.children.clone().into_views()
    }
}

pub struct ZStackRenderObject {
    id: NodeId,
    parent: Option<NodeId>,
    children: Vec<Box<dyn RenderObject>>,
    buffer: Option<Buffer>,
    frame: Rect,
    dirty_flags: DirtyFlags,
}

impl ZStackRenderObject {
    pub fn new(mut children: Vec<Box<dyn RenderObject>>) -> Self {
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

impl RenderObject for ZStackRenderObject {
    fn id(&self) -> NodeId {
        self.id
    }

    fn parent(&self) -> Option<NodeId> {
        self.parent
    }

    fn set_parent(&mut self, parent: NodeId) {
        self.parent = Some(parent);
    }

    fn children(&self) -> &[Box<dyn RenderObject>] {
        &self.children
    }

    fn children_mut(&mut self) -> &mut [Box<dyn RenderObject>] {
        &mut self.children
    }

    fn get_child(&self, id: NodeId) -> Option<&dyn RenderObject> {
        self.children.iter().find(|c| c.id() == id).map(|c| c.as_ref())
    }

    fn get_child_mut(&mut self, id: NodeId) -> Option<&mut (dyn RenderObject + '_)> {
        for child in self.children.iter_mut() {
            if child.id() == id {
                return Some(child.as_mut());
            }
        }
        None
    }

    fn type_id(&self) -> std::any::TypeId {
        std::any::TypeId::of::<ZStackRenderObject>()
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
        // Containers don't have their own buffer
        // Just render children, SceneBuilder will handle compositing
        for child in self.children.iter_mut() {
            child.render();
        }
        self.clear_dirty();
    }

    fn get_buffer(&self) -> Option<&Buffer> {
        // Containers don't have their own buffer
        None
    }

    fn get_buffer_mut(&mut self) -> Option<&mut Buffer> {
        // Containers don't have their own buffer
        None
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
