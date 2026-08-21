//! Mouse cursor image loading, scaling, and composition.

use png::{BitDepth, BlendOp, ColorType, Decoder, DisposeOp, Limits, OutputInfo, Transformations};
use std::fs::File;
use std::io::BufReader;
use std::vec;
use std::vec::Vec;

const FALLBACK_CURSOR_WIDTH: usize = 16;
const FALLBACK_CURSOR_HEIGHT: usize = 24;
const CURSOR_DAMAGE_PADDING: i32 = 2;
const MAX_CURSOR_SOURCE_DIMENSION: u32 = 256;
const MAX_CURSOR_DECODE_BYTES: usize = 4 * 1024 * 1024;
const MAX_CURSOR_TOTAL_FRAME_BYTES: usize = 16 * 1024 * 1024;
const MAX_CURSOR_ANIMATION_FRAMES: u32 = 64;
const MIN_CURSOR_FRAME_DURATION_NS: u64 = 1_000_000;
const MAX_CURSOR_FRAME_DURATION_NS: u64 = 10_000_000_000;

/// Cursor color used by the built-in fallback image (white, BGRA).
const FALLBACK_CURSOR_COLOR: [u8; 4] = [255, 255, 255, 255];
/// Border color used by the built-in fallback image (black, BGRA).
const FALLBACK_CURSOR_BORDER: [u8; 4] = [0, 0, 0, 255];

