use crate::buffer::Buffer;
use crate::dirty::DirtyFlags;
use crate::event::{Event, EventContext, HitResult};
use crate::geometry::{Point, Rect, Size};
use crate::layout::{Alignment, LayoutConstraints};
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
pub struct HStack<V: View + Clone + IntoRenderNodes> {
    pub children: V,
    pub spacing: f32,
    pub alignment: Alignment,
}

impl<V: View + Clone + IntoRenderNodes> HStack<V> {
    pub fn new(children: V) -> Self {
        Self {
            children,
            spacing: 0.0,
            alignment: Alignment::Center,
        }
    }

    pub fn spacing(mut self, spacing: f32) -> Self {
        self.spacing = spacing;
        self
    }

    pub fn alignment(mut self, alignment: Alignment) -> Self {
        self.alignment = alignment;
        self
    }
}

impl<V: View + Clone + IntoRenderNodes> View for HStack<V> {
    fn type_id(&self) -> std::any::TypeId {
        std::any::TypeId::of::<Self>()
    }

    fn type_name(&self) -> &'static str {
        "HStack"
    }

    fn build(&self) -> Box<dyn RenderNode> {
        Box::new(HStackRenderNode::new(self.children.clone().into_nodes(), self.spacing))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct HStackRenderNode {
    id: NodeId,
    parent: Option<NodeId>,
    children: Vec<Box<dyn RenderNode>>,
    spacing: f32,
    alignment: Alignment,
    buffer: Option<Buffer>,
    frame: Rect,
    dirty_flags: DirtyFlags,
}

impl HStackRenderNode {
    pub fn new(mut children: Vec<Box<dyn RenderNode>>, spacing: f32) -> Self {
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
            alignment: Alignment::Center,
            buffer: None,
            frame: Rect::ZERO,
            dirty_flags: DirtyFlags::all(),
        }
    }
}

impl RenderNode for HStackRenderNode {
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
        std::any::TypeId::of::<HStackRenderNode>()
    }

    fn type_name(&self) -> &'static str {
        "HStack"
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

        // Clamp available width to reasonable maximum
        const MAX_WIDTH: f32 = 65536.0;
        const MAX_HEIGHT: f32 = 65536.0;
        let available_width = constraints.max.width.min(MAX_WIDTH);
        let available_height = constraints.max.height.min(MAX_HEIGHT);

        let total_spacing = self.spacing * (n - 1) as f32;
        let available_for_children = available_width - total_spacing;

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
        let mut non_spacer_width: f32 = 0.0;
        let mut max_height: f32 = 0.0;

        for (i, size) in intrinsic_sizes.iter().enumerate() {
            if self.children[i].type_id() == spacer_type_id {
                spacers.push(i);
            } else {
                non_spacer_width += size.width;
            }
            max_height = max_height.max(size.height);
        }

        // Pass 2: Layout children
        let spacer_count = spacers.len();
        if spacer_count > 0 {
            // Distribute remaining space to spacers
            let remaining = (available_for_children - non_spacer_width).max(0.0);
            let spacer_width = remaining / spacer_count as f32;

            for (i, child) in self.children.iter_mut().enumerate() {
                let is_spacer = spacers.contains(&i);
                let child_constraints = if is_spacer {
                    // Spacer stretches horizontally to fill space, keeps min vertical
                    LayoutConstraints {
                        min: Size::new(spacer_width, intrinsic_sizes[i].height),
                        max: Size::new(spacer_width, intrinsic_sizes[i].height),
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
        // If constraints are tight (min == max), use that size
        // Otherwise, use children's total size + spacing (unless spacers exist)
        let use_full_size = (constraints.max.width - constraints.min.width).abs() < 0.5
            && (constraints.max.height - constraints.min.height).abs() < 0.5;

        let total_width = if spacer_count > 0 || use_full_size {
            available_width
        } else {
            non_spacer_width + total_spacing
        };

        let total_height = if spacer_count > 0 || use_full_size {
            available_height
        } else {
            max_height
        };

        let size = Size::new(total_width.min(MAX_WIDTH), total_height.min(MAX_HEIGHT));

        // Set frames for children with actual positions (alignment + offset)
        let mut x_offset = 0.0;
        for child in self.children.iter_mut() {
            let child_size = child.frame().size;

            // Calculate y offset based on alignment
            let y_offset = match self.alignment {
                Alignment::Start => 0.0,
                Alignment::Center => (max_height - child_size.height) / 2.0,
                Alignment::End => max_height - child_size.height,
                Alignment::Stretch => 0.0,
            };

            // Set child frame at actual position
            child.set_frame(Rect::new(Point::new(x_offset, y_offset), child_size));
            x_offset += child_size.width + self.spacing;
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

        // Create buffer and composite children
        self.buffer = Some(Buffer::new(self.frame.size));

        for child in &mut self.children {
            // child.frame() already has correct origin from layout()
            child.render();

            if let Some(child_buffer) = child.get_buffer() {
                // Blit at child's frame position
                self.buffer.as_mut().unwrap().blit_from(child_buffer, child.frame());
            }
        }

        self.clear_dirty();
    }

    fn get_buffer(&self) -> Option<&Buffer> {
        self.buffer.as_ref()
    }

    fn get_buffer_mut(&mut self) -> Option<&mut Buffer> {
        self.buffer.as_mut()
    }

    fn hit_test(&self, point: Point) -> HitResult {
        // Check children in reverse order (z-order)
        for child in self.children.iter().rev() {
            let child_frame = child.frame();

            // child_frame.origin is already set correctly by layout()
            let in_bounds = child_frame.contains(point);

            if in_bounds {
                // Transform to child-local coordinates
                let local_point = point - child_frame.origin;
                match child.hit_test(local_point) {
                    HitResult::Handled(id) => return HitResult::Handled(id),
                    HitResult::Stop => return HitResult::Stop,
                    HitResult::Passthrough => continue,
                }
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
