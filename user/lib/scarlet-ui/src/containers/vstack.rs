use crate::buffer::Buffer;
use crate::dirty::DirtyFlags;
use crate::event::{Event, EventContext, HitResult};
use crate::geometry::{Point, Rect, Size};
use crate::layout::{Alignment, LayoutConstraints};
use crate::node_id::NodeId;
use crate::traits::{RenderNode, UpdateResult, View};
use std::any::TypeId;
use std::boxed::Box;
use std::vec::Vec;

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
        // println!("[vstack] VStack::new() called");
        Self {
            children,
            spacing: 0.0,
            alignment: Alignment::Center,
        }
    }

    pub fn spacing(mut self, spacing: f32) -> Self {
        // println!("[vstack] VStack::spacing({}) called", spacing);
        self.spacing = spacing;
        self
    }

    pub fn alignment(mut self, alignment: Alignment) -> Self {
        // println!("[vstack] VStack::alignment() called");
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
        // println!("[vstack] VStack::build() called");
        let children = self.children.clone().into_nodes();
        // println!("[vstack] into_nodes() returned {} children", children.len());

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
        // println!("[vstack] layout() called, constraints={:?}, children.len()={}", constraints, self.children.len());
        let n = self.children.len();
        if n == 0 {
            return Size::ZERO;
        }

        // Clamp available height to reasonable maximum
        const MAX_WIDTH: f32 = 65536.0;
        const MAX_HEIGHT: f32 = 65536.0;
        let available_height = constraints.max.height.min(MAX_HEIGHT);
        let available_width = constraints.max.width.min(MAX_WIDTH);

        let total_spacing = self.spacing * (n - 1) as f32;
        let available_for_children = available_height - total_spacing;

        // Pass 1: Measure intrinsic sizes with loose constraints
        let loose_constraints = LayoutConstraints::loose(Size::new(f32::MAX, f32::MAX));
        let mut intrinsic_sizes: Vec<Size> = self
            .children
            .iter_mut()
            .map(|c| c.layout(loose_constraints))
            .collect();

        // Identify spacers and calculate dimensions
        let spacer_type_id = std::any::TypeId::of::<crate::views::Spacer>();
        let mut spacers: Vec<usize> = Vec::new();
        let mut non_spacer_height: f32 = 0.0;
        let mut max_width: f32 = 0.0;

        for (i, size) in intrinsic_sizes.iter().enumerate() {
            if self.children[i].type_id() == spacer_type_id {
                spacers.push(i);
            } else {
                non_spacer_height += size.height;
            }
            max_width = max_width.max(size.width);
        }

        // Clamp width to available
        max_width = max_width.min(available_width);

        // Pass 2: Layout children
        let spacer_count = spacers.len();
        if spacer_count > 0 {
            // Distribute remaining space to spacers
            let remaining = (available_for_children - non_spacer_height).max(0.0);
            let spacer_height = remaining / spacer_count as f32;

            for (i, child) in self.children.iter_mut().enumerate() {
                let is_spacer = spacers.contains(&i);
                let child_constraints = if is_spacer {
                    // Spacer stretches vertically to fill space, keeps min horizontal
                    LayoutConstraints {
                        min: Size::new(intrinsic_sizes[i].width, spacer_height),
                        max: Size::new(intrinsic_sizes[i].width, spacer_height),
                    }
                } else {
                    // Non-spacer keeps intrinsic size
                    LayoutConstraints::tight(intrinsic_sizes[i])
                };
                child.layout(child_constraints);
            }
        } else {
            // No spacers: all children keep intrinsic size
            for (i, child) in self.children.iter_mut().enumerate() {
                child.layout(LayoutConstraints::tight(intrinsic_sizes[i]));
            }
        }

        // Calculate total size
        let total_height = if spacer_count > 0 {
            available_for_children + total_spacing
        } else {
            non_spacer_height.min(available_for_children) + total_spacing
        };

        let size = Size::new(max_width, total_height.min(MAX_HEIGHT));

        // Set frame
        self.frame = Rect::new(Point::ZERO, size);

        // Set frames for children with actual positions (alignment + offset)
        let mut y_offset = 0.0;
        for (i, child) in self.children.iter_mut().enumerate() {
            let child_size = child.frame().size;

            // Calculate x offset based on alignment
            let x_offset = match self.alignment {
                Alignment::Start => 0.0,
                Alignment::Center => (self.frame.size.width - child_size.width) / 2.0,
                Alignment::End => self.frame.size.width - child_size.width,
                Alignment::Stretch => 0.0,
            };

            // Set child frame at actual position
            let child_frame = Rect::new(Point::new(x_offset, y_offset), child_size);
            child.set_frame(child_frame);
            y_offset += child_size.height + self.spacing;
        }

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

        // println!("[vstack] render() called, self.frame={:?}", self.frame);
        self.buffer = Some(Buffer::new(self.frame.size));

        for (i, child) in self.children.iter_mut().enumerate() {
            // child.frame() already has correct origin from layout()
            // println!("[vstack] child[{}] frame: {:?}", i, child.frame());
            child.render();

            if let Some(child_buffer) = child.get_buffer() {
                // println!("[vstack] child[{}] buffer size: {:?}", i, child_buffer.size());
                // Blit at child's frame position
                self.buffer.as_mut().unwrap().blit_from(child_buffer, child.frame());
            } else {
                // println!("[vstack] WARNING: child[{}] has no buffer!", i);
            }
        }

        // println!("[vstack] render() done");
        self.clear_dirty();
    }

    fn get_buffer(&self) -> Option<&Buffer> {
        self.buffer.as_ref()
    }

    fn get_buffer_mut(&mut self) -> Option<&mut Buffer> {
        self.buffer.as_mut()
    }

    fn hit_test(&self, point: Point) -> HitResult {
        // println!("[vstack] hit_test: point={:?}, frame={:?}, children={}", point, self.frame, self.children.len());

        // Check children in reverse order (z-order)
        for (i, child) in self.children.iter().rev().enumerate() {
            let child_index = self.children.len() - 1 - i;  // Actual index in children Vec
            let child_frame = child.frame();

            // println!("[vstack] hit_test: child[{}] type={}, frame={:?}",
            //     child_index, child.type_name(), child_frame);

            // child_frame.origin is already set correctly by layout()
            let in_bounds = child_frame.contains(point);

            // println!("[vstack] hit_test: child[{}] in_bounds={}", child_index, in_bounds);

            if in_bounds {
                // Transform to child-local coordinates
                let local_point = point - child_frame.origin;
                match child.hit_test(local_point) {
                    HitResult::Handled(id) => {
                        // println!("[vstack] hit_test: child[{}] Handled by id={:?}", child_index, id);
                        return HitResult::Handled(id);
                    }
                    HitResult::Stop => {
                        // println!("[vstack] hit_test: child[{}] Stop", child_index);
                        return HitResult::Stop;
                    }
                    HitResult::Passthrough => {
                        // println!("[vstack] hit_test: child[{}] Passthrough", child_index);
                        continue;
                    }
                }
            }
        }

        // println!("[vstack] hit_test: all children passed through, returning Passthrough");
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