/// Built-in fallback arrow.
///
/// `0` is transparent, `1` is white, and `2` is the black border.
const FALLBACK_CURSOR_BITMAP: [[u8; FALLBACK_CURSOR_WIDTH]; FALLBACK_CURSOR_HEIGHT] = [
    [2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [2, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [2, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [2, 1, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [2, 1, 1, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [2, 1, 1, 1, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [2, 1, 1, 1, 1, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [2, 1, 1, 1, 1, 1, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0],
    [2, 1, 1, 1, 1, 1, 1, 1, 2, 0, 0, 0, 0, 0, 0, 0],
    [2, 1, 1, 1, 1, 1, 1, 1, 1, 2, 0, 0, 0, 0, 0, 0],
    [2, 1, 1, 1, 1, 1, 1, 1, 1, 1, 2, 0, 0, 0, 0, 0],
    [2, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 2, 0, 0, 0, 0],
    [2, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, 0, 0, 0, 0],
    [2, 1, 1, 1, 1, 1, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0],
    [2, 1, 1, 2, 2, 1, 1, 1, 2, 0, 0, 0, 0, 0, 0, 0],
    [2, 1, 2, 0, 0, 2, 1, 1, 2, 0, 0, 0, 0, 0, 0, 0],
    [2, 2, 0, 0, 0, 0, 2, 1, 1, 2, 0, 0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0, 0, 2, 1, 1, 2, 0, 0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0, 0, 0, 2, 1, 1, 2, 0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0, 0, 0, 2, 1, 1, 2, 0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0, 0, 0, 0, 2, 1, 1, 2, 0, 0, 0, 0],
    [0, 0, 0, 0, 0, 0, 0, 0, 2, 1, 2, 0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 2, 0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0],
];

struct CursorPixels {
    width: u32,
    height: u32,
    bgra: Vec<u8>,
}

struct DecodedCursorFrame {
    pixels: CursorPixels,
    duration_ns: u64,
}

struct DecodedCursorImage {
    frames: Vec<DecodedCursorFrame>,
}

struct CursorFrame {
    bgra: Vec<u8>,
    duration_ns: u64,
}

struct CursorImage {
    icon: sws_protocol::CursorIcon,
    width: u32,
    height: u32,
    hotspot_x: u32,
    hotspot_y: u32,
    frames: Vec<CursorFrame>,
}

fn scale_milli_or_default(scale_milli: u32) -> u32 {
    scale_milli.max(1)
}

fn scaled_len(value: u32, scale_milli: u32) -> u32 {
    let scaled = (u64::from(value) * u64::from(scale_milli_or_default(scale_milli)) + 999) / 1000;
    scaled.max(1).min(u64::from(u32::MAX)) as u32
}

fn fallback_cursor_pixels() -> CursorPixels {
    let mut bgra = Vec::with_capacity(FALLBACK_CURSOR_WIDTH * FALLBACK_CURSOR_HEIGHT * 4);
    for row in FALLBACK_CURSOR_BITMAP {
        for pixel in row {
            let color = match pixel {
                1 => FALLBACK_CURSOR_COLOR,
                2 => FALLBACK_CURSOR_BORDER,
                _ => [0, 0, 0, 0],
            };
            bgra.extend_from_slice(&color);
        }
    }
    CursorPixels {
        width: FALLBACK_CURSOR_WIDTH as u32,
        height: FALLBACK_CURSOR_HEIGHT as u32,
        bgra,
    }
}

fn decode_png_file(path: &str) -> Result<DecodedCursorImage, &'static str> {
    let file = File::open(path).map_err(|_| "Failed to open cursor PNG")?;
    let mut decoder = Decoder::new(BufReader::new(file));
    decoder.set_limits(Limits {
        bytes: MAX_CURSOR_DECODE_BYTES,
    });
    decoder.set_transformations(Transformations::normalize_to_color8());

    let mut reader = decoder
        .read_info()
        .map_err(|_| "Failed to read cursor PNG header")?;
    let source_width = reader.info().width;
    let source_height = reader.info().height;
    if source_width == 0 || source_height == 0 {
        return Err("Cursor PNG has empty dimensions");
    }
    if source_width > MAX_CURSOR_SOURCE_DIMENSION || source_height > MAX_CURSOR_SOURCE_DIMENSION {
        return Err("Cursor PNG dimensions exceed 256x256");
    }
    let frame_count = reader
        .info()
        .animation_control
        .as_ref()
        .map(|control| control.num_frames)
        .unwrap_or(1);
    if frame_count == 0 || frame_count > MAX_CURSOR_ANIMATION_FRAMES {
        return Err("Cursor PNG animation frame count is invalid");
    }
    let output_buffer_size = reader
        .output_buffer_size()
        .ok_or("Cursor PNG output buffer size overflow")?;
    if output_buffer_size > MAX_CURSOR_DECODE_BYTES {
        return Err("Cursor PNG output exceeds decode limit");
    }
    let mut decoded = vec![0; output_buffer_size];
    let mut frames = Vec::new();
    let mut total_frame_bytes = 0usize;
    for _ in 0..frame_count {
        decoded.fill(0);
        let info = reader
            .next_frame(&mut decoded)
            .map_err(|_| "Failed to decode cursor PNG frame")?;
        if info.bit_depth != BitDepth::Eight {
            return Err("Cursor PNG did not normalize to 8-bit color");
        }

        let duration_ns = if frame_count > 1 {
            let control = reader
                .info()
                .frame_control
                .as_ref()
                .ok_or("Animated cursor PNG frame is missing control data")?;
            if control.width != source_width
                || control.height != source_height
                || control.x_offset != 0
                || control.y_offset != 0
                || control.dispose_op != DisposeOp::None
                || control.blend_op != BlendOp::Source
            {
                return Err("Animated cursor PNG must use full source frames");
            }
            frame_duration_ns(control.delay_num, control.delay_den)
        } else {
            0
        };
        let pixels = decoded_frame_to_bgra(&decoded[..info.buffer_size()], info)?;
        total_frame_bytes = total_frame_bytes
            .checked_add(pixels.bgra.len())
            .ok_or("Cursor PNG animation size overflow")?;
        if total_frame_bytes > MAX_CURSOR_TOTAL_FRAME_BYTES {
            return Err("Cursor PNG animation exceeds decode limit");
        }
        frames.push(DecodedCursorFrame {
            pixels,
            duration_ns,
        });
    }

    Ok(DecodedCursorImage { frames })
}

fn frame_duration_ns(delay_num: u16, delay_den: u16) -> u64 {
    let numerator = u64::from(delay_num.max(1));
    let denominator = u64::from(if delay_den == 0 { 100 } else { delay_den });
    numerator
        .saturating_mul(1_000_000_000)
        .checked_div(denominator)
        .unwrap_or(MIN_CURSOR_FRAME_DURATION_NS)
        .clamp(MIN_CURSOR_FRAME_DURATION_NS, MAX_CURSOR_FRAME_DURATION_NS)
}

fn decoded_frame_to_bgra(decoded: &[u8], info: OutputInfo) -> Result<CursorPixels, &'static str> {
    let pixel_count = (info.width as usize)
        .checked_mul(info.height as usize)
        .ok_or("Cursor PNG dimensions overflow")?;
    let output_len = pixel_count
        .checked_mul(4)
        .ok_or("Cursor PNG buffer size overflow")?;
    let samples = info.color_type.samples();
    let mut bgra = Vec::with_capacity(output_len);

    for y in 0..info.height as usize {
        let row_start = y
            .checked_mul(info.line_size)
            .ok_or("Cursor PNG row offset overflow")?;
        let row_end = row_start
            .checked_add(info.line_size)
            .ok_or("Cursor PNG row size overflow")?;
        let row = decoded
            .get(row_start..row_end)
            .ok_or("Cursor PNG row is truncated")?;

        for x in 0..info.width as usize {
            let source_offset = x
                .checked_mul(samples)
                .ok_or("Cursor PNG pixel offset overflow")?;
            let source = row
                .get(source_offset..source_offset + samples)
                .ok_or("Cursor PNG pixel is truncated")?;
            let (red, green, blue, alpha) = match info.color_type {
                ColorType::Rgba => (source[0], source[1], source[2], source[3]),
                ColorType::Rgb => (source[0], source[1], source[2], 255),
                ColorType::GrayscaleAlpha => (source[0], source[0], source[0], source[1]),
                ColorType::Grayscale => (source[0], source[0], source[0], 255),
                ColorType::Indexed => return Err("Cursor PNG palette was not expanded"),
            };
            bgra.extend_from_slice(&[blue, green, red, alpha]);
        }
    }

    Ok(CursorPixels {
        width: info.width,
        height: info.height,
        bgra,
    })
}

fn scale_bgra_nearest(
    source: &CursorPixels,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, &'static str> {
    let pixel_count = (width as usize)
        .checked_mul(height as usize)
        .ok_or("Scaled cursor dimensions overflow")?;
    let mut scaled = vec![
        0;
        pixel_count
            .checked_mul(4)
            .ok_or("Scaled cursor buffer size overflow")?
    ];

    for target_y in 0..height {
        let source_y =
            ((u64::from(target_y) * u64::from(source.height)) / u64::from(height)) as u32;
        for target_x in 0..width {
            let source_x =
                ((u64::from(target_x) * u64::from(source.width)) / u64::from(width)) as u32;
            let source_offset =
                ((source_y as usize * source.width as usize) + source_x as usize) * 4;
            let target_offset = ((target_y as usize * width as usize) + target_x as usize) * 4;
            scaled[target_offset..target_offset + 4]
                .copy_from_slice(&source.bgra[source_offset..source_offset + 4]);
        }
    }

    Ok(scaled)
}

fn scaled_hotspot(value: u32, source_len: u32, target_len: u32) -> u32 {
    let source_value = value.min(source_len.saturating_sub(1));
    ((u64::from(source_value) * u64::from(target_len)) / u64::from(source_len))
        .min(u64::from(target_len.saturating_sub(1))) as u32
}

fn blend_straight_bgra(destination: &mut [u8], source: [u8; 4]) {
    let source_alpha = u32::from(source[3]);
    if source_alpha == 0 {
        return;
    }
    if source_alpha == 255 {
        destination[..4].copy_from_slice(&source);
        return;
    }

    let destination_alpha = u32::from(destination[3]);
    let inverse_source_alpha = 255 - source_alpha;
    let output_alpha_numerator = source_alpha * 255 + destination_alpha * inverse_source_alpha;
    if output_alpha_numerator == 0 {
        destination[..4].fill(0);
        return;
    }

    for channel in 0..3 {
        let source_premultiplied = u32::from(source[channel]) * source_alpha * 255;
        let destination_premultiplied =
            u32::from(destination[channel]) * destination_alpha * inverse_source_alpha;
        destination[channel] =
            ((source_premultiplied + destination_premultiplied + output_alpha_numerator / 2)
                / output_alpha_numerator) as u8;
    }
    destination[3] = ((output_alpha_numerator + 127) / 255) as u8;
}

/// Mouse cursor state and its scaled straight-alpha BGRA image.
pub struct Cursor {
    /// Pointer hotspot x-coordinate in screen space.
    pub x: i32,
    /// Pointer hotspot y-coordinate in screen space.
    pub y: i32,
    prev_draw_x: i32,
    prev_draw_y: i32,
    prev_width: u32,
    prev_height: u32,
    /// Scaled cursor image width.
    pub width: u32,
    /// Scaled cursor image height.
    pub height: u32,
    hotspot_x: u32,
    hotspot_y: u32,
    images: Vec<CursorImage>,
    active_index: usize,
    active_frame_index: usize,
    next_animation_deadline_ns: Option<u64>,
    texture_generation: u64,
    needs_redraw: bool,
}

impl Cursor {
    /// Load and scale a PNG cursor image.
    ///
    /// # Arguments
    ///
    /// * `path` - Absolute path to an RGBA, RGB, grayscale, or paletted PNG.
    /// * `scale_milli` - Output scale in thousandths, where `1000` is 1x.
    /// * `hotspot_x` - Unscaled x-coordinate of the pointer hotspot.
    /// * `hotspot_y` - Unscaled y-coordinate of the pointer hotspot.
    ///
    /// # Returns
    ///
    /// A cursor containing scaled straight-alpha BGRA pixels, or a decode error.
    pub fn from_png_file(
        path: &str,
        scale_milli: u32,
        hotspot_x: u32,
        hotspot_y: u32,
    ) -> Result<Self, &'static str> {
        let decoded = decode_png_file(path)?;
        Self::from_decoded(decoded, scale_milli, hotspot_x, hotspot_y)
    }

    /// Create the built-in cursor used when the configured image cannot load.
    ///
    /// # Arguments
    ///
    /// * `scale_milli` - Output scale in thousandths, where `1000` is 1x.
    ///
    /// # Returns
    ///
    /// The scaled built-in cursor with a top-left hotspot.
    pub fn fallback(scale_milli: u32) -> Self {
        Self::from_source(fallback_cursor_pixels(), scale_milli, 0, 0)
            .expect("built-in cursor image dimensions must be valid")
    }

    fn from_source(
        source: CursorPixels,
        scale_milli: u32,
        hotspot_x: u32,
        hotspot_y: u32,
    ) -> Result<Self, &'static str> {
        Self::from_decoded(
            DecodedCursorImage {
                frames: vec![DecodedCursorFrame {
                    pixels: source,
                    duration_ns: 0,
                }],
            },
            scale_milli,
            hotspot_x,
            hotspot_y,
        )
    }

    fn from_decoded(
        decoded: DecodedCursorImage,
        scale_milli: u32,
        hotspot_x: u32,
        hotspot_y: u32,
    ) -> Result<Self, &'static str> {
        let image = Self::scaled_image_from_decoded(
            sws_protocol::CursorIcon::Arrow,
            decoded,
            scale_milli,
            hotspot_x,
            hotspot_y,
        )?;
        let width = image.width;
        let height = image.height;
        let hotspot_x = image.hotspot_x;
        let hotspot_y = image.hotspot_y;

        Ok(Self {
            x: 0,
            y: 0,
            prev_draw_x: -(hotspot_x as i32),
            prev_draw_y: -(hotspot_y as i32),
            prev_width: width,
            prev_height: height,
            width,
            height,
            hotspot_x,
            hotspot_y,
            images: vec![image],
            active_index: 0,
            active_frame_index: 0,
            next_animation_deadline_ns: None,
            texture_generation: 1,
            needs_redraw: true,
        })
    }

    #[cfg(test)]
    fn scaled_image_from_source(
        icon: sws_protocol::CursorIcon,
        source: CursorPixels,
        scale_milli: u32,
        hotspot_x: u32,
        hotspot_y: u32,
    ) -> Result<CursorImage, &'static str> {
        Self::scaled_image_from_decoded(
            icon,
            DecodedCursorImage {
                frames: vec![DecodedCursorFrame {
                    pixels: source,
                    duration_ns: 0,
                }],
            },
            scale_milli,
            hotspot_x,
            hotspot_y,
        )
    }

    fn scaled_image_from_decoded(
        icon: sws_protocol::CursorIcon,
        decoded: DecodedCursorImage,
        scale_milli: u32,
        hotspot_x: u32,
        hotspot_y: u32,
    ) -> Result<CursorImage, &'static str> {
        let first = decoded.frames.first().ok_or("Cursor image has no frames")?;
        let source_width = first.pixels.width;
        let source_height = first.pixels.height;
        let expected_len = (source_width as usize)
            .checked_mul(source_height as usize)
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or("Cursor image dimensions overflow")?;
        if source_width == 0 || source_height == 0 {
            return Err("Cursor image buffer is invalid");
        }
        if hotspot_x >= source_width || hotspot_y >= source_height {
            return Err("Cursor hotspot is outside the source image");
        }

        let width = scaled_len(source_width, scale_milli);
        let height = scaled_len(source_height, scale_milli);
        let hotspot_x = scaled_hotspot(hotspot_x, source_width, width);
        let hotspot_y = scaled_hotspot(hotspot_y, source_height, height);
        let mut frames = Vec::new();
        for frame in decoded.frames {
            if frame.pixels.width != source_width
                || frame.pixels.height != source_height
                || frame.pixels.bgra.len() != expected_len
            {
                return Err("Cursor animation frames have inconsistent dimensions");
            }
            frames.push(CursorFrame {
                bgra: scale_bgra_nearest(&frame.pixels, width, height)?,
                duration_ns: frame.duration_ns,
            });
        }

        Ok(CursorImage {
            icon,
            width,
            height,
            hotspot_x,
            hotspot_y,
            frames,
        })
    }

    /// Load one additional image into the cursor theme.
    ///
    /// # Arguments
    ///
    /// * `icon` - Cursor state represented by the image.
    /// * `path` - Absolute path to an RGBA, RGB, grayscale, or paletted PNG.
    /// * `scale_milli` - Output scale in thousandths, where `1000` is 1x.
    /// * `hotspot_x` - Unscaled x-coordinate of the pointer hotspot.
    /// * `hotspot_y` - Unscaled y-coordinate of the pointer hotspot.
    ///
    /// # Returns
    ///
    /// `Ok(())` after inserting or replacing the image, or a decode error.
    pub fn load_png_icon(
        &mut self,
        icon: sws_protocol::CursorIcon,
        path: &str,
        scale_milli: u32,
        hotspot_x: u32,
        hotspot_y: u32,
    ) -> Result<(), &'static str> {
        let decoded = decode_png_file(path)?;
        let image =
            Self::scaled_image_from_decoded(icon, decoded, scale_milli, hotspot_x, hotspot_y)?;
        if let Some(index) = self.images.iter().position(|entry| entry.icon == icon) {
            self.images[index] = image;
        } else {
            self.images.push(image);
        }
        Ok(())
    }

    /// Select an image from the loaded cursor theme.
    ///
    /// Missing images fall back to the standard arrow.
    ///
    /// # Arguments
    ///
    /// * `icon` - Cursor state to display.
    ///
    /// # Returns
    ///
    /// `true` when the effective cursor image changed.
    pub fn set_icon(&mut self, icon: sws_protocol::CursorIcon) -> bool {
        let next_index = self
            .images
            .iter()
            .position(|entry| entry.icon == icon)
            .or_else(|| {
                self.images
                    .iter()
                    .position(|entry| entry.icon == sws_protocol::CursorIcon::Arrow)
            })
            .unwrap_or(0);
        if next_index == self.active_index {
            return false;
        }

        let image = &self.images[next_index];
        self.active_index = next_index;
        self.active_frame_index = 0;
        self.next_animation_deadline_ns = None;
        self.width = image.width;
        self.height = image.height;
        self.hotspot_x = image.hotspot_x;
        self.hotspot_y = image.hotspot_y;
        self.texture_generation = self.texture_generation.wrapping_add(1).max(1);
        self.needs_redraw = true;
        true
    }

    /// Return the active cursor state.
    ///
    /// # Returns
    ///
    /// The icon whose image is currently selected.
    #[cfg(test)]
    pub fn icon(&self) -> sws_protocol::CursorIcon {
        self.images[self.active_index].icon
    }

    /// Return the active image generation for GPU texture synchronization.
    ///
    /// # Returns
    ///
    /// A non-zero value that changes whenever the active image changes.
    pub fn texture_generation(&self) -> u64 {
        self.texture_generation
    }

    /// Return the number of frames in the active cursor image.
    ///
    /// # Returns
    ///
    /// The active image's non-zero frame count.
    pub fn frame_count(&self) -> usize {
        self.images[self.active_index].frames.len()
    }

    /// Return the displayed frame index in the active cursor image.
    ///
    /// # Returns
    ///
    /// A zero-based index smaller than [`Self::frame_count`].
    pub fn active_frame_index(&self) -> usize {
        self.active_frame_index
    }

    /// Return one frame from the active cursor image as straight-alpha BGRA pixels.
    ///
    /// # Arguments
    ///
    /// * `index` - Zero-based frame index in the active image.
    ///
    /// # Returns
    ///
    /// The tightly packed frame pixels, or `None` when `index` is out of range.
    pub fn frame_bgra_pixels(&self, index: usize) -> Option<&[u8]> {
        self.images[self.active_index]
            .frames
            .get(index)
            .map(|frame| frame.bgra.as_slice())
    }

    /// Advance the active APNG animation to the frame due at `now_ns`.
    ///
    /// # Arguments
    ///
    /// * `now_ns` - Current monotonic time in nanoseconds.
    ///
    /// # Returns
    ///
    /// `true` when the displayed frame changed and must be recomposed.
    pub fn advance_animation(&mut self, now_ns: u64) -> bool {
        let frames = &self.images[self.active_index].frames;
        if frames.len() <= 1 {
            self.next_animation_deadline_ns = None;
            return false;
        }

        let Some(mut deadline) = self.next_animation_deadline_ns else {
            self.next_animation_deadline_ns =
                Some(now_ns.saturating_add(frames[self.active_frame_index].duration_ns));
            return false;
        };
        if now_ns < deadline {
            return false;
        }

        let cycle_duration = frames.iter().fold(0u64, |duration, frame| {
            duration.saturating_add(frame.duration_ns)
        });
        if cycle_duration != 0 {
            let complete_cycles = now_ns.saturating_sub(deadline) / cycle_duration;
            deadline = deadline.saturating_add(complete_cycles.saturating_mul(cycle_duration));
        }

        let mut changed = false;
        let mut steps = 0usize;
        while now_ns >= deadline && steps < frames.len() {
            self.active_frame_index = (self.active_frame_index + 1) % frames.len();
            deadline = deadline.saturating_add(frames[self.active_frame_index].duration_ns);
            changed = true;
            steps += 1;
        }
        if deadline <= now_ns {
            deadline = now_ns.saturating_add(frames[self.active_frame_index].duration_ns);
        }
        self.next_animation_deadline_ns = Some(deadline);

        if changed {
            self.needs_redraw = true;
        }
        changed
    }

    /// Return the next active animation deadline.
    ///
    /// # Returns
    ///
    /// The monotonic nanosecond timestamp for the next frame, or `None` for a
    /// static cursor or an animation that has not yet been scheduled.
    pub fn next_animation_deadline_ns(&self) -> Option<u64> {
        self.next_animation_deadline_ns
    }

    /// Set the pointer hotspot position directly.
    ///
    /// # Arguments
    ///
    /// * `x` - Screen-space hotspot x-coordinate.
    /// * `y` - Screen-space hotspot y-coordinate.
    /// * `screen_width` - Current output width.
    /// * `screen_height` - Current output height.
    ///
    /// # Returns
    ///
    /// `true` when the pointer position changed.
    pub fn set_position(&mut self, x: i32, y: i32, screen_width: u32, screen_height: u32) -> bool {
        let old_x = self.x;
        let old_y = self.y;
        self.x = x.max(0).min(screen_width as i32 - 1);
        self.y = y.max(0).min(screen_height as i32 - 1);
        let moved = old_x != self.x || old_y != self.y;
        if moved {
            self.needs_redraw = true;
        }
        moved
    }

    /// Update the pointer hotspot using relative movement.
    ///
    /// # Arguments
    ///
    /// * `dx` - Horizontal movement delta.
    /// * `dy` - Vertical movement delta.
    /// * `screen_width` - Current output width.
    /// * `screen_height` - Current output height.
    ///
    /// # Returns
    ///
    /// `true` when the pointer position changed.
    pub fn update_position(
        &mut self,
        dx: i32,
        dy: i32,
        screen_width: u32,
        screen_height: u32,
    ) -> bool {
        self.set_position(self.x + dx, self.y + dy, screen_width, screen_height)
    }

    /// Check whether cursor movement requires recomposition.
    ///
    /// # Returns
    ///
    /// `true` when the previous and current cursor regions need redrawing.
    pub fn needs_redraw(&self) -> bool {
        self.needs_redraw
    }

    /// Mark the current cursor position as presented.
    pub fn mark_drawn(&mut self) {
        self.needs_redraw = false;
        let (draw_x, draw_y) = self.draw_position();
        self.prev_draw_x = draw_x;
        self.prev_draw_y = draw_y;
        self.prev_width = self.width;
        self.prev_height = self.height;
    }

    /// Return the top-left screen position at which the image is drawn.
    ///
    /// # Returns
    ///
    /// Screen coordinates adjusted by the scaled image hotspot.
    pub fn draw_position(&self) -> (i32, i32) {
        self.draw_position_for(self.x, self.y)
    }

    fn draw_position_for(&self, x: i32, y: i32) -> (i32, i32) {
        (
            x.saturating_sub(self.hotspot_x as i32),
            y.saturating_sub(self.hotspot_y as i32),
        )
    }

    /// Return the scaled image hotspot.
    ///
    /// # Returns
    ///
    /// Hotspot coordinates relative to the scaled cursor image.
    #[cfg(test)]
    pub fn hotspot(&self) -> (u32, u32) {
        (self.hotspot_x, self.hotspot_y)
    }

    /// Draw the cursor into a BGRA buffer with optional clipping.
    ///
    /// Straight-alpha source pixels are composited over the destination using
    /// source-over blending.
    ///
    /// # Arguments
    ///
    /// * `buffer` - Destination pixel buffer.
    /// * `screen_width` - Destination width in pixels.
    /// * `screen_height` - Destination height in pixels.
    /// * `bytes_per_pixel` - Destination bytes per pixel; values below four are ignored.
    /// * `stride` - Destination row stride in bytes.
    /// * `clip_rect` - Optional screen-space clip rectangle.
    pub fn draw_to_buffer_direct_clipped(
        &self,
        buffer: &mut [u8],
        screen_width: u32,
        screen_height: u32,
        bytes_per_pixel: u32,
        stride: u32,
        clip_rect: Option<(i32, i32, u32, u32)>,
    ) {
        if bytes_per_pixel < 4 {
            return;
        }

        let (cursor_x, cursor_y) = self.draw_position();
        let clip = clip_rect
            .map(|(x, y, w, h)| (x, y, x.saturating_add(w as i32), y.saturating_add(h as i32)));
        let bgra = self.bgra_pixels();

        for y in 0..self.height {
            for x in 0..self.width {
                let source_offset = ((y as usize * self.width as usize) + x as usize) * 4;
                let source = [
                    bgra[source_offset],
                    bgra[source_offset + 1],
                    bgra[source_offset + 2],
                    bgra[source_offset + 3],
                ];
                if source[3] == 0 {
                    continue;
                }

                let screen_x = cursor_x.saturating_add(x as i32);
                let screen_y = cursor_y.saturating_add(y as i32);
                if screen_x < 0
                    || screen_x >= screen_width as i32
                    || screen_y < 0
                    || screen_y >= screen_height as i32
                {
                    continue;
                }
                if let Some((clip_x0, clip_y0, clip_x1, clip_y1)) = clip
                    && (screen_x < clip_x0
                        || screen_x >= clip_x1
                        || screen_y < clip_y0
                        || screen_y >= clip_y1)
                {
                    continue;
                }

                let destination_offset = (screen_y as usize)
                    .saturating_mul(stride as usize)
                    .saturating_add((screen_x as usize).saturating_mul(bytes_per_pixel as usize));
                if destination_offset + 4 <= buffer.len() {
                    blend_straight_bgra(
                        &mut buffer[destination_offset..destination_offset + 4],
                        source,
                    );
                }
            }
        }
    }

    /// Return the scaled cursor image as straight-alpha BGRA pixels.
    ///
    /// # Returns
    ///
    /// A tightly packed BGRA slice with `width * height * 4` bytes.
    pub fn bgra_pixels(&self) -> &[u8] {
        &self.images[self.active_index].frames[self.active_frame_index].bgra
    }

    /// Return damage covering both the previous and current cursor images.
    ///
    /// # Returns
    ///
    /// A screen-space rectangle padded for conservative partial redraw.
    pub fn get_dirty_region(&self) -> (i32, i32, u32, u32) {
        let (current_x, current_y) = self.draw_position();
        let min_x = self
            .prev_draw_x
            .min(current_x)
            .saturating_sub(CURSOR_DAMAGE_PADDING);
        let min_y = self
            .prev_draw_y
            .min(current_y)
            .saturating_sub(CURSOR_DAMAGE_PADDING);
        let max_x = self
            .prev_draw_x
            .saturating_add(self.prev_width as i32)
            .max(current_x.saturating_add(self.width as i32))
            .saturating_add(CURSOR_DAMAGE_PADDING);
        let max_y = self
            .prev_draw_y
            .saturating_add(self.prev_height as i32)
            .max(current_y.saturating_add(self.height as i32))
            .saturating_add(CURSOR_DAMAGE_PADDING);
        (
            min_x,
            min_y,
            max_x.saturating_sub(min_x) as u32,
            max_y.saturating_sub(min_y) as u32,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Cursor, CursorPixels, DecodedCursorFrame, DecodedCursorImage, blend_straight_bgra,
    };
    use std::vec;
    use sws_protocol::CursorIcon;

    #[test]
    fn blends_straight_alpha_over_opaque_destination() {
        let mut destination = [0, 0, 0, 255];
        blend_straight_bgra(&mut destination, [255, 255, 255, 128]);
        assert_eq!(destination, [128, 128, 128, 255]);
    }

    #[test]
    fn scales_hotspot_with_cursor_image() {
        let source = CursorPixels {
            width: 2,
            height: 2,
            bgra: vec![255; 16],
        };
        let mut cursor = Cursor::from_source(source, 2000, 1, 1).expect("valid cursor");
        cursor.x = 10;
        cursor.y = 12;
        assert_eq!(cursor.width, 4);
        assert_eq!(cursor.height, 4);
        assert_eq!(cursor.hotspot(), (2, 2));
        assert_eq!(cursor.draw_position(), (8, 10));
    }

    #[test]
    fn switching_icons_updates_geometry_and_texture_generation() {
        let source = CursorPixels {
            width: 2,
            height: 2,
            bgra: vec![255; 16],
        };
        let mut cursor = Cursor::from_source(source, 1000, 0, 0).expect("valid arrow");
        let pointer = Cursor::scaled_image_from_source(
            CursorIcon::Pointer,
            CursorPixels {
                width: 4,
                height: 6,
                bgra: vec![255; 4 * 6 * 4],
            },
            1000,
            2,
            3,
        )
        .expect("valid pointer");
        cursor.images.push(pointer);

        let generation = cursor.texture_generation();
        assert!(cursor.set_icon(CursorIcon::Pointer));
        assert_eq!(cursor.icon(), CursorIcon::Pointer);
        assert_eq!((cursor.width, cursor.height), (4, 6));
        assert_eq!(cursor.hotspot(), (2, 3));
        assert_ne!(cursor.texture_generation(), generation);
        assert!(!cursor.set_icon(CursorIcon::Pointer));
    }

    #[test]
    fn animation_uses_per_frame_deadlines() {
        let mut cursor = Cursor::from_decoded(
            DecodedCursorImage {
                frames: vec![
                    DecodedCursorFrame {
                        pixels: CursorPixels {
                            width: 1,
                            height: 1,
                            bgra: vec![1, 1, 1, 255],
                        },
                        duration_ns: 10,
                    },
                    DecodedCursorFrame {
                        pixels: CursorPixels {
                            width: 1,
                            height: 1,
                            bgra: vec![2, 2, 2, 255],
                        },
                        duration_ns: 20,
                    },
                ],
            },
            1000,
            0,
            0,
        )
        .expect("valid animation");

        let image_generation = cursor.texture_generation();
        assert_eq!(cursor.frame_count(), 2);
        assert_eq!(cursor.active_frame_index(), 0);
        assert_eq!(cursor.frame_bgra_pixels(0).expect("first frame")[0], 1);
        assert!(cursor.frame_bgra_pixels(2).is_none());

        assert!(!cursor.advance_animation(100));
        assert_eq!(cursor.next_animation_deadline_ns(), Some(110));
        assert!(!cursor.advance_animation(109));
        assert!(cursor.advance_animation(110));
        assert_eq!(cursor.active_frame_index(), 1);
        assert_eq!(cursor.bgra_pixels()[0], 2);
        assert_eq!(cursor.texture_generation(), image_generation);
        assert_eq!(cursor.next_animation_deadline_ns(), Some(130));
    }
}
