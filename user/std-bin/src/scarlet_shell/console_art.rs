//! Cached, enlarged launcher icons clipped into sharp application cover images.
//! ScarletUI renders the vector and displays the result with its standard Image.

use crate::app_artwork::{ConsoleImages, blurred_reflected_strip};
use scarlet_ui::renderer::{CpuPaintRenderer, PaintContext};
use scarlet_ui::{BitmapImage, Color, Icon, IconStyle, Rect, Size};
use std::sync::{Mutex, OnceLock};

const WIDTH: usize = 320;
const HEIGHT: usize = 180;
type CoverCache = Vec<(Icon, u32, ConsoleImages, u32)>;
static COVERS: OnceLock<Mutex<CoverCache>> = OnceLock::new();

pub fn images(icon: Icon, color: Color, name_ratio: f32) -> ConsoleImages {
    let rows = (HEIGHT as f32 * name_ratio)
        .round()
        .clamp(1.0, HEIGHT as f32) as u32;
    let mut cache = COVERS
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .unwrap();
    let key = color.to_bgra();
    if let Some((_, _, images, _)) = cache.iter().find(|(cached, tint, _, cached_rows)| {
        *cached == icon && *tint == key && *cached_rows == rows
    }) {
        return images.clone();
    }
    let image = cache
        .iter()
        .find(|(cached, tint, _, _)| *cached == icon && *tint == key)
        .map(|(_, _, images, _)| images.picture.clone())
        .unwrap_or_else(|| cover(icon, color));
    if cache.len() >= 32 {
        cache.remove(0);
    }
    let images = ConsoleImages {
        name_panel: blurred_reflected_strip(&image, rows),
        picture: image,
    };
    cache.push((icon, key, images.clone(), rows));
    images
}

fn cover(icon: Icon, color: Color) -> BitmapImage {
    let mut paint = PaintContext::new();
    paint.draw_icon(
        Rect::from_xywh(95.0, -36.0, 236.0, 236.0),
        icon,
        IconStyle::default().stroke_width(1.8),
        Color::WHITE,
    );
    let mut renderer = CpuPaintRenderer::new(
        Size::new(WIDTH as f32, HEIGHT as f32),
        1000,
        Color::TRANSPARENT,
    );
    renderer.execute(&paint);
    let mask: Vec<f32> = renderer
        .buffer()
        .data()
        .chunks_exact(4)
        .map(|p| p[3] as f32 / 255.0)
        .collect();
    let base = [
        color.b * 0.38 + 0.035,
        color.g * 0.38 + 0.035,
        color.r * 0.38 + 0.035,
    ];
    let pixels = mask
        .into_iter()
        .map(|alpha| {
            let channels =
                base.map(|channel| ((channel + (1.0 - channel) * alpha * 0.42) * 255.0) as u32);
            0xff00_0000 | channels[0] | channels[1] << 8 | channels[2] << 16
        })
        .collect();
    BitmapImage::from_bgra(pixels, WIDTH as u32, HEIGHT as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_picture_and_reflection_are_shared_across_rebuilds() {
        let color = Color::rgb(90, 100, 110);
        let first = images(Icon::Apps, color, 0.25);
        let again = images(Icon::Apps, color, 0.25);
        assert_eq!(
            first.picture.pixels().as_ptr(),
            again.picture.pixels().as_ptr()
        );
        assert_eq!(
            first.name_panel.pixels().as_ptr(),
            again.name_panel.pixels().as_ptr()
        );
        let resized = images(Icon::Apps, color, 0.5);
        assert_eq!(
            first.picture.pixels().as_ptr(),
            resized.picture.pixels().as_ptr()
        );
        assert_eq!(resized.name_panel.height(), 90);
        assert_eq!(
            resized.name_panel.pixels(),
            blurred_reflected_strip(&first.picture, 90).pixels()
        );
        assert_ne!(
            resized.name_panel.pixels(),
            crate::app_artwork::reflected_strip(&first.picture, 90).pixels()
        );
    }
}
