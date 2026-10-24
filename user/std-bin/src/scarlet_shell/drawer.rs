//! Drawer viewport and handle gestures. A swipe requests one short shell
//! transition; it never moves or resizes the application grid while held.
use scarlet_ui::View;
use scarlet_ui::element::{
    Element, ElementRenderObject, LayoutConstraints, RenderElement, UpdateResult,
};
use scarlet_ui::event::{
    Event, GesturePhase, MouseButton, MouseEvent, Phase, TouchDragAxis, TouchGesture,
};
use scarlet_ui::geometry::{Point, Size};
use std::{any::Any, rc::Rc};
use sws_protocol::workspace::ShellPresentation;

pub fn visible_height(work_height: f32, top: f32, open_top: f32) -> f32 {
    // Prepare the expanded sheet while hidden in Workspace.
    (work_height - if top >= work_height { open_top } else { top }).max(1.0)
}

#[derive(Clone, Copy, PartialEq)]
pub struct Geometry {
    pub width: f32,
    pub bar_height: f32,
    pub top: f32,
    pub lip_height: f32,
}

impl Geometry {
    fn in_lip(self, x: i32, y: i32) -> bool {
        let y = y as f32 - self.bar_height;
        x >= 0 && (x as f32) < self.width && y >= self.top && y < self.top + self.lip_height
    }
}

#[derive(Clone)]
pub struct Handle<V: View + Clone> {
    content: V,
    presentation: ShellPresentation,
    geometry: Geometry,
    settle: Rc<dyn Fn(bool)>,
}

impl<V: View + Clone> Handle<V> {
    pub fn new(
        content: V,
        presentation: ShellPresentation,
        geometry: Geometry,
        settle: impl Fn(bool) + 'static,
    ) -> Self {
        Self {
            content,
            presentation,
            geometry,
            settle: Rc::new(settle),
        }
    }
}

impl<V: View + Clone + 'static> View for Handle<V> {
    fn create_element(&self) -> Box<dyn Element> {
        Box::new(RenderElement::with_view_children_and_updater(
            self.clone(),
            |view| HandleRender {
                presentation: view.presentation,
                geometry: view.geometry,
                settle: view.settle.clone(),
                drag: None,
                size: Size::ZERO,
            },
            |render, view| {
                if render.presentation != view.presentation {
                    render.drag = None;
                }
                render.presentation = view.presentation;
                render.geometry = view.geometry;
                render.settle = view.settle.clone();
                UpdateResult::Updated
            },
            |view| vec![view.content.clone_view()],
        ))
    }
    fn listenables(&self) -> Vec<&dyn scarlet_ui::Listenable> {
        self.content.listenables()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Clone, Copy)]
struct Drag {
    start_y: i32,
    delta_y: i32,
    moved: bool,
    touch: Option<u64>,
}

struct HandleRender {
    presentation: ShellPresentation,
    geometry: Geometry,
    settle: Rc<dyn Fn(bool)>,
    drag: Option<Drag>,
    size: Size,
}

impl HandleRender {
    fn begin(&mut self, x: i32, y: i32, touch: Option<u64>) -> bool {
        if self.presentation == ShellPresentation::Workspace || !self.geometry.in_lip(x, y) {
            return false;
        }
        self.drag = Some(Drag {
            start_y: y,
            delta_y: 0,
            moved: false,
            touch,
        });
        true
    }
    fn moved(&mut self, y: i32) -> bool {
        let Some(drag) = self.drag.as_mut() else {
            return false;
        };
        // Use the same eight-pixel tap slop as the UI touch arena.
        drag.moved |= y.abs_diff(drag.start_y) > 8;
        drag.delta_y = y.saturating_sub(drag.start_y);
        true
    }
    fn finish(&mut self, cancel: bool) -> bool {
        let Some(drag) = self.drag.take() else {
            return false;
        };
        if cancel {
            return true;
        }
        let currently_open = self.presentation == ShellPresentation::Home;
        let open = if !drag.moved {
            !currently_open
        } else if drag.delta_y <= -32 {
            true
        } else if drag.delta_y >= 32 {
            false
        } else {
            currently_open
        };
        if open != currently_open {
            (self.settle)(open);
        }
        true
    }
}

