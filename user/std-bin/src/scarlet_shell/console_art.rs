//! Cached, enlarged launcher icons blurred into application cover images.
//! ScarletUI renders the vector and displays the result with its standard Image.

use scarlet_ui::renderer::{CpuPaintRenderer, PaintContext};
use scarlet_ui::{BitmapImage, Color, Icon, IconStyle, Rect, Size};
use std::sync::{Mutex, OnceLock};

const WIDTH: usize = 320;
const HEIGHT: usize = 180;
type CoverCache = Vec<(Icon, u32, BitmapImage)>;
static COVERS: OnceLock<Mutex<CoverCache>> = OnceLock::new();

pub fn cover(icon: Icon, color: Color) -> BitmapImage {
    let mut cache = COVERS
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .unwrap();
    let key = color.to_bgra();
    if let Some((_, _, image)) = cache
        .iter()
        .find(|(cached, tint, _)| *cached == icon && *tint == key)
    {
        return image.clone();
    }
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
    let mut mask: Vec<f32> = renderer
        .buffer()
        .data()
        .chunks_exact(4)
        .map(|p| p[3] as f32 / 255.0)
        .collect();
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
    let image = BitmapImage::from_bgra(pixels, WIDTH as u32, HEIGHT as u32);
    if cache.len() >= 32 {
        cache.remove(0);
    }
    cache.push((icon, key, image.clone()));
    image
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
