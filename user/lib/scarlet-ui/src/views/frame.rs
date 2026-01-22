use crate::buffer::Buffer;
use crate::dirty::DirtyFlags;
use crate::event::{Event, EventContext, HitResult};
use crate::geometry::{Point, Rect, Size};
use crate::layout::{Alignment, LayoutConstraints};
use crate::node_id::NodeId;
use crate::traits::{RenderObject, UpdateResult, View};
use std::any::Any;

/// Frame modifier - controls size constraints of child view
///
/// Similar to SwiftUI's .frame() modifier:
/// - Fixed size: .frame(width: 100, height: 50)
/// - Max size: .frame(maxWidth: 400, maxHeight: 300)
/// - Fill parent: .frame_max() (takes parent's max size)
#[derive(Clone)]
pub struct Frame<V: View + Clone> {
    pub child: V,
    pub width: Option<f32>,
    pub height: Option<f32>,
    pub max_width: Option<f32>,
    pub max_height: Option<f32>,
    pub fill_parent: bool,
}

impl<V: View + Clone> Frame<V> {
    pub fn new(child: V) -> Self {
        Self {
            child,
            width: None,
            height: None,
            max_width: None,
            max_height: None,
            fill_parent: false,
        }
    }

    pub fn width(mut self, width: f32) -> Self {
        self.width = Some(width);
        self
    }

    pub fn height(mut self, height: f32) -> Self {
        self.height = Some(height);
        self
    }

    pub fn max_width(mut self, max_width: f32) -> Self {
        self.max_width = Some(max_width);
        self
    }

    pub fn max_height(mut self, max_height: f32) -> Self {
        self.max_height = Some(max_height);
        self
    }

    pub fn fill_parent(mut self) -> Self {
        self.fill_parent = true;
        self
    }
}

impl<V: View + Clone> View for Frame<V> {
    fn type_id(&self) -> std::any::TypeId {
        std::any::TypeId::of::<Self>()
    }