impl ElementRenderObject for HandleRender {
    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        self.size = Size::new(constraints.max_width, constraints.max_height);
        self.size
    }
    fn layout_with_children(
        &mut self,
        constraints: LayoutConstraints,
        children: &mut [Box<dyn Element>],
    ) -> Size {
        if let Some(child) = children.first_mut() {
            self.size = child.layout(constraints);
            child.set_position(Point::ZERO);
        }
        self.size
    }
    fn size(&self) -> Size {
        self.size
    }
    fn update_needs_layout(&self) -> bool {
        true
    }
    fn touch_drag_axis(&self, point: Point) -> Option<TouchDragAxis> {
        self.geometry
            .in_lip(point.x as i32, point.y as i32)
            .then_some(TouchDragAxis::Vertical)
    }
    fn handle_event(&mut self, event: &Event, phase: Phase) -> bool {
        if !matches!(phase, Phase::Target | Phase::Bubble) {
            return false;
        }
        match event {
            Event::Mouse(MouseEvent::ButtonPressed {
                button: MouseButton::Left,
                x,
                y,
                ..
            }) => self.begin(*x, *y, None),
            Event::Mouse(MouseEvent::Moved { y, .. })
                if self.drag.is_some_and(|drag| drag.touch.is_none()) =>
            {
                self.moved(*y)
            }
            Event::Mouse(MouseEvent::ButtonReleased {
                button: MouseButton::Left,
                y,
                ..
            }) if self.drag.is_some_and(|drag| drag.touch.is_none()) => {
                self.moved(*y);
                self.finish(false)
            }
            Event::Mouse(MouseEvent::ButtonCancelled {
                button: MouseButton::Left,
                ..
            }) => self.finish(true),
            Event::TouchGesture(TouchGesture::Tap { x, y, .. }) => {
                if self.begin(*x, *y, None) {
                    self.finish(false)
                } else {
                    false
                }
            }
            Event::TouchGesture(TouchGesture::Drag {
                id,
                phase,
                x,
                y,
                delta_x,
                delta_y,
            }) => {
                if *phase == GesturePhase::Started {
                    self.begin(
                        x.saturating_sub(*delta_x),
                        y.saturating_sub(*delta_y),
                        Some(*id),
                    );
                }
                if !self.drag.is_some_and(|drag| drag.touch == Some(*id)) {
                    return false;
                }
                if *phase == GesturePhase::Cancelled {
                    return self.finish(true);
                }
                self.moved(*y);
                if *phase == GesturePhase::Ended {
                    self.finish(false);
                }
                true
            }
            _ => false,
        }
    }
    fn render(&mut self) {}
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scarlet_ui::event::{ScrollSource, WheelPhase};
    use scarlet_ui::pipeline::RenderingPipeline;
    use scarlet_ui::prelude::*;
    use scarlet_ui::{Color, GridView, State};
    use std::cell::RefCell;

    fn control(presentation: ShellPresentation) -> (HandleRender, Rc<RefCell<Vec<bool>>>) {
        let outcomes = Rc::new(RefCell::new(Vec::new()));
        let result = outcomes.clone();
        (
            HandleRender {
                presentation,
                geometry: Geometry {
                    width: 800.0,
                    bar_height: 24.0,
                    top: if presentation == ShellPresentation::Home {
                        100.0
                    } else {
                        460.0
                    },
                    lip_height: 36.0,
                },
                settle: Rc::new(move |open| result.borrow_mut().push(open)),
                drag: None,
                size: Size::new(800.0, 720.0),
            },
            outcomes,
        )
    }

    fn press(y: i32) -> Event {
        Event::Mouse(MouseEvent::ButtonPressed {
            button: MouseButton::Left,
            x: 400,
            y,
            click_count: 1,
        })
    }
    fn release(y: i32) -> Event {
        Event::Mouse(MouseEvent::ButtonReleased {
            button: MouseButton::Left,
            x: 400,
            y,
            click_count: 1,
        })
    }

    #[test]
    fn swipe_keeps_geometry_fixed_and_commits_once_on_release() {
        let (mut control, outcomes) = control(ShellPresentation::Overview);
        control.handle_event(&press(500), Phase::Target);
        control.handle_event(
            &Event::Mouse(MouseEvent::Moved { x: 400, y: 280 }),
            Phase::Target,
        );
        assert_eq!(control.geometry.top, 460.0);
        assert!(outcomes.borrow().is_empty());
        control.handle_event(&release(280), Phase::Target);
        assert_eq!(*outcomes.borrow(), vec![true]);
        assert!(!control.handle_event(&release(280), Phase::Target));
    }

    #[test]
    fn short_or_wrong_direction_drag_does_not_toggle_but_click_does() {
        let (mut control, outcomes) = control(ShellPresentation::Home);
        for end in [160, 80] {
            control.handle_event(&press(140), Phase::Target);
            control.handle_event(&release(end), Phase::Target);
        }
        assert!(outcomes.borrow().is_empty());
        control.handle_event(&press(140), Phase::Target);
        control.handle_event(&release(144), Phase::Target);
        assert_eq!(*outcomes.borrow(), vec![false]);
    }

    #[test]
    fn touch_cancel_never_changes_depth_and_downward_swipe_closes() {
        let (mut control, outcomes) = control(ShellPresentation::Home);
        assert_eq!(
            control.touch_drag_axis(Point::new(400.0, 140.0)),
            Some(TouchDragAxis::Vertical)
        );
        assert_eq!(control.touch_drag_axis(Point::new(400.0, 250.0)), None);
        for cancel in [true, false] {
            control.begin(400, 140, Some(7));
            control.moved(240);
            control.finish(cancel);
        }
        assert_eq!(*outcomes.borrow(), vec![false]);
    }

    #[test]
    fn tablet_overview_wheel_reaches_rows_previously_below_screen_clip() {
        let work_height = 696.0;
        let top = 459.0;
        let viewport = visible_height(work_height, top, 100.0) - 36.0 - 44.0 - 6.0 - 16.0 - 12.0;
        assert_eq!(viewport, 123.0);
        assert_eq!(visible_height(work_height, work_height, 100.0), 596.0);
        // Four rows fit in the old expanded viewport (482px) and therefore
        // could not scroll, while only the first row was actually on screen.
        let grid = GridView::new(
            State::new(
                scarlet_ui::state::generate_state_id(),
                (0..24usize).collect(),
            ),
            State::new(scarlet_ui::state::generate_state_id(), None),
            6,
            110.0,
            |index, _, _| scarlet_ui::views::Rectangle::new().fill(Color::rgb(index as u8, 30, 50)),
        )
        .row_spacing(18.0)
        .frame(800.0, viewport);
        let mut pipeline = RenderingPipeline::new();
        pipeline.set_root(grid.create_element());
        pipeline.layout_initial();
        pipeline.resize(Size::new(800.0, viewport));
        assert_eq!(
            pipeline.render_with_damage().unwrap().0.get_pixel(20, 20),
            Some(Color::rgb(0, 30, 50).to_bgra())
        );
        assert!(pipeline.handle_event(&Event::Mouse(MouseEvent::Wheel {
            delta_x: 0,
            delta_y: -10000,
            x: 20,
            y: 20,
            phase: WheelPhase::Moved,
            source: ScrollSource::Wheel,
        })));
        assert_eq!(
            pipeline.render_with_damage().unwrap().0.get_pixel(20, 100),
            Some(Color::rgb(18, 30, 50).to_bgra())
        );
    }

    #[test]
    fn pipeline_captures_mouse_and_native_touch_for_handle_swipes() {
        use scarlet_ui::event::{TouchChange, TouchFrame, TouchPhase};
        for touch in [false, true] {
            let (control, outcomes) = control(ShellPresentation::Overview);
            let view = Handle::new(
                Spacer::new().frame(800.0, 720.0),
                control.presentation,
                control.geometry,
                move |open| (control.settle)(open),
            );
            let mut pipeline = RenderingPipeline::new();
            pipeline.set_root(view.create_element());
            pipeline.layout_initial();
            let touch_event = |serial, phase, y| {
                Event::TouchFrame(TouchFrame {
                    seat_id: 0,
                    serial,
                    time_ns: serial * 16_000_000,
                    changes: vec![TouchChange {
                        seat_id: 0,
                        serial,
                        time_ns: serial * 16_000_000,
                        id: 7,
                        phase,
                        x: 400,
                        y,
                        pressure: None,
                        touch_major: None,
                    }],
                })
            };
            if touch {
                pipeline.handle_event(&touch_event(1, TouchPhase::Down, 500));
                pipeline.handle_event(&touch_event(2, TouchPhase::Move, 300));
                assert!(outcomes.borrow().is_empty());
                pipeline.handle_event(&touch_event(3, TouchPhase::Up, 300));
            } else {
                assert!(pipeline.handle_event(&press(500)));
                assert!(pipeline.handle_event(&Event::Mouse(MouseEvent::Moved { x: 400, y: 300 })));
                assert!(outcomes.borrow().is_empty());
                pipeline.handle_event(&release(300));
            }
            assert_eq!(*outcomes.borrow(), vec![true], "touch={touch}");
        }
    }
}
