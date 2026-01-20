use crate::buffer::Buffer;
use crate::dirty::DirtyFlags;
use crate::event::{Event, EventContext, HitResult};
use crate::geometry::{Point, Rect, Size};
use crate::layout::{Alignment, LayoutConstraints};
use crate::node_id::NodeId;
use crate::traits::{RenderNode, UpdateResult, View};
use std::any::TypeId;
use std::boxed::Box;
use std::vec;
use std::vec::Vec;
use std::println;

/// Trait to convert tuple of Views into Vec of RenderNodes
pub trait IntoRenderNodes {
    fn into_nodes(self) -> Vec<Box<dyn RenderNode>>;
}

// Implement View for tuples
macro_rules! impl_view_for_tuple {
    ($($idx:tt: $ty:ident),*) => {
        impl<$($ty: View + Clone),*> View for ($($ty),*) {
            fn type_id(&self) -> TypeId {
                TypeId::of::<Self>()
            }

            fn type_name(&self) -> &'static str {
                "TupleView"
            }

            fn build(&self) -> Box<dyn RenderNode> {
                // Build tuple view by creating a placeholder
                // This shouldn't be called directly, use VStack instead
                let mut nodes = Vec::new();
                $(nodes.push(self.$idx.clone().build());)*
                // For now, create a dummy VStack
                Box::new(VStackRenderNode::new(nodes, 0.0, Alignment::Center))
            }

            fn as_any(&self) -> &dyn std::any::Any {
                self
            }
        }
    }
}

impl_view_for_tuple!(0: A, 1: B);
impl_view_for_tuple!(0: A, 1: B, 2: C);
impl_view_for_tuple!(0: A, 1: B, 2: C, 3: D);
impl_view_for_tuple!(0: A, 1: B, 2: C, 3: D, 4: E);
impl_view_for_tuple!(0: A, 1: B, 2: C, 3: D, 4: E, 5: F);
impl_view_for_tuple!(0: A, 1: B, 2: C, 3: D, 4: E, 5: F, 6: G);
impl_view_for_tuple!(0: A, 1: B, 2: C, 3: D, 4: E, 5: F, 6: G, 7: H);
impl_view_for_tuple!(0: A, 1: B, 2: C, 3: D, 4: E, 5: F, 6: G, 7: H, 8: I);
impl_view_for_tuple!(0: A, 1: B, 2: C, 3: D, 4: E, 5: F, 6: G, 7: H, 8: I, 9: J);

// Implement for tuples up to 10 elements
macro_rules! impl_into_nodes {
    ($($idx:tt: $ty:ident),*) => {
        impl<$($ty: View + Clone),*> IntoRenderNodes for ($($ty),*) {
            fn into_nodes(self) -> Vec<Box<dyn RenderNode>> {
                let mut nodes = Vec::new();
                $(nodes.push(self.$idx.build());)*
                nodes
            }
        }
    }
}

impl_into_nodes!(0: A, 1: B);
impl_into_nodes!(0: A, 1: B, 2: C);
impl_into_nodes!(0: A, 1: B, 2: C, 3: D);
impl_into_nodes!(0: A, 1: B, 2: C, 3: D, 4: E);
impl_into_nodes!(0: A, 1: B, 2: C, 3: D, 4: E, 5: F);
impl_into_nodes!(0: A, 1: B, 2: C, 3: D, 4: E, 5: F, 6: G);
impl_into_nodes!(0: A, 1: B, 2: C, 3: D, 4: E, 5: F, 6: G, 7: H);
impl_into_nodes!(0: A, 1: B, 2: C, 3: D, 4: E, 5: F, 6: G, 7: H, 8: I);
impl_into_nodes!(0: A, 1: B, 2: C, 3: D, 4: E, 5: F, 6: G, 7: H, 8: I, 9: J);

#[derive(Clone)]
pub struct VStack<V: View + Clone> {
    pub children: V,
    pub spacing: f32,
    pub alignment: Alignment,
}

impl<V: View + Clone> VStack<V> {
    pub fn new(children: V) -> Self {
        println!("[vstack] VStack::new() called");
        Self {
            children,
            spacing: 0.0,
            alignment: Alignment::Center,
        }
    }

    pub fn spacing(mut self, spacing: f32) -> Self {
        println!("[vstack] VStack::spacing({}) called", spacing);
        self.spacing = spacing;
        self
    }

    pub fn alignment(mut self, alignment: Alignment) -> Self {
        println!("[vstack] VStack::alignment() called");
        self.alignment = alignment;
        self
    }
}

impl<V: View + Clone + Default> Default for VStack<V> {
    fn default() -> Self {
        Self::new(V::default())
    }
}

impl<V: View + Clone + IntoRenderNodes> View for VStack<V> {
    fn type_id(&self) -> TypeId {
        TypeId::of::<Self>()
    }

