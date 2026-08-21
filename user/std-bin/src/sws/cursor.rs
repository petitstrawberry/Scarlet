//! Mouse cursor image loading, scaling, and composition.

use png::{BitDepth, ColorType, Decoder, Limits, OutputInfo, Transformations};
use std::fs::File;
use std::io::BufReader;
use std::vec;
use std::vec::Vec;

const FALLBACK_CURSOR_WIDTH: usize = 16;
const FALLBACK_CURSOR_HEIGHT: usize = 24;
const CURSOR_DAMAGE_PADDING: i32 = 2;
const MAX_CURSOR_SOURCE_DIMENSION: u32 = 256;
const MAX_CURSOR_DECODE_BYTES: usize = 4 * 1024 * 1024;

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

struct CursorImage {
    icon: sws_protocol::CursorIcon,
    width: u32,
    height: u32,
    hotspot_x: u32,
    hotspot_y: u32,
    bgra: Vec<u8>,
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

fn decode_png_file(path: &str) -> Result<CursorPixels, &'static str> {
    let file = File::open(path).map_err(|_| "Failed to open cursor PNG")?;
    let mut decoder = Decoder::new(BufReader::new(file));
    decoder.set_limits(Limits {
        bytes: MAX_CURSOR_DECODE_BYTES,
    });
    decoder.set_transformations(Transformations::normalize_to_color8());

    let mut reader = decoder
        .read_info()
        .map_err(|_| "Failed to read cursor PNG header")?;
    let source_info = reader.info();
    if source_info.width == 0 || source_info.height == 0 {
        return Err("Cursor PNG has empty dimensions");
    }
    if source_info.width > MAX_CURSOR_SOURCE_DIMENSION
        || source_info.height > MAX_CURSOR_SOURCE_DIMENSION
    {
        return Err("Cursor PNG dimensions exceed 256x256");
    }
    let output_buffer_size = reader
        .output_buffer_size()
        .ok_or("Cursor PNG output buffer size overflow")?;
    if output_buffer_size > MAX_CURSOR_DECODE_BYTES {
        return Err("Cursor PNG output exceeds decode limit");
    }
    let mut decoded = vec![0; output_buffer_size];
    let info = reader
        .next_frame(&mut decoded)
        .map_err(|_| "Failed to decode cursor PNG")?;

    if info.bit_depth != BitDepth::Eight {
        return Err("Cursor PNG did not normalize to 8-bit color");
    }

    decoded.truncate(info.buffer_size());
    decoded_frame_to_bgra(&decoded, info)
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
        let source = decode_png_file(path)?;
        Self::from_source(source, scale_milli, hotspot_x, hotspot_y)
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
        let image = Self::scaled_image_from_source(
            sws_protocol::CursorIcon::Arrow,
            source,
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
            texture_generation: 1,
            needs_redraw: true,
        })
    }

    fn scaled_image_from_source(
        icon: sws_protocol::CursorIcon,
        source: CursorPixels,
        scale_milli: u32,
        hotspot_x: u32,
        hotspot_y: u32,
    ) -> Result<CursorImage, &'static str> {
        let expected_len = (source.width as usize)
            .checked_mul(source.height as usize)
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or("Cursor image dimensions overflow")?;
        if source.width == 0 || source.height == 0 || source.bgra.len() != expected_len {
            return Err("Cursor image buffer is invalid");
        }
        if hotspot_x >= source.width || hotspot_y >= source.height {
            return Err("Cursor hotspot is outside the source image");
        }

        let width = scaled_len(source.width, scale_milli);
        let height = scaled_len(source.height, scale_milli);
        let hotspot_x = scaled_hotspot(hotspot_x, source.width, width);
        let hotspot_y = scaled_hotspot(hotspot_y, source.height, height);
        let bgra = scale_bgra_nearest(&source, width, height)?;

        Ok(CursorImage {
            icon,
            width,
            height,
            hotspot_x,
            hotspot_y,
            bgra,
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
        let source = decode_png_file(path)?;
        let image =
            Self::scaled_image_from_source(icon, source, scale_milli, hotspot_x, hotspot_y)?;
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
        &self.images[self.active_index].bgra
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
    use super::{Cursor, CursorPixels, blend_straight_bgra};
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
}
