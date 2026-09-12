//! Optional application image assets. File I/O, decoding and blur run on the
//! catalog worker; views only reuse images and cache crops for their geometry.

use scarlet_ui::BitmapImage;
use std::collections::BTreeMap;
use std::fmt;
use std::io::{BufReader, Cursor, Read};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BackgroundBlur {
    #[default]
    Auto,
    None,
    Full,
    Label,
}

impl BackgroundBlur {
    pub fn parse(value: &str) -> Self {
        match value {
            "none" => Self::None,
            "full" => Self::Full,
            "label" => Self::Label,
            _ => Self::Auto,
        }
    }
}

#[derive(Default)]
struct ArtworkData {
    icon: Option<BitmapImage>,
    background: Option<BitmapImage>,
    blurred: Option<BitmapImage>,
    blur: BackgroundBlur,
    dedicated: bool,
    crops: Mutex<Vec<((u32, u32, u32), BitmapImage)>>,
    console_images: Mutex<Vec<((u32, u32, u32), ConsoleImages)>>,
}

#[derive(Clone)]
pub struct ConsoleImages {
    pub picture: BitmapImage,
    pub name_panel: BitmapImage,
}

#[derive(Clone, Default)]
pub struct AppArtwork(Arc<ArtworkData>);

impl PartialEq for AppArtwork {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for AppArtwork {}
impl fmt::Debug for AppArtwork {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AppArtwork")
            .field("icon", &self.0.icon.is_some())
            .field("background", &self.0.background.is_some())
            .field("blur", &self.0.blur)
            .finish()
    }
}

impl AppArtwork {
    pub fn icon(&self) -> Option<BitmapImage> {
        self.0.icon.clone()
    }

    /// Untinted, cached image for a picture area with its label outside it.
    /// Without a dedicated asset, this is the generated icon background.
    pub fn background_source(&self) -> Option<BitmapImage> {
        self.0.background.clone()
    }

    /// A complete picture and a separate reflection of its lower edge. Source
    /// blur is prepared by the catalog worker; only fitted geometry is cached
    /// here, so ordinary focus changes do not decode or blur artwork again.
    pub fn console_images(
        &self,
        width: f32,
        height: f32,
        name_height: f32,
    ) -> Option<ConsoleImages> {
        let source = self.0.background.as_ref()?;
        let scale = (840.0 / width.max(height).max(1.0)).min(2.0);
        let width = (width * scale).round().max(1.0) as u32;
        let height = (height * scale).round().max(1.0) as u32;
        let rows = (name_height * scale).round().clamp(1.0, height as f32) as u32;
        let key = (width, height, rows);
        let mut cache = self.0.console_images.lock().unwrap();
        if let Some((_, images)) = cache.iter().find(|(geometry, _)| *geometry == key) {
            return Some(images.clone());
        }
        let blurred = self.0.blurred.as_ref().unwrap_or(source);
        let backdrop = fit_scene(blurred, blurred, width, height);
        let picture = if self.0.blur == BackgroundBlur::Full {
            backdrop.clone()
        } else {
            fit_scene(source, blurred, width, height)
        };
        let picture = if self.0.dedicated {
            picture
        } else {
            tint_image(&picture, 0.70)
        };
        let images = ConsoleImages {
            picture,
            name_panel: tint_image(&reflected_strip(&backdrop, rows), 0.70),
        };
        if cache.len() == 6 {
            cache.remove(0);
        }
        cache.push((key, images.clone()));
        Some(images)
    }

    pub fn background(&self, width: f32, height: f32, label_top: f32) -> Option<BitmapImage> {
        let original = self.0.background.as_ref()?;
        // Keep a useful 2x raster for ordinary tiles, bounded for large outputs.
        let scale = (840.0 / width.max(height)).min(2.0);
        let width = (width * scale).round().max(1.0) as u32;
        let height = (height * scale).round().max(1.0) as u32;
        let top = (label_top * scale).round().clamp(0.0, height as f32) as u32;
        let key = (width, height, top);
        let mut cache = self.0.crops.lock().unwrap();
        if let Some((_, image)) = cache.iter().find(|(geometry, _)| *geometry == key) {
            return Some(image.clone());
        }
        let sharp = resample(original, width, height, true, 0xff202126);
        let pixels = if let Some(blurred) = &self.0.blurred {
            let blurred = resample(blurred, width, height, true, 0xff202126);
            sharp
                .pixels()
                .iter()
                .zip(blurred.pixels())
                .enumerate()
                .map(|(index, (&a, &b))| {
                    let mix = if self.0.blur == BackgroundBlur::Full {
                        1.0
                    } else {
                        // Feather blur at the label boundary; do not add a color gradient.
                        ((index as u32 / width) as f32 - top as f32) / (12.0 * scale).max(1.0)
                    }
                    .clamp(0.0, 1.0);
                    mix_pixel(a, b, mix)
                })
                .collect::<Vec<_>>()
        } else {
            sharp.pixels().to_vec()
        };
        // A constant tint preserves legibility even over a bright photograph.
        let pixels = pixels
            .into_iter()
            .map(|pixel| mix_pixel(0xff16181d, pixel, 0.70))
            .collect();
        let image = BitmapImage::from_bgra(pixels, width, height);
        if cache.len() == 6 {
            cache.remove(0);
        }
        cache.push((key, image.clone()));
        Some(image)
    }
}

