//! Production drawer nesting, short-screen geometry and wheel paint regression.
#[path = "../../../user/std-bin/src/scarlet_shell/drawer.rs"]
mod drawer;

use scarlet_ui::element::{ComponentElement, Element};
use scarlet_ui::event::{Event, MouseEvent, ScrollSource, WheelPhase};
use scarlet_ui::geometry::{Point, Size};
use scarlet_ui::pipeline::RenderingPipeline;
use scarlet_ui::prelude::*;
use scarlet_ui::{GridView, hstack, vstack};
use std::any::Any;
use sws_protocol::workspace::{ShellPresentation, workspace_region_height};

#[derive(Clone)]
struct Fixture {
    width: f32,
    height: f32,
    presentation: ShellPresentation,
    applications: State<Vec<usize>>,
    selected: State<Option<usize>>,
    hovered: State<Option<usize>>,
    query: State<String>,
}

impl View for Fixture {
    fn create_element(&self) -> Box<dyn Element> {
        Box::new(ComponentElement::new_with_builder(
            self.clone(),
            build_drawer,
        ))
    }
    fn listenables(&self) -> Vec<&dyn Listenable> {
        vec![&self.applications, &self.selected, &self.hovered]
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// Keep the production frames, padding, alignment wrappers and interactive cells.
// The host fixture substitutes colored rectangles for catalog artwork and shell
// services; hover still rebuilds the scene containing the selected grid.
fn build_drawer(fixture: &Fixture) -> Box<dyn View> {
    let width = fixture.width;
    let height = fixture.height;
    let bar = 40.0;
    let work_height = height - bar;
    let top = workspace_region_height(work_height as u32, true, fixture.presentation, 1000) as f32;
    let drawer_height = drawer::visible_height(work_height, top, 96.0);
    let body_height = drawer_height - 36.0;
    let content_height = body_height - 6.0 - 20.0;
    let grid_height = content_height - 44.0 - 20.0;
    let content_width = width - 40.0;
    let columns = (((content_width + 20.0) / 148.0) as usize).clamp(1, 6);
    let grid_width = columns as f32 * 128.0 + columns.saturating_sub(1) as f32 * 20.0;
    let cells = fixture.clone();
    let grid = GridView::new(
        fixture.applications.clone(),
        fixture.selected.clone(),
        columns,
        110.0,
        move |index, _, selected| {
            let hover = cells.hovered.clone();
            let exit = hover.clone();
            let hovered = hover.get() == Some(index);
            vstack! {
                Spacer::new().frame(1.0, 10.0),
                Rectangle::new().fill(Color::rgb(index as u8 * 5, 30, 50)).frame(70.0, 70.0),
                Text::new(format!("App {index}")).font_size(13.0),
            }
            .alignment(Alignment::Center)
            .frame(f32::INFINITY, 110.0)
            .background(if hovered || selected == Some(index) {
                Color::rgb(50, 50, 50)
            } else {
                Color::TRANSPARENT
            })
            .clip_radius(13.0)
            .on_hover(move || hover.set(Some(index)))
            .on_exit(move || {
                if exit.get() == Some(index) {
                    exit.set(None);
                }
            })
            .on_click(|| {})
        },
    )
    .spacing(20.0)
    .row_spacing(18.0)
    .minimum_cell_width(96.0)
    .frame(grid_width, grid_height);
    let centered = hstack! { Spacer::new(), grid, Spacer::new() }
        .frame(content_width, grid_height)
        .alignment(Alignment::Top);
    // Match the two production Either wrappers for empty/search/grid branches.
    let populated: Either<Spacer, _> = Either::B(centered);
    let results: Either<Spacer, _> = Either::B(populated);
    let body = vstack! {
        hstack! {
            Text::new("Applications").alignment(Alignment::Leading).frame(112.0, 44.0),
            Spacer::new(),
            TextField::new(fixture.query.clone()).placeholder("Search applications")
                .frame((width * 0.32).clamp(280.0, 460.0), 36.0),
            Spacer::new(),
            Text::new("24 apps").alignment(Alignment::Trailing).frame(112.0, 44.0),
        }.alignment(Alignment::Center).frame(content_width, 44.0),
        results,
    }
    .spacing(20.0)
    .alignment(Alignment::TopLeading)
    .frame(content_width, content_height)
    .padding_insets(EdgeInsets {
        top: 6.0,
        left: 20.0,
        bottom: 20.0,
        right: 20.0,
    })
    .frame(width, body_height);
    let sheet = vstack! {
        hstack! { Spacer::new(), Spacer::new().frame(44.0, 4.0), Spacer::new() }
            .alignment(Alignment::Center).frame(width, 36.0), body,
    }
    .alignment(Alignment::TopLeading)
    .frame(width, drawer_height);
    let root = vstack! { Spacer::new().frame(width, top), sheet }
        .alignment(Alignment::TopLeading)
        .frame(width, work_height)
        .clip_radius(0.0)
        .padding_insets(EdgeInsets {
            top: bar,
            left: 0.0,
            bottom: 0.0,
            right: 0.0,
        })
        .frame(width, height)
        .on_key(|_| false);
    Box::new(drawer::Handle::new(
        root,
        fixture.presentation,
        drawer::Geometry {
            width,
            bar_height: bar,
            top,
            lip_height: 36.0,
        },
        |_| {},
    ))
}

fn scroll_bounds(element: &dyn Element, origin: Point, out: &mut Vec<(Point, Size)>) {
    let position = element.position();
    let origin = Point::new(origin.x + position.x, origin.y + position.y);
    if element
        .render_object()
        .is_some_and(|render| render.accepts_touch_scroll())
    {
        out.push((origin, element.render_object().unwrap().size()));
    }
    for child in element.children() {
        scroll_bounds(child.as_ref(), origin, out);
    }
}

#[test]
fn production_small_tablet_drawer_wheel_changes_pixels_after_hover_rebuild() {
    for (width, height) in [(756.0, 475.0), (960.0, 600.0)] {
        for presentation in [ShellPresentation::Overview, ShellPresentation::Home] {
            let fixture = Fixture {
                width,
                height,
                presentation,
                applications: State::new(scarlet_ui::state::generate_state_id(), (0..24).collect()),
                selected: State::new(scarlet_ui::state::generate_state_id(), Some(0)),
                hovered: State::new(scarlet_ui::state::generate_state_id(), None),
                query: State::new(scarlet_ui::state::generate_state_id(), String::new()),
            };
            let mut pipeline = RenderingPipeline::new();
            pipeline.set_root(fixture.create_element());
            pipeline.layout_initial();
            pipeline.resize(Size::new(width, height));
            pipeline.render_with_damage();
            let mut bounds = Vec::new();
            scroll_bounds(
                pipeline.element_tree().root().unwrap(),
                Point::ZERO,
                &mut bounds,
            );
            let (origin, size) = bounds[0];
            assert!(
                origin.y + size.height <= height,
                "grid below screen: {bounds:?}"
            );
            assert!(
                size.height >= 110.0,
                "one application row must fit: {bounds:?}"
            );
            let x = (origin.x + 50.0) as i32;
            let y = (origin.y + 30.0) as i32;
            pipeline.handle_event(&Event::Mouse(MouseEvent::Moved { x, y }));
            assert_eq!(fixture.hovered.get(), Some(0));
            pipeline.render_with_damage(); // Real scene rebuild from the hover state.
            let capture = |buffer: &scarlet_ui::buffer::Buffer| {
                (origin.y as u32..(origin.y + size.height) as u32)
                    .flat_map(|y| {
                        (origin.x as u32..(origin.x + size.width) as u32)
                            .map(move |x| buffer.get_pixel(x, y))
                    })
                    .collect::<Vec<_>>()
            };
            pipeline.request_redraw();
            let before = capture(pipeline.render_with_damage().unwrap().0);
            assert!(
                pipeline.handle_event(&Event::Mouse(MouseEvent::Wheel {
                    delta_x: 0,
                    delta_y: -400,
                    x,
                    y,
                    phase: WheelPhase::Moved,
                    source: ScrollSource::Wheel,
                })),
                "wheel not handled at {width}x{height} {presentation:?}"
            );
            let after = capture(pipeline.render_with_damage().unwrap().0);
            assert_ne!(
                before, after,
                "wheel must paint different rows at {width}x{height} {presentation:?}"
            );
            assert_eq!(fixture.selected.get(), Some(0));
        }
    }
}

#[test]
fn production_tablet_drawer_reserve_matches_logical_and_physical_scale() {
    let logical = workspace_region_height(435, true, ShellPresentation::Overview, 1000);
    let physical = workspace_region_height(870, true, ShellPresentation::Overview, 2000);
    assert_eq!(435 - logical, 240);
    assert_eq!(870 - physical, 480);
    assert_eq!(physical, logical * 2);
}
