//! Shared console application card: fixed 16:9 image and reflected name panel.

use super::ApplicationTile;
use crate::app_artwork::{ConsoleImages, reflected_strip};
use crate::home_style::application_icon;
use scarlet_ui::prelude::*;

#[path = "console_art.rs"]
mod art;

pub const NAME_HEIGHT: f32 = 52.0;
pub const IMAGE_RATIO: f32 = 16.0 / 9.0;
const ACCENT: Color = Color::rgba_f32(0.88, 0.25, 0.25, 1.0);

pub fn build(app: &ApplicationTile, width: f32, selected: bool) -> impl View + Clone + use<> {
    let image_height = width / IMAGE_RATIO;
    let images = app
        .artwork
        .console_images(width, image_height, NAME_HEIGHT)
        .unwrap_or_else(|| {
            let picture = art::cover(app.icon, app.color);
            let rows = (picture.height() as f32 * NAME_HEIGHT / image_height).round() as u32;
            let name_panel = reflected_strip(&picture, rows);
            ConsoleImages {
                picture,
                name_panel,
            }
        });
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