#[derive(Clone, PartialEq, Eq)]
struct FileStamp {
    path: String,
    metadata: Option<(u64, Option<SystemTime>)>,
}

impl FileStamp {
    fn new(path: &str) -> Self {
        // Scarlet uses slash-rooted paths, but its current std implementation
        // reports false from Path::is_absolute even for /share/... .
        let metadata = if path.starts_with('/') {
            std::fs::metadata(path)
                .ok()
                .filter(|m| m.is_file())
                .map(|m| (m.len(), m.modified().ok()))
        } else {
            None
        };
        Self {
            path: path.to_owned(),
            metadata,
        }
    }
}

#[derive(Default)]
pub struct ArtworkCache(BTreeMap<String, (FileStamp, FileStamp, BackgroundBlur, AppArtwork)>);

impl ArtworkCache {
    pub fn load(
        &mut self,
        id: &str,
        icon: &str,
        background: &str,
        blur: BackgroundBlur,
    ) -> AppArtwork {
        let requested_blur = blur;
        let icon_stamp = FileStamp::new(icon);
        let background_stamp = FileStamp::new(background);
        if let Some((a, b, mode, artwork)) = self.0.get(id)
            && a == &icon_stamp
            && b == &background_stamp
            && mode == &blur
        {
            return artwork.clone();
        }
        let icon = load_image(&icon_stamp).map(|image| thumbnail(&image, 256));
        let cover = load_image(&background_stamp).map(|image| {
            let image = thumbnail(&image, 640);
            resample(&image, image.width(), image.height(), false, 0xff202126)
        });
        let dedicated_background = cover.is_some();
        let cover = cover.or_else(|| icon.as_ref().map(icon_cover));
        let blur = match blur {
            BackgroundBlur::Auto if dedicated_background => BackgroundBlur::Label,
            BackgroundBlur::Auto => BackgroundBlur::Full,
            value => value,
        };
        let blurred = cover
            .as_ref()
            .filter(|_| blur != BackgroundBlur::None)
            .map(blur_image);
        let artwork = AppArtwork(Arc::new(ArtworkData {
            icon,
            background: cover,
            blurred,
            blur,
            dedicated: dedicated_background,
            crops: Mutex::new(Vec::new()),
            console_images: Mutex::new(Vec::new()),
        }));
        self.0.insert(
            id.to_owned(),
            (
                icon_stamp,
                background_stamp,
                requested_blur,
                artwork.clone(),
            ),
        );
        artwork
    }

    pub fn retain(&mut self, ids: &[String]) {
        self.0.retain(|id, _| ids.contains(id));
    }
}

fn load_image(stamp: &FileStamp) -> Option<BitmapImage> {
    const MAX_BYTES: u64 = 8 * 1024 * 1024;
    let (length, _) = stamp.metadata?;
    if length == 0 || length > MAX_BYTES {
        return None;
    }
    let mut bytes = Vec::new();
    std::fs::File::open(&stamp.path)
        .ok()?
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_BYTES {
        return None;
    }
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        let mut decoder = png::Decoder::new(BufReader::new(Cursor::new(bytes)));
        decoder.set_limits(png::Limits {
            bytes: 32 * 1024 * 1024,
        });
        decoder.set_transformations(png::Transformations::normalize_to_color8());
        let mut reader = decoder.read_info().ok()?;
        let (width, height) = (reader.info().width, reader.info().height);
        if !valid_dimensions(width, height) || reader.info().animation_control.is_some() {
            return None;
        }
        let size = reader.output_buffer_size()?;
        if size > 16 * 1024 * 1024 {
            return None;
        }
        let mut buffer = vec![0; size];
        let info = reader.next_frame(&mut buffer).ok()?;
        if (info.width, info.height) != (width, height) {
            return None;
        }
        let channels = info.color_type.samples();
        let mut pixels = Vec::with_capacity((width * height) as usize);
        for p in buffer[..info.buffer_size()].chunks_exact(channels) {
            let (r, g, b, a) = match info.color_type {
                png::ColorType::Rgba => (p[0], p[1], p[2], p[3]),
                png::ColorType::Rgb => (p[0], p[1], p[2], 255),
                png::ColorType::GrayscaleAlpha => (p[0], p[0], p[0], p[1]),
                png::ColorType::Grayscale => (p[0], p[0], p[0], 255),
                _ => return None,
            };
            pixels.push((a as u32) << 24 | (r as u32) << 16 | (g as u32) << 8 | b as u32);
        }
        Some(BitmapImage::from_bgra(pixels, width, height))
    } else {
        // Read dimensions before ScarletUI's JPEG decoder allocates its pixel buffer.
        let mut decoder =
            zune_jpeg::JpegDecoder::new(zune_jpeg::zune_core::bytestream::ZCursor::new(&bytes));
        decoder.decode_headers().ok()?;
        let (width, height) = decoder.dimensions()?;
        if !valid_dimensions(width as u32, height as u32) {
            return None;
        }
        BitmapImage::from_jpeg_bytes(&bytes)
    }
}