    fn type_name(&self) -> &'static str {
        "Frame"
    }

    fn build(&self) -> std::boxed::Box<dyn RenderObject> {
        std::boxed::Box::new(FrameRenderObject::new(self.clone()))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct FrameRenderObject {
    id: NodeId,
    parent: Option<NodeId>,
    child: std::boxed::Box<dyn RenderObject>,
    width: Option<f32>,
    height: Option<f32>,
    max_width: Option<f32>,
    max_height: Option<f32>,
    fill_parent: bool,
    buffer: Option<Buffer>,
    frame: Rect,
    dirty_flags: DirtyFlags,
}

impl FrameRenderObject {
    pub fn new(view: Frame<impl View + Clone>) -> Self {
        let child = view.child.build();
        let id = NodeId::new();

        let mut child_boxed = child;
        child_boxed.set_parent(id);

        Self {
            id,
            parent: None,
            child: child_boxed,
            width: view.width,
            height: view.height,
            max_width: view.max_width,
            max_height: view.max_height,
            fill_parent: view.fill_parent,
            buffer: None,
            frame: Rect::ZERO,
            dirty_flags: DirtyFlags::all(),
        }
    }
}

impl RenderObject for FrameRenderObject {
    fn id(&self) -> NodeId {
        self.id
    }

    fn parent(&self) -> Option<NodeId> {
        self.parent
    }

    fn set_parent(&mut self, parent: NodeId) {
        self.parent = Some(parent);
    }

    fn children(&self) -> &[std::boxed::Box<dyn RenderObject>] {
        std::slice::from_ref(&self.child)
    }

    fn children_mut(&mut self) -> &mut [std::boxed::Box<dyn RenderObject>] {
        std::slice::from_mut(&mut self.child)
    }

    fn get_child(&self, id: NodeId) -> Option<&dyn RenderObject> {
        if self.child.id() == id {
            Some(self.child.as_ref())
        } else {
            self.child.get_child(id)
        }
    }

    fn get_child_mut(&mut self, id: NodeId) -> Option<&mut (dyn RenderObject + '_)> {
        if self.child.id() == id {
            Some(self.child.as_mut())
        } else {
            self.child.get_child_mut(id)
        }
    }

    fn type_id(&self) -> std::any::TypeId {
        std::any::TypeId::of::<FrameRenderObject>()
    }

    fn type_name(&self) -> &'static str {
        "Frame"
    }

    fn try_update(&mut self, _new_view: &dyn View) -> Option<UpdateResult> {
        Some(UpdateResult::Changed(DirtyFlags::CHILDREN))
    }

    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        // Transform constraints based on frame parameters
        let new_constraints = if self.fill_parent {
            LayoutConstraints {
                min: constraints.min,
                max: constraints.max,
            }
        } else if let Some(width) = self.width {
            LayoutConstraints::tight(Size::new(
                width.clamp(constraints.min.width, constraints.max.width),
                self.height.map_or(constraints.max.height, |h| h.clamp(constraints.min.height, constraints.max.height)),
            ))
        } else if let Some(max_w) = self.max_width {
            LayoutConstraints {
                min: constraints.min,
                max: Size::new(
                    max_w.min(constraints.max.width),
                    self.max_height.map_or(constraints.max.height, |h| h.min(constraints.max.height)),
                ),
            }
        } else {
            constraints
        };

        // Layout child with transformed constraints
        let child_size = self.child.layout(new_constraints);
        self.child.set_frame(Rect::new(Point::ZERO, child_size));

        // Frame takes child's size
        self.frame = Rect::new(Point::ZERO, child_size);
        child_size
    }

    fn set_frame(&mut self, frame: Rect) {
        self.frame = frame;
    }

    fn frame(&self) -> Rect {
        self.frame
    }

    fn render(&mut self) {
        // Just render child and use its buffer directly
        // Frame doesn't create its own buffer
        self.child.render();

        if let Some(child_buffer) = self.child.get_buffer() {
            self.buffer = Some(child_buffer.clone());
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
        self.child.hit_test(point)
    }

    fn handle_event(&mut self, event: &Event, ctx: &mut EventContext) {
        self.child.handle_event(event, ctx);
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

/// Padding modifier - adds space around child view
///
/// Similar to SwiftUI's .padding() modifier
#[derive(Clone)]
pub struct Padding<V: View + Clone> {
    pub child: V,
    pub top: f32,
    pub bottom: f32,
    pub leading: f32,
    pub trailing: f32,
}

impl<V: View + Clone> Padding<V> {
    pub fn new(child: V) -> Self {
        Self {
            child,
            top: 0.0,
            bottom: 0.0,
            leading: 0.0,
            trailing: 0.0,
        }
    }

    pub fn all(mut self, padding: f32) -> Self {
        self.top = padding;
        self.bottom = padding;
        self.leading = padding;
        self.trailing = padding;
        self
    }

    pub fn top(mut self, top: f32) -> Self {
        self.top = top;
        self
    }

    pub fn bottom(mut self, bottom: f32) -> Self {
        self.bottom = bottom;
        self
    }

    pub fn leading(mut self, leading: f32) -> Self {
        self.leading = leading;
        self
    }

    pub fn trailing(mut self, trailing: f32) -> Self {
        self.trailing = trailing;
        self
    }

    pub fn vertical(mut self, padding: f32) -> Self {
        self.top = padding;
        self.bottom = padding;
        self
    }

    pub fn horizontal(mut self, padding: f32) -> Self {
        self.leading = padding;
        self.trailing = padding;
        self
    }
}

impl<V: View + Clone> View for Padding<V> {
    fn type_id(&self) -> std::any::TypeId {
        std::any::TypeId::of::<Self>()
    }

    fn type_name(&self) -> &'static str {
        "Padding"
    }

    fn build(&self) -> std::boxed::Box<dyn RenderObject> {
        std::boxed::Box::new(PaddingRenderObject::new(self.clone()))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct PaddingRenderObject {
    id: NodeId,
    parent: Option<NodeId>,
    child: std::boxed::Box<dyn RenderObject>,
    top: f32,
    bottom: f32,
    leading: f32,
    trailing: f32,
    buffer: Option<Buffer>,
    frame: Rect,
    dirty_flags: DirtyFlags,
}

impl PaddingRenderObject {
    pub fn new(view: Padding<impl View + Clone>) -> Self {
        let child = view.child.build();
        let id = NodeId::new();

        let mut child_boxed = child;
        child_boxed.set_parent(id);

        Self {
            id,
            parent: None,
            child: child_boxed,
            top: view.top,
            bottom: view.bottom,
            leading: view.leading,
            trailing: view.trailing,
            buffer: None,
            frame: Rect::ZERO,
            dirty_flags: DirtyFlags::all(),
        }
    }
}

impl RenderObject for PaddingRenderObject {
    fn id(&self) -> NodeId {
        self.id
    }

    fn parent(&self) -> Option<NodeId> {
        self.parent
    }

    fn set_parent(&mut self, parent: NodeId) {
        self.parent = Some(parent);
    }

    fn children(&self) -> &[std::boxed::Box<dyn RenderObject>] {
        std::slice::from_ref(&self.child)
    }

    fn children_mut(&mut self) -> &mut [std::boxed::Box<dyn RenderObject>] {
        std::slice::from_mut(&mut self.child)
    }

    fn get_child(&self, id: NodeId) -> Option<&dyn RenderObject> {
        if self.child.id() == id {
            Some(self.child.as_ref())
        } else {
            self.child.get_child(id)
        }
    }

    fn get_child_mut(&mut self, id: NodeId) -> Option<&mut (dyn RenderObject + '_)> {
        if self.child.id() == id {
            Some(self.child.as_mut())
        } else {
            self.child.get_child_mut(id)
        }
    }

    fn type_id(&self) -> std::any::TypeId {
        std::any::TypeId::of::<PaddingRenderObject>()
    }

    fn type_name(&self) -> &'static str {
        "Padding"
    }

    fn try_update(&mut self, _new_view: &dyn View) -> Option<UpdateResult> {
        Some(UpdateResult::Changed(DirtyFlags::CHILDREN))
    }

    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        let horizontal_padding = self.leading + self.trailing;
        let vertical_padding = self.top + self.bottom;

        // Reduce constraints by padding
        let child_constraints = LayoutConstraints {
            min: Size::new(
                (constraints.min.width - horizontal_padding).max(0.0),
                (constraints.min.height - vertical_padding).max(0.0),
            ),
            max: Size::new(
                (constraints.max.width - horizontal_padding).max(0.0),
                (constraints.max.height - vertical_padding).max(0.0),
            ),
        };

        let child_size = self.child.layout(child_constraints);

        // Child position at (leading, top)
        self.child.set_frame(Rect::new(
            Point::new(self.leading, self.top),
            child_size,
        ));

        // Total size includes padding
        let size = Size::new(
            child_size.width + horizontal_padding,
            child_size.height + vertical_padding,
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
        // Create or reuse buffer
        if self.buffer.as_ref().map_or(true, |b| b.size() != self.frame.size) {
            self.buffer = Some(Buffer::new(self.frame.size));
        }

        // Render child
        self.child.render();

        if let Some(child_buffer) = self.child.get_buffer() {
            // Blit child at padded position
            self.buffer.as_mut().unwrap().blit_from(child_buffer, self.child.frame());
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
        let child_frame = self.child.frame();
        if child_frame.contains(point) {
            let local_point = point - child_frame.origin;
            self.child.hit_test(local_point)
        } else {
            HitResult::Passthrough
        }
    }

    fn handle_event(&mut self, event: &Event, ctx: &mut EventContext) {
        self.child.handle_event(event, ctx);
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
