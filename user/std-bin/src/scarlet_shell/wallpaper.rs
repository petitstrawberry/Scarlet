//! System desktop wallpaper shared by the SWS shell and the anonymous login painter.
use scarlet_desktop_config::BackgroundStyle;
use scarlet_ui::{BitmapImage, Color};
#[derive(Clone, PartialEq, Eq)]
pub struct BackgroundState {
    color: [u8; 3],
    style: BackgroundStyle,
    image: Option<String>,
}

pub fn load_background_state() -> BackgroundState {
    let config = scarlet_desktop_config::load_desktop_config();
    BackgroundState {
        color: config.theme.background.unwrap_or([40, 40, 50]),
        style: config
            .theme
            .background_style
            .unwrap_or(BackgroundStyle::GradientLines),
        image: config.theme.background_image,
    }
}

impl BackgroundState {
    /// Pre-login artwork belongs to the system. Personal backgrounds under a
    /// home directory remain within the authenticated desktop session.
    pub fn login_bitmap(mut self, width: u32, height: u32) -> BitmapImage {
        self.image = self.image.filter(|path| {
            path.starts_with("/share/") && !path.split('/').any(|component| component == "..")
        });
        let mut pixels = Vec::with_capacity(width as usize * height as usize);
        self.draw(width, height, |_, row| pixels.extend_from_slice(row));
        BitmapImage::from_bgra(pixels, width, height)
    }

    /// The compositor writes each row straight into its existing surface buffer.
    pub fn draw(&self, width: u32, height: u32, mut write_row: impl FnMut(usize, &[u32])) {
        if width == 0 || height == 0 {
            return;
        }
        let source = self.image.as_deref().and_then(BitmapImage::from_path);
        if let Some(image) = source.filter(|i| i.width() > 0 && i.height() > 0) {
            let (sw, sh) = (u64::from(image.width()), u64::from(image.height()));
            let (w, h) = (u64::from(width), u64::from(height));
            let (dw, dh) = if w * sh >= h * sw {
                (w, w * sh / sw)
            } else {
                (h * sw / sh, h)
            };
            let mut row = vec![0; width as usize];
            for y in 0..h {
                let sy = (y + (dh - h) / 2) * sh / dh;
                for (x, pixel) in row.iter_mut().enumerate() {
                    let sx = (x as u64 + (dw - w) / 2) * sw / dw;
                    *pixel = image.pixels()[(sy * sw + sx) as usize];
                }
                write_row(y as usize, &row);
            }
            return;
        }
        let base = Color::rgb(self.color[0], self.color[1], self.color[2]);
        let top = if self.color == [40, 40, 50] {
            Color::rgb(0.157, 0.157, 0.196)
        } else {
            base
        };
        let bottom = if self.color == [40, 40, 50] {
            Color::rgb(0.078, 0.078, 0.118)
        } else {
            Color::rgb(
                self.color[0] as f32 / 255.0 * 0.7,
                self.color[1] as f32 / 255.0 * 0.7,
                self.color[2] as f32 / 255.0 * 0.7,
            )
        };

        let mut row = vec![top.to_bgra(); width as usize];
        let accent = Color::rgba(0.392, 0.471, 0.549, 0.110).to_bgra();
        for y in 0..height as usize {
            if self.style != BackgroundStyle::Solid {
                let t = ((y as u32 * 255) / height.saturating_sub(1).max(1)) as f32;
                let mix = |a: f32, b: f32| (a * (255.0 - t) + b * t) / 255.0;
                row.fill(
                    Color::rgb(
                        mix(top.r, bottom.r),
                        mix(top.g, bottom.g),
                        mix(top.b, bottom.b),
                    )
                    .to_bgra(),
                );
                if self.style == BackgroundStyle::GradientLines {
                    for x in ((64 - y % 64) % 64..row.len()).step_by(64) {
                        row[x] = accent;
                    }
                }
            }
            write_row(y, &row);
        }
    }
}