fn valid_dimensions(width: u32, height: u32) -> bool {
    width > 0
        && height > 0
        && width <= 4096
        && height <= 4096
        && (width as u64 * height as u64) <= 4 * 1024 * 1024
}

fn thumbnail(image: &BitmapImage, edge: u32) -> BitmapImage {
    let scale = (edge as f32 / image.width().max(image.height()) as f32).min(1.0);
    resample(
        image,
        (image.width() as f32 * scale).round().max(1.0) as u32,
        (image.height() as f32 * scale).round().max(1.0) as u32,
        false,
        0,
    )
}

fn icon_cover(icon: &BitmapImage) -> BitmapImage {
    // Leave room around a solid icon's silhouette. Cover-scaling a square icon
    // directly to 16:9 cuts its outline away and often produces a flat color.
    const WIDTH: u32 = 640;
    const HEIGHT: u32 = 360;
    const EDGE: f32 = 384.0;
    const BASE: u32 = 0xff252932;
    let scale = EDGE / icon.width().max(icon.height()) as f32;
    let width = (icon.width() as f32 * scale).round().max(1.0) as u32;
    let height = (icon.height() as f32 * scale).round().max(1.0) as u32;
    let icon = resample(icon, width, height, false, 0);
    let left = (WIDTH as i32 - width as i32) / 2;
    let top = (HEIGHT as i32 - height as i32) / 2;
    let mut pixels = vec![BASE; (WIDTH * HEIGHT) as usize];
    for y in 0..height {
        let target_y = top + y as i32;
        if !(0..HEIGHT as i32).contains(&target_y) {
            continue;
        }
        for x in 0..width {
            let target_x = left + x as i32;
            if !(0..WIDTH as i32).contains(&target_x) {
                continue;
            }
            let pixel = icon.pixels()[(y * width + x) as usize];
            pixels[(target_y as u32 * WIDTH + target_x as u32) as usize] =
                mix_pixel(BASE, pixel, (pixel >> 24) as f32 / 255.0);
        }
    }
    BitmapImage::from_bgra(pixels, WIDTH, HEIGHT)
}

fn mix_pixel(a: u32, b: u32, t: f32) -> u32 {
    let mut result = 0xff000000;
    for shift in [0, 8, 16] {
        let (a, b) = (((a >> shift) & 255) as f32, ((b >> shift) & 255) as f32);
        result |= ((a + (b - a) * t).round().clamp(0.0, 255.0) as u32) << shift;
    }
    result
}

/// Mirror only the lower strip, keeping its scale and horizontal alignment.
pub fn reflected_strip(image: &BitmapImage, rows: u32) -> BitmapImage {
    let rows = rows.clamp(1, image.height());
    let width = image.width() as usize;
    let mut pixels = Vec::with_capacity(width * rows as usize);
    for y in (image.height() - rows..image.height()).rev() {
        let start = y as usize * width;
        pixels.extend_from_slice(&image.pixels()[start..start + width]);
    }
    BitmapImage::from_bgra(pixels, image.width(), rows)
}

fn tint_image(image: &BitmapImage, amount: f32) -> BitmapImage {
    BitmapImage::from_bgra(
        image
            .pixels()
            .iter()
            .map(|&pixel| mix_pixel(0xff16181d, pixel, amount))
            .collect(),
        image.width(),
        image.height(),
    )
}

