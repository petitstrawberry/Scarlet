//! Shared console application card: fixed 16:9 image and reflected name panel.

use super::ApplicationTile;
use crate::home_style::application_icon;
use scarlet_ui::element::{ComponentElement, Element, ElementId, LayoutConstraints, UpdateResult};
use scarlet_ui::pipeline::MountContext;
use scarlet_ui::prelude::*;
use std::any::Any;

#[path = "console_art.rs"]
mod art;

pub const NAME_HEIGHT: f32 = 52.0;
pub const IMAGE_RATIO: f32 = 16.0 / 9.0;
const ACCENT: Color = Color::rgba_f32(0.88, 0.25, 0.25, 1.0);

pub fn build(app: &ApplicationTile, width: f32, selected: bool) -> impl View + Clone + use<> {
    // The focus outline and click handler update independently of the static
    // picture/label subtree. Building the subtree here would recursively clone
    // every modifier even when the artwork and geometry have not changed.
    CardBody {
        app: app.clone(),
        width,
    }
    .border_rounded(
        if selected {
            ACCENT
        } else {
            Color::WHITE.with_opacity(0.12)
        },
        if selected { 2.0 } else { 1.0 },
        16.0,
    )
    .repaint_boundary()
}

#[derive(Clone)]
struct CardBody {
    app: ApplicationTile,
    width: f32,
}

impl CardBody {
    fn same_content(&self, other: &Self) -> bool {
        self.width == other.width
            && self.app.app_id == other.app.app_id
            && self.app.name == other.app.name
            && self.app.icon == other.app.icon
            && self.app.color == other.app.color
            && self.app.artwork == other.app.artwork
    }
}

