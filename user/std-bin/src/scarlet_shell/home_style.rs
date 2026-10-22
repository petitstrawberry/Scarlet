//! Shared launcher and console typography and application icon treatment.

use scarlet_ui::prelude::*;
use scarlet_ui::{Color, Icon};

pub fn application_icon(
    artwork: &super::app_artwork::AppArtwork,
    fallback: Icon,
    size: u16,
) -> impl View + Clone + use<> {
    if let Some(bitmap) = artwork.icon() {
        Either::A(
            Image::from_bitmap(bitmap)
                .fit_mode(ImageFit::Contain)
                .frame(size as f32, size as f32),
        )
    } else {
        Either::B(
            IconView::new(fallback)
                .size(IconSize::Pixels(size))
                .color(Color::WHITE),
        )
    }
}

pub fn launcher_icon(
    artwork: &super::app_artwork::AppArtwork,
    name: &str,
    size: u16,
    padding: f32,
    radius: f32,
) -> impl View + Clone + use<> {
    if let Some(bitmap) = artwork.icon() {
        let edge = size as f32 + padding * 2.0;
        Either::A(
            Image::from_bitmap(bitmap)
                .fit_mode(ImageFit::Contain)
                .frame(edge, edge),
        )
    } else {
        Either::B(
            IconView::new(icon(name))
                .size(IconSize::Pixels(size))
                .weight(IconWeight::Bold)
                .color(Color::WHITE)
                .padding(padding)
                .background(icon_tile_color(name))
                .clip_radius(radius),
        )
    }
}

#[cfg(target_os = "scarlet")]
pub fn initialize_fonts() {
    if std::env::var_os("SCARLET_UI_FONT_PATH").is_some() {
        return;
    }
    // The pinned std font discovery treats all bundled families as SystemUi,
    // including terminal fonts. Keep the distribution's UI face first in both
    // shell modes, matching ScarletUI's established non-std font stack.
    let Ok(primary) = std::fs::read("/share/fonts/Mplus1-Regular.ttf") else {
        return;
    };
    if scarlet_ui::graphics::set_default_font(Box::leak(primary.into_boxed_slice())).is_err() {
        return;
    }
    if let Ok(fallback) = std::fs::read("/share/fonts/JetBrainsMonoNerdFontMono-Regular.ttf") {
        let _ =
            scarlet_ui::graphics::add_default_font_fallback(Box::leak(fallback.into_boxed_slice()));
    }
}

pub fn icon(name: &str) -> Icon {
    icon_style(name).0
}

pub fn icon_tile_color(name: &str) -> Color {
    icon_style(name).1
}

pub fn catalog_icon<'a>(app_id: &str, name: &'a str) -> &'a str {
    // Older external bundles still advertise the generic launcher icon.
    // Respect a real icon supplied by a newer bundle or by the user.
    match (app_id, name) {
        ("org.scarlet-os.moonlight", "apps" | "application-x-executable" | "") => "moonlight",
        _ => name,
    }
}

// Resolve the catalog's theme names to ScarletUI's native Tabler vectors.
// Keep each glyph and accent together so launcher tiles and console covers
// cannot disagree about an application's identity.
fn icon_style(name: &str) -> (Icon, Color) {
    match name {
        "accessories-calculator" => (Icon::Calculator, Color::rgb(182, 107, 32)),
        "accessories-character-map" => (Icon::Language, Color::rgb(126, 79, 164)),
        "application-x-executable" | "apps" => (Icon::Package, Color::rgb(186, 95, 43)),
        "applications-development" | "code" => (Icon::Code, Color::rgb(47, 94, 174)),
        "applications-games" => (Icon::Building, Color::rgb(73, 128, 71)),
        "applications-graphics" => (Icon::Palette, Color::rgb(166, 72, 61)),
        "camera-photo" => (Icon::Camera, Color::rgb(38, 122, 151)),
        "file-description" | "text-x-generic" => (Icon::FileDescription, Color::rgb(28, 119, 126)),
        "file-music" => (Icon::FileMusic, Color::rgb(152, 62, 161)),
        "folder" => (Icon::Folder, Color::rgb(46, 112, 190)),
        "games" => (Icon::Map, Color::rgb(64, 119, 83)),
        "image" => (Icon::Photo, Color::rgb(180, 62, 121)),
        "layout-grid" => (Icon::LayoutGrid, Color::rgb(68, 103, 167)),
        "moonlight" => (Icon::Moon, Color::rgb(65, 91, 158)),
        "preferences-system" => (Icon::Settings, Color::rgb(84, 96, 119)),
        "preferences-system-time" => (Icon::Clock, Color::rgb(195, 76, 54)),
        "text-editor" => (Icon::FileText, Color::rgb(31, 132, 103)),
        "utilities-system-monitor" => (Icon::ChartBar, Color::rgb(25, 128, 139)),
        "utilities-terminal" => (Icon::Terminal, Color::rgb(68, 78, 96)),
        "video" | "multimedia-player" => (Icon::Video, Color::rgb(103, 70, 177)),
        "web-browser" => (Icon::Network, Color::rgb(38, 113, 158)),
        "x-office-calendar" => (Icon::Calendar, Color::rgb(174, 60, 81)),
        _ => (Icon::Apps, Color::rgb(184, 55, 79)),
    }
}