    fn type_name(&self) -> &'static str {
        "VStack"
    }

    fn build(&self) -> Box<dyn RenderNode> {
        println!("[vstack] VStack::build() called");
        let children = self.children.clone().into_nodes();
        println!("[vstack] into_nodes() returned {} children", children.len());

        Box::new(VStackRenderNode::new(
            children,
            self.spacing,
            self.alignment,
        ))
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

pub struct VStackRenderNode {
    id: NodeId,
    parent: Option<NodeId>,
    children: Vec<Box<dyn RenderNode>>,
    spacing: f32,
    alignment: Alignment,
    buffer: Option<Buffer>,
    frame: Rect,
    dirty_flags: DirtyFlags,
}

impl VStackRenderNode {
    pub fn new(mut children: Vec<Box<dyn RenderNode>>, spacing: f32, alignment: Alignment) -> Self {
        let id = NodeId::new();

        // Set parent for each child
        for child in &mut children {
            child.set_parent(id);
        }

        Self {
            id,
            parent: None,
            children,
            spacing,
            alignment,
            buffer: None,
            frame: Rect::ZERO,
            dirty_flags: DirtyFlags::all(),
        }
    }

    fn get_child_index(&self, id: NodeId) -> Option<usize> {
        self.children.iter().position(|c| c.id() == id)
    }
}

impl RenderNode for VStackRenderNode {
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
        self.get_child_index(id)
            .and_then(|i| self.children.get(i))
            .map(|b| b.as_ref())
    }

    fn get_child_mut(&mut self, id: NodeId) -> Option<&mut (dyn RenderNode + '_)> {
        let index = self.get_child_index(id)?;
        for (i, child) in self.children_mut().iter_mut().enumerate() {
            if i == index {
                return Some(child.as_mut());
            }
        }
        None
    }

    fn type_id(&self) -> std::any::TypeId {
        std::any::TypeId::of::<VStackRenderNode>()
    }

    fn type_name(&self) -> &'static str {
        "VStack"
    }

    fn try_update(&mut self, _new_view: &dyn View) -> Option<UpdateResult> {
        // Containers always rebuild children on update
        Some(UpdateResult::Changed(DirtyFlags::CHILDREN))
    }

    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        let n = self.children.len();
        if n == 0 {
            return Size::ZERO;
        }

        let available_height = constraints.max.height;
        let total_spacing = self.spacing * (n - 1) as f32;
        let available_for_children = available_height - total_spacing;

        // Pass 1: Measure minimum requirements (loose constraints)
        let loose_constraints = LayoutConstraints::loose(constraints.max);
        let min_heights: Vec<f32> = self
            .children
            .iter_mut()
            .map(|c| c.layout(loose_constraints).height)
            .collect();

        let min_total: f32 = min_heights.iter().sum();

        // Pass 2: Distribute remaining space or clamp
        if min_total <= available_for_children {
            // Space available: distribute to expandable children
            let remaining = available_for_children - min_total;
            let per_child = if n > 0 {
                remaining / n as f32
            } else {
                0.0
            };

            for (i, child) in self.children.iter_mut().enumerate() {
                // Ensure width is set to the available width
                let child_width = constraints.max.width;
                let child_constraints = LayoutConstraints {
                    min: Size::new(child_width, min_heights[i]),
                    max: Size::new(
                        child_width,
                        min_heights[i] + per_child,
                    ),
                };
                child.layout(child_constraints);
            }
        } else {
            // Not enough space: clamp proportionally
            for (i, child) in self.children.iter_mut().enumerate() {
                let ratio = if min_total > 0.0 {
                    min_heights[i] / min_total
                } else {
                    1.0 / n as f32
                };
                let child_height = (available_for_children * ratio).min(min_heights[i]);
                // Use tight constraints for width to ensure it's set correctly
                let child_width = constraints.max.width;
                let child_constraints = LayoutConstraints {
                    min: Size::new(child_width, 0.0),
                    max: Size::new(child_width, child_height),
                };
                child.layout(child_constraints);
            }
        }

        // Calculate total size
        let max_width = constraints.max.width;
        let total_height = min_total.min(available_for_children) + total_spacing;
        let size = Size::new(max_width, total_height);

        // Set frame
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

        println!("[vstack] VStackRenderNode::render() frame.size: {:?}", self.frame.size);
        self.buffer = Some(Buffer::new(self.frame.size));

        let mut y_offset = 0.0;
        for (i, child) in self.children.iter_mut().enumerate() {
            let child_size = child.frame().size;
            println!("[vstack] child[{}] size before set_frame: {:?}", i, child_size);
            child.set_frame(Rect::new(Point::new(0.0, y_offset), child_size));
            println!("[vstack] child[{}] frame after set_frame: {:?}", i, child.frame());
            child.render();

            if let Some(child_buffer) = child.get_buffer() {
                println!("[vstack] child[{}] buffer size: {:?}", i, child_buffer.size());
                println!("[vstack] blitting child[{}] at frame: {:?}", i, child.frame());
                self.buffer.as_mut().unwrap().blit_from(child_buffer, child.frame());
            } else {
                println!("[vstack] WARNING: child[{}] has no buffer!", i);
            }

            y_offset += child.frame().size.height + self.spacing;
        }

        println!("[vstack] VStackRenderNode::render() done");
        self.clear_dirty();
    }

    fn get_buffer(&self) -> Option<&Buffer> {
        self.buffer.as_ref()
    }

    fn hit_test(&self, point: Point) -> HitResult {
        for child in self.children.iter().rev() {
            let local_point = point - child.frame().origin;
            match child.hit_test(local_point) {
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