impl View for CardBody {
    fn create_element(&self) -> Box<dyn Element> {
        Box::new(CardBodyElement(ComponentElement::new_with_builder(
            self.clone(),
            |view| Box::new(body(&view.app, view.width)),
        )))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// CardBody has no callbacks or listenables. Only skip reconciliation when all
// of its immutable inputs match; keep the normal component lifecycle/layout.
struct CardBodyElement(ComponentElement<CardBody>);

impl Element for CardBodyElement {
    fn id(&self) -> ElementId {
        self.0.id()
    }
    fn type_name(&self) -> &str {
        "CardBodyElement"
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn children(&self) -> &[Box<dyn Element>] {
        self.0.children()
    }
    fn children_mut(&mut self) -> &mut [Box<dyn Element>] {
        self.0.children_mut()
    }

    fn update(&mut self, view: &dyn View) -> UpdateResult {
        let Some(next) = view.as_any().downcast_ref::<CardBody>() else {
            return UpdateResult::Replaced;
        };
        if self.0.view().same_content(next) {
            UpdateResult::NoChange
        } else {
            self.0.update(view)
        }
    }

    fn rebuild(&mut self) -> UpdateResult {
        self.0.rebuild()
    }
    fn mount(&mut self, ctx: &MountContext) {
        self.0.mount(ctx);
    }
    fn unmount(&mut self) {
        self.0.unmount();
    }
    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        self.0.layout(constraints)
    }
    fn last_layout_constraints(&self) -> Option<LayoutConstraints> {
        self.0.last_layout_constraints()
    }
    fn set_last_layout_constraints(&mut self, constraints: LayoutConstraints) {
        self.0.set_last_layout_constraints(constraints);
    }
    fn position(&self) -> Point {
        self.0.position()
    }
    fn set_position(&mut self, position: Point) {
        self.0.set_position(position);
    }
    fn set_viewport_hint(&mut self, viewport: Rect) -> bool {
        self.0.set_viewport_hint(viewport)
    }
    fn bounds(&self) -> Rect {
        self.0.bounds()
    }
    fn hit_test(&self, point: Point) -> bool {
        self.0.hit_test(point)
    }
    fn handle_event(&mut self, event: &scarlet_ui::Event, phase: scarlet_ui::event::Phase) -> bool {
        self.0.handle_event(event, phase)
    }
    fn take_window_action(&mut self) -> Option<scarlet_ui::event::WindowEvent> {
        self.0.take_window_action()
    }
    fn fill_width(&self) -> bool {
        self.0.fill_width()
    }
    fn fill_height(&self) -> bool {
        self.0.fill_height()
    }
}

fn body(app: &ApplicationTile, width: f32) -> impl View + Clone + use<> {
    let image_height = width / IMAGE_RATIO;
    let images = app
        .artwork
        .console_images(width, image_height, NAME_HEIGHT)
        .unwrap_or_else(|| art::images(app.icon, app.color, NAME_HEIGHT / image_height));
    let label = HStack::new((
        application_icon(&app.artwork, app.icon, 28),
        Text::new(&app.name)
            .font_size(21.0)
            .color(Color::WHITE)
            .alignment(Alignment::Leading)
            .frame((width - 62.0).max(1.0), 28.0)
            .clip(),
    ))
    .spacing(10.0)
    .alignment(Alignment::Center)
    .frame(width - 24.0, 28.0)
    .padding(12.0);
    let name_panel = ZStack::new((
        Image::from_bitmap(images.name_panel)
            .fit_mode(ImageFit::Cover)
            .frame(width, NAME_HEIGHT),
        Spacer::new()
            .frame(width, NAME_HEIGHT)
            .background(Color::BLACK.with_opacity(0.42)),
        label,
        VStack::new((
            Spacer::new()
                .frame(width, 1.0)
                .background(Color::WHITE.with_opacity(0.22)),
            Spacer::new().frame(width, NAME_HEIGHT - 1.0),
        ))
        .spacing(0.0),
    ))
    .alignment(Alignment::TopLeading)
    .frame(width, NAME_HEIGHT);
    VStack::new((
        Image::from_bitmap(images.picture)
            .fit_mode(ImageFit::Cover)
            .frame(width, image_height),
        name_panel,
    ))
    .spacing(0.0)
    .alignment(Alignment::TopLeading)
    .frame(width, image_height + NAME_HEIGHT)
    .clip_radius(16.0)
    // Artwork and labels stay unchanged while the focus outline moves.
    // Keep their raster separate from the enclosing shelf's selection paint.
    .repaint_boundary()
}

#[cfg(test)]
mod tests {
    use super::*;
    use scarlet_ui::pipeline::RenderingPipeline;
    use scarlet_ui::renderer::{CpuPaintBackend, PresentedFrame};

    fn window(app: &ApplicationTile, width: f32, selected: bool) -> impl View + Clone + use<> {
        Window::new("Card", build(app, width, selected))
            .decorated(false)
            .size(Size::new(280.0, 240.0))
    }

    fn pipeline(view: &dyn View) -> RenderingPipeline {
        let mut pipeline = RenderingPipeline::new();
        pipeline.set_root(view.create_element());
        pipeline.layout_initial();
        pipeline.set_paint_backend(Box::new(CpuPaintBackend::new(
            Size::new(280.0, 240.0),
            1000,
            Color::BLACK,
        )));
        pipeline
    }

    fn pixels(pipeline: &mut RenderingPipeline) -> Option<Vec<u8>> {
        match pipeline.render_for_present().unwrap() {
            PresentedFrame::Cpu { buffer, .. } => Some(buffer.data().to_vec()),
            PresentedFrame::Idle => None,
            PresentedFrame::External => panic!("expected CPU card paint"),
        }
    }

    #[test]
    fn retained_card_matches_fresh_paint_after_focus_catalog_and_geometry_changes() {
        let path = std::env::temp_dir().join(format!("scarlet-card-{}.png", std::process::id()));
        let file = std::fs::File::create(&path).unwrap();
        let mut encoder = png::Encoder::new(file, 16, 9);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&[80; 16 * 9 * 3])
            .unwrap();
        let replacement = crate::app_artwork::ArtworkCache::default().load(
            "test-card",
            "",
            path.to_str().unwrap(),
            crate::app_artwork::BackgroundBlur::None,
        );
        std::fs::remove_file(path).unwrap();
        assert!(replacement.background_source().is_some());
        let mut app = ApplicationTile {
            app_id: "test-card".into(),
            name: "Before".into(),
            icon: Icon::Terminal,
            color: Color::BLUE,
            artwork: Default::default(),
        };
        let mut retained = pipeline(&window(&app, 240.0, false));
        let before = pixels(&mut retained).unwrap();
        let mut previous = before.clone();
        for step in 0..5 {
            let width = if step >= 3 { 200.0 } else { 240.0 };
            let selected = step != 1;
            match step {
                2 => app.name = "Changed catalog label".into(),
                3 => {
                    app.icon = Icon::Settings;
                    app.color = Color::RED;
                }
                4 => app.artwork = replacement.clone(),
                _ => {}
            }
            retained
                .element_tree_mut()
                .root_mut()
                .unwrap()
                .update(&window(&app, width, selected));
            let actual = pixels(&mut retained).unwrap_or_else(|| previous.clone());
            let expected_view = Window::new(
                "Reference",
                body(&app, width)
                    .border_rounded(
                        if selected {
                            ACCENT
                        } else {
                            Color::WHITE.with_opacity(0.12)
                        },
                        if selected { 2.0 } else { 1.0 },
                        16.0,
                    )
                    .repaint_boundary(),
            )
            .decorated(false)
            .size(Size::new(280.0, 240.0));
            let mut fresh = pipeline(&expected_view);
            let expected = pixels(&mut fresh).unwrap();
            assert!(actual == expected, "retained card differs at step {step}");
            if step == 0 {
                assert_ne!(actual, before, "focus outline must update");
            }
            fresh.teardown();
            previous = actual;
        }
        retained.teardown();
    }
}
