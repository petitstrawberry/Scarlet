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
    match name {
        "apps" => Icon::Package,
        "applications-development" | "code" => Icon::Code,
        "file-description" => Icon::FileDescription,
        "file-music" => Icon::FileMusic,
        "folder" => Icon::Folder,
        "image" => Icon::Photo,
        "preferences-system" => Icon::Settings,
        "preferences-system-time" => Icon::Clock,
        "text-editor" => Icon::FileText,
        "utilities-system-monitor" => Icon::ChartBar,
        "utilities-terminal" => Icon::Terminal,
        "video" | "multimedia-player" => Icon::Video,
        _ => Icon::Apps,
    }
}

pub fn icon_tile_color(name: &str) -> Color {
    match name {
        "apps" => Color::rgb(186, 95, 43),
        "applications-development" | "code" => Color::rgb(47, 94, 174),
        "file-description" => Color::rgb(28, 119, 126),
        "file-music" => Color::rgb(152, 62, 161),
        "folder" => Color::rgb(46, 112, 190),
        "image" => Color::rgb(180, 62, 121),
        "preferences-system" => Color::rgb(84, 96, 119),
        "preferences-system-time" => Color::rgb(195, 76, 54),
        "text-editor" => Color::rgb(31, 132, 103),
        "utilities-system-monitor" => Color::rgb(25, 128, 139),
        "utilities-terminal" => Color::rgb(68, 78, 96),
        "video" | "multimedia-player" => Color::rgb(103, 70, 177),
        _ => Color::rgb(184, 55, 79),
    }
}