fn fit_scene(source: &BitmapImage, padding: &BitmapImage, width: u32, height: u32) -> BitmapImage {
    let scale = (width as f32 / source.width() as f32).min(height as f32 / source.height() as f32);
    let sw = (source.width() as f32 * scale)
        .round()
        .clamp(1.0, width as f32) as u32;
    let sh = (source.height() as f32 * scale)
        .round()
        .clamp(1.0, height as f32) as u32;
    let image = resample(source, sw, sh, false, 0xff202126);
    if sw == width && sh == height {
        return image;
    }
    let mut pixels = resample(padding, width, height, true, 0xff202126)
        .pixels()
        .to_vec();
    let (left, top) = ((width - sw) / 2, (height - sh) / 2);
    for y in 0..sh {
        let src = (y * sw) as usize;
        let dst = ((top + y) * width + left) as usize;
        pixels[dst..dst + sw as usize].copy_from_slice(&image.pixels()[src..src + sw as usize]);
    }
    BitmapImage::from_bgra(pixels, width, height)
}

fn resample(image: &BitmapImage, width: u32, height: u32, cover: bool, base: u32) -> BitmapImage {
    let (sw, sh) = (image.width() as f32, image.height() as f32);
    let scale = if cover {
        (width as f32 / sw).max(height as f32 / sh)
    } else {
        (width as f32 / sw).min(height as f32 / sh)
    };
    let (ox, oy) = (
        (sw - width as f32 / scale) * 0.5,
        (sh - height as f32 / scale) * 0.5,
    );
    let mut pixels = Vec::with_capacity((width * height) as usize);
    for y in 0..height {
        for x in 0..width {
            let sx = (ox + (x as f32 + 0.5) / scale - 0.5).clamp(0.0, sw - 1.0);
            let sy = (oy + (y as f32 + 0.5) / scale - 0.5).clamp(0.0, sh - 1.0);
            let (x0, y0) = (sx.floor() as u32, sy.floor() as u32);
            let (tx, ty) = (sx - x0 as f32, sy - y0 as f32);
            let mut rgba = [0.0; 4];
            for (dx, dy, weight) in [
                (0, 0, (1.0 - tx) * (1.0 - ty)),
                (1, 0, tx * (1.0 - ty)),
                (0, 1, (1.0 - tx) * ty),
                (1, 1, tx * ty),
            ] {
                let pixel = image.pixels()[(y0.saturating_add(dy).min(image.height() - 1)
                    * image.width()
                    + x0.saturating_add(dx).min(image.width() - 1))
                    as usize];
                let alpha = ((pixel >> 24) & 255) as f32 / 255.0;
                rgba[3] += alpha * weight;
                for (channel, shift) in [0, 8, 16].into_iter().enumerate() {
                    rgba[channel] += ((pixel >> shift) & 255) as f32 * alpha * weight;
                }
            }
            let opaque = base >> 24 == 255;
            let mut pixel = if opaque {
                0xff000000
            } else {
                ((rgba[3] * 255.0).round() as u32) << 24
            };
            for (channel, shift) in [0, 8, 16].into_iter().enumerate() {
                let value = if opaque {
                    rgba[channel] + ((base >> shift) & 255) as f32 * (1.0 - rgba[3])
                } else if rgba[3] > 0.0 {
                    rgba[channel] / rgba[3]
                } else {
                    0.0
                };
                pixel |= (value.round().clamp(0.0, 255.0) as u32) << shift;
            }
            pixels.push(pixel);
        }
    }
    BitmapImage::from_bgra(pixels, width, height)
}

fn blur_image(image: &BitmapImage) -> BitmapImage {
    let (width, height) = (image.width() as usize, image.height() as usize);
    let mut pixels = image.pixels().to_vec();
    let radius = (width.min(height) / 36).max(2) as isize;
    for _ in 0..3 {
        for horizontal in [true, false] {
            let mut output = vec![0; pixels.len()];
            let (lines, length) = if horizontal {
                (height, width)
            } else {
                (width, height)
            };
            for line in 0..lines {
                let at = |p: isize| {
                    let p = p.clamp(0, length as isize - 1) as usize;
                    if horizontal {
                        line * width + p
                    } else {
                        p * width + line
                    }
                };
                let mut sums = [0i32; 3];
                let channels = |p: u32| {
                    [
                        (p & 255) as i32,
                        ((p >> 8) & 255) as i32,
                        ((p >> 16) & 255) as i32,
                    ]
                };
                for p in -radius..=radius {
                    let c = channels(pixels[at(p)]);
                    for i in 0..3 {
                        sums[i] += c[i];
                    }
                }
                for p in 0..length as isize {
                    output[at(p)] = 0xff000000;
                    for i in 0..3 {
                        output[at(p)] |= ((sums[i] / (radius as i32 * 2 + 1)) as u32) << (i * 8);
                    }
                    let (enter, leave) = (
                        channels(pixels[at(p + radius + 1)]),
                        channels(pixels[at(p - radius)]),
                    );
                    for i in 0..3 {
                        sums[i] += enter[i] - leave[i];
                    }
                }
            }
            pixels = output;
        }
    }
    BitmapImage::from_bgra(pixels, image.width(), image.height())
}
