use crate::buffer::Buffer;
use crate::dirty::DirtyFlags;
use crate::event::{Event, EventContext, EventPhase, HitResult, MouseEventKind};
use crate::geometry::{Point, Rect, Size};
use crate::layout::LayoutConstraints;
use crate::node_id::NodeId;
use crate::state::State;
use crate::traits::{RenderObject, UpdateResult, View};
use crate::geometry::Color;
use crate::theme::with_theme;
use std::any::Any;

#[derive(Clone)]
pub struct Slider {
    pub value: State<f32>,
    pub min: f32,
    pub max: f32,
    pub width: f32,
}

impl Slider {
    pub fn new(value: State<f32>) -> Self {
        Self {
            value,
            min: 0.0,
            max: 100.0,
            width: 200.0,
        }
    }

    pub fn range(mut self, min: f32, max: f32) -> Self {
        self.min = min;
        self.max = max;
        self
    }

    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }
}

impl View for Slider {
    fn type_id(&self) -> std::any::TypeId {
        std::any::TypeId::of::<Self>()
    }

    fn type_name(&self) -> &'static str {
        "Slider"
    }

    fn build(&self) -> std::boxed::Box<dyn RenderObject> {
        std::boxed::Box::new(SliderRenderObject::new(self.clone()))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Default)]
struct SliderInteractionState {
    dragging: bool,
    hovered: bool,
    drag_start_x: f32,
    drag_start_value: f32,
}

pub struct SliderRenderObject {
    id: NodeId,
    parent: Option<NodeId>,
    view: Slider,
    buffer: Option<Buffer>,
    frame: Rect,
    interaction_state: SliderInteractionState,
    dirty_flags: DirtyFlags,
    last_known_value: f32,
}

impl SliderRenderObject {
    pub fn new(view: Slider) -> Self {
        let last_known_value = view.value.get();

        Self {
            id: NodeId::new(),
            parent: None,
            view,
            buffer: None,
            frame: Rect::ZERO,
            interaction_state: SliderInteractionState::default(),
            dirty_flags: DirtyFlags::PAINT | DirtyFlags::LAYOUT,
            last_known_value,
        }
    }

    fn value_for_position(&self, x: f32) -> f32 {
        let track_padding: f32 = 8.0;
        let track_width = self.frame.size.width - track_padding * 2.0;

        if track_width <= 0.0 {
            return self.view.min;
        }

        let relative_x = (x - track_padding).clamp(0.0, track_width);
        let ratio = relative_x / track_width;

        self.view.min + ratio * (self.view.max - self.view.min)
    }

    fn position_for_value(&self, value: f32) -> f32 {
        let track_padding: f32 = 8.0;
        let track_width = self.frame.size.width - track_padding * 2.0;

        if track_width <= 0.0 {
            return track_padding;
        }

        let range = self.view.max - self.view.min;
        if range <= 0.0 {
            return track_padding;
        }

        let ratio = (value - self.view.min) / range;
        track_padding + ratio * track_width
    }
}

impl RenderObject for SliderRenderObject {
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
        &[]
    }

    fn type_id(&self) -> std::any::TypeId {
        std::any::TypeId::of::<Slider>()
    }

    fn type_name(&self) -> &'static str {
        "Slider"
    }

    fn try_update(&mut self, new_view: &dyn View) -> Option<UpdateResult> {
        new_view
            .as_any()
            .downcast_ref::<Slider>()
            .map(|new_slider| {
                let current_value = self.view.value.get();
                let new_value = new_slider.value.get();

                if current_value != new_value {
                    self.view = new_slider.clone();
                    self.last_known_value = new_value;
                    Some(UpdateResult::Changed(DirtyFlags::PAINT))
                } else {
                    Some(UpdateResult::Unchanged)
                }
            })
            .flatten()
    }

    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        let height: f32 = 24.0;
        let width = self.view.width;

        let size = Size::new(
            width.clamp(constraints.min.width, constraints.max.width),
            height.clamp(constraints.min.height, constraints.max.height),
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
        if !self.is_dirty() {
            return;
        }

        let current_value = self.view.value.get();
        self.last_known_value = current_value;

        self.buffer = Some(Buffer::new(self.frame.size));

        // Draw track
        let track_padding: f32 = 8.0;
        let track_height: f32 = 4.0;
        let track_rect = Rect::new(
            Point::new(track_padding, (self.frame.size.height - track_height) / 2.0),
            Size::new(self.frame.size.width - track_padding * 2.0, track_height),
        );

        let track_color = with_theme(|theme| theme.background_secondary);
        self.buffer
            .as_mut()
            .unwrap()
            .fill_rect(track_rect, track_color.as_bgra());

        // Draw thumb
        let thumb_x = self.position_for_value(current_value);
        let thumb_size: f32 = 16.0;
        let thumb_rect = Rect::new(
            Point::new(
                thumb_x - thumb_size / 2.0,
                (self.frame.size.height - thumb_size) / 2.0,
            ),
            Size::new(thumb_size, thumb_size),
        );

        // Use theme-based colors for thumb with hover state
        let thumb_color = if self.interaction_state.dragging || self.interaction_state.hovered {
            with_theme(|theme| theme.button_background_hovered)
        } else {
            with_theme(|theme| theme.button_background)
        };

        self.buffer
            .as_mut()
            .unwrap()
            .fill_rect(thumb_rect, thumb_color.as_bgra());

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

    fn handle_event(&mut self, event: &Event, ctx: &mut EventContext) {
        match event {
            Event::Mouse(e) if ctx.phase == EventPhase::Target => {
                // Use local frame for bounds checking (point is in local coordinates)
                let local_frame = Rect::new(Point::ZERO, self.frame.size);
                match e.kind {
                    MouseEventKind::Move => {
                        let was_hovered = self.interaction_state.hovered;
                        self.interaction_state.hovered = local_frame.contains(e.position);

                        if self.interaction_state.dragging {
                            let new_value =
                                self.value_for_position(e.position.x);
                            self.view.value.set(new_value);
                        }

                        if was_hovered != self.interaction_state.hovered {
                            self.mark_dirty(DirtyFlags::PAINT);
                        }
                    }
                    MouseEventKind::Press => {
                        if self.interaction_state.hovered {
                            self.interaction_state.dragging = true;
                            self.interaction_state.drag_start_x = e.position.x;
                            self.interaction_state.drag_start_value =
                                self.view.value.get();
                            self.mark_dirty(DirtyFlags::PAINT);
                        }
                    }
                    MouseEventKind::Release => {
                        if self.interaction_state.dragging {
                            self.interaction_state.dragging = false;
                            self.mark_dirty(DirtyFlags::PAINT);
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn mark_dirty(&mut self, flags: DirtyFlags) {
        self.dirty_flags |= flags;
    }

    fn is_dirty(&self) -> bool {
        let current_value = self.view.value.get();
        !self.dirty_flags.is_empty() || current_value != self.last_known_value
    }

    fn clear_dirty(&mut self) {
        self.dirty_flags = DirtyFlags::empty();
    }
}
