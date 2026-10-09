//! Cached, enlarged launcher icons blurred into application cover images.
//! ScarletUI renders the vector and displays the result with its standard Image.

use crate::app_artwork::{ConsoleImages, reflected_strip};
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
        name_panel: reflected_strip(&image, rows),
        picture: image,
    };
    cache.push((icon, key, images.clone(), rows));
    images
}

fn cover(icon: Icon, color: Color) -> BitmapImage {
    const RASTER_SCALE: usize = 4;
    let scale = RASTER_SCALE as f32;
    let mut paint = PaintContext::new();
    paint.draw_icon(
        Rect::from_xywh(95.0 / scale, -36.0 / scale, 236.0 / scale, 236.0 / scale),
        icon,
        IconStyle::default().stroke_width(1.8),
        Color::WHITE,
    );
    let mut renderer = CpuPaintRenderer::new(
        Size::new(
            (WIDTH / RASTER_SCALE) as f32,
            (HEIGHT / RASTER_SCALE) as f32,
        ),
        1000,
        Color::TRANSPARENT,
    );
    renderer.execute(&paint);
    let small_mask: Vec<f32> = renderer
        .buffer()
        .data()
        .chunks_exact(4)
        .map(|p| p[3] as f32 / 255.0)
        .collect();
    // Only fallback covers use this deliberately soft enlarged icon. Rasterize
    // its mask cheaply, then restore the original 320x180 raster and blur radius.
    let mut mask = enlarge_mask(&small_mask, WIDTH / RASTER_SCALE, HEIGHT / RASTER_SCALE);
    // Three separable box passes approximate a Gaussian blur. Cache by the
    // actual launcher icon and color, so focus and clock changes do no blur work.
    for _ in 0..3 {
        mask = blur(&blur(&mask, true), false);
    }
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

fn enlarge_mask(input: &[f32], width: usize, height: usize) -> Vec<f32> {
    let mut output = Vec::with_capacity(WIDTH * HEIGHT);
    for y in 0..HEIGHT {
        let sy = ((y as f32 + 0.5) * height as f32 / HEIGHT as f32 - 0.5)
            .clamp(0.0, (height - 1) as f32);
        let y0 = sy as usize;
        let y1 = (y0 + 1).min(height - 1);
        let ty = sy - y0 as f32;
        for x in 0..WIDTH {
            let sx = ((x as f32 + 0.5) * width as f32 / WIDTH as f32 - 0.5)
                .clamp(0.0, (width - 1) as f32);
            let x0 = sx as usize;
            let x1 = (x0 + 1).min(width - 1);
            let tx = sx - x0 as f32;
            let top = input[y0 * width + x0] * (1.0 - tx) + input[y0 * width + x1] * tx;
            let bottom = input[y1 * width + x0] * (1.0 - tx) + input[y1 * width + x1] * tx;
            output.push(top * (1.0 - ty) + bottom * ty);
        }
    }
    output
}

fn blur(input: &[f32], horizontal: bool) -> Vec<f32> {
    const RADIUS: isize = 5;
    let mut result = vec![0.0; input.len()];
    let (lines, length) = if horizontal {
        (HEIGHT, WIDTH)
    } else {
        (WIDTH, HEIGHT)
    };
    for line in 0..lines {
        let at = |position: isize| {
            let position = position.clamp(0, length as isize - 1) as usize;
            if horizontal {
                line * WIDTH + position
            } else {
                position * WIDTH + line
            }
        };
        let mut sum: f32 = (-RADIUS..=RADIUS).map(|p| input[at(p)]).sum();
        for position in 0..length as isize {
            result[at(position)] = sum / (RADIUS * 2 + 1) as f32;
            sum += input[at(position + RADIUS + 1)] - input[at(position - RADIUS)];
        }
    }
    result
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
            reflected_strip(&first.picture, 90).pixels()
        );
    }

    #[test]
    fn enlarged_mask_preserves_constant_alpha_and_clamps_edges() {
        assert!(
            enlarge_mask(&[0.5; 4], 2, 2)
                .iter()
                .all(|value| *value == 0.5)
        );
        let mask = enlarge_mask(&[0., 1., 1., 0.], 2, 2);
        assert_eq!(mask[0], 0.);
        assert_eq!(mask[WIDTH - 1], 1.);
        assert_eq!(mask[(HEIGHT - 1) * WIDTH], 1.);
        assert_eq!(mask[WIDTH * HEIGHT - 1], 0.);
        assert!(mask.iter().all(|value| (0. ..=1.).contains(value)));
    }
}
