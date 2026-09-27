//! Shared, passive audio feedback for Desktop and Console shells.
use scarlet_ui::prelude::*;
use scarlet_ui::{Icon, IconView, hstack, vstack};

pub const SCENE_KEY: &str = "volume-osd";
pub use super::volume_policy::{Feedback, Visibility, step_percent};

pub fn geometry(width: f32, height: f32) -> (Size, i32, i32) {
    let size = Size::new(
        (width - 32.0).clamp(1.0, 288.0),
        (height - 32.0).clamp(1.0, 88.0),
    );
    (
        size,
        ((width - size.width) / 2.0).max(0.0) as i32,
        (height - size.height - 48.0).max(0.0) as i32,
    )
}

pub fn view(feedback: Feedback, size: Size) -> impl View + Clone {
    let palette = scarlet_ui::color::ColorPalette::default();
    let label = match feedback.percent {
        None => String::from("Audio unavailable"),
        Some(percent) if feedback.muted => format!("Muted · {percent}%"),
        Some(percent) => format!("Volume {percent}%"),
    };
    let icon = if feedback.muted || feedback.percent.unwrap_or(0) == 0 {
        Icon::Volume3
    } else {
        Icon::Volume2
    };
    Surface::new(vstack! {
        hstack! { IconView::new(icon), Text::new(label).font_size(17.0) }
            .spacing(12.0).alignment(Alignment::Center),
        ProgressView::new(if feedback.muted { 0.0 } else { feedback.percent.unwrap_or(0) as f32 / 100.0 })
            .frame_width((size.width - 40.0).max(1.0)),
    }.spacing(12.0).padding(16.0), SurfaceRole::Floating)
        .fill(palette.surface().with_opacity(0.96))
        .bordered(true).corner_radius(16.0).frame(size.width, size.height)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn geometry_stays_on_output_in_portrait_and_small_sizes() {
        for (width, height) in [(1280.0, 720.0), (720.0, 1280.0), (240.0, 100.0)] {
            let (size, x, y) = geometry(width, height);
            assert!(x >= 0 && y >= 0);
            assert!(x as f32 + size.width <= width);
            assert!(y as f32 + size.height <= height);
        }
    }
}
