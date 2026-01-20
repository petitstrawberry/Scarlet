use crate::buffer::Buffer;
use crate::dirty::DirtyFlags;
use crate::event::{Event, EventContext, HitResult};
use crate::geometry::{Point, Rect, Size};
use crate::layout::{Alignment, LayoutConstraints};
use crate::node_id::NodeId;
use crate::traits::{RenderNode, UpdateResult, View};
use std::boxed::Box;
use std::vec::Vec;

#[derive(Clone)]
pub struct VStack {
    pub spacing: f32,
    pub alignment: Alignment,
}

impl VStack {
    pub fn new() -> Self {
        Self {
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

impl Default for VStack {
    fn default() -> Self {
        Self::new()
    }
}

impl View for VStack {
    fn type_id(&self) -> std::any::TypeId {
        std::any::TypeId::of::<Self>()
    }

    fn type_name(&self) -> &'static str {
        "VStack"
    }

    fn build(&self) -> Box<dyn RenderNode> {
        // Note: This is a simplified version that creates empty VStack
        // In real implementation, you'd want to pass children during construction
        Box::new(VStackRenderNode::new(
            Vec::new(),
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
                let child_constraints = LayoutConstraints {
                    min: Size::new(constraints.min.width, min_heights[i]),
                    max: Size::new(
                        constraints.max.width,
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
                let child_constraints = LayoutConstraints::tight(Size::new(
                    constraints.max.width,
                    child_height,
                ));
                child.layout(child_constraints);
            }
        }

        // Calculate total size
        let max_width = constraints.max.width;
        let total_height = min_total.min(available_for_children) + total_spacing;
        Size::new(max_width, total_height)
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

        let mut y_offset = 0.0;
        for child in &mut self.children {
            child.set_frame(Rect::new(Point::new(0.0, y_offset), child.frame().size));
            child.render();

            if let Some(child_buffer) = child.get_buffer() {
                self.buffer.as_mut().unwrap().blit_from(child_buffer, child.frame());
            }

            y_offset += child.frame().size.height + self.spacing;
        }

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
        // Events routed to children by dispatcher
        // Container can intercept in capture/bubble phase
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
