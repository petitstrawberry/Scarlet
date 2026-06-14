//! Buffer - Pixel buffer management for ScarletUI
//!
//! Provides BGRA format pixel buffers with alpha blending support.

use crate::color::Color;
use crate::geometry::Size;
use alloc::vec;
use alloc::vec::Vec;

/// Pixel buffer in BGRA format
///
/// Each pixel is stored as a u32 in BGRA byte order:
/// - Byte 0: Blue
/// - Byte 1: Green
/// - Byte 2: Red
/// - Byte 3: Alpha
pub struct Buffer {
    width: u32,
    height: u32,
    logical_width: u32,
    logical_height: u32,
    scale_milli: u32,
    data: Vec<u32>,
}

impl Buffer {
    /// Create a new buffer with the given size
    pub fn new(size: Size) -> Self {
        let width = size.width as u32;
        let height = size.height as u32;
        Self {
            width,
            height,
            logical_width: width,
            logical_height: height,
            scale_milli: 1000,
            data: vec![0; (width * height) as usize],
        }
    }

    /// Create a buffer with explicit dimensions
    pub fn from_dimensions(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            logical_width: width,
            logical_height: height,
            scale_milli: 1000,
            data: vec![0; (width * height) as usize],
        }
    }

    /// Create a physical buffer for a logical size using the current UI scale.
    pub fn from_logical_dimensions(logical_width: u32, logical_height: u32) -> Self {
        Self::from_logical_dimensions_with_scale(
            logical_width,
            logical_height,
            crate::graphics::current_scale_milli(),
        )
    }

    /// Create a physical buffer for a logical size using an explicit UI scale.
    pub fn from_logical_dimensions_with_scale(
        logical_width: u32,
        logical_height: u32,
        scale_milli: u32,
    ) -> Self {
        let scale_milli = scale_milli.max(1);
        let width = Self::scale_len(logical_width, scale_milli);
        let height = Self::scale_len(logical_height, scale_milli);
        Self {
            width,
            height,
            logical_width,
            logical_height,
            scale_milli,
            data: vec![0; (width * height) as usize],
        }
    }

    fn scale_len(value: u32, scale_milli: u32) -> u32 {
        ((value as u64)
            .saturating_mul(scale_milli as u64)
            .saturating_add(999)
            / 1000)
            .max(1) as u32
    }

    /// Get the buffer width
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Get the buffer height
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Get the logical buffer width.
    pub fn logical_width(&self) -> u32 {
        self.logical_width
    }

    /// Get the logical buffer height.
    pub fn logical_height(&self) -> u32 {
        self.logical_height
    }

    /// Get the scale used to create this buffer in milli-units.
    pub fn scale_milli(&self) -> u32 {
        self.scale_milli
    }

    /// Get the buffer size
    pub fn size(&self) -> Size {
        Size {
            width: self.width as f32,
            height: self.height as f32,
        }
    }

    /// Get the pixel data as a slice
    pub fn as_slice(&self) -> &[u32] {
        &self.data
    }

    /// Get the pixel data as a mutable slice
    pub fn as_mut_slice(&mut self) -> &mut [u32] {
        &mut self.data
    }

    /// Clear the buffer with a color
    pub fn clear(&mut self, color: Color) {
        let pixel = color.to_bgra();
        for px in self.data.iter_mut() {
            *px = pixel;
        }
    }

    /// Clear a rectangle with a color
    pub fn clear_rect(&mut self, x: u32, y: u32, width: u32, height: u32, color: Color) {
        if width == 0 || height == 0 {
            return;
        }
        let pixel = color.to_bgra();
        let x_end = (x + width).min(self.width);
        let y_end = (y + height).min(self.height);
        for yy in y..y_end {
            let row_start = (yy * self.width + x) as usize;
            let row_end = (yy * self.width + x_end) as usize;
            for px in self.data[row_start..row_end].iter_mut() {
                *px = pixel;
            }
        }
    }

    /// Composite another buffer into this buffer
    ///
    /// # Arguments
    /// * `src` - Source buffer to composite
    /// * `dst_x` - Destination X position
    /// * `dst_y` - Destination Y position
    /// * `opacity` - Opacity multiplier (0.0 - 1.0)
    pub fn composite(&mut self, src: &Buffer, dst_x: i32, dst_y: i32, opacity: f32) {
        if crate::debug::is_enabled() {
            crate::logln!(
                "[Buffer] composite: src_size={}x{}, dst_pos=({},{}), opacity={}",
                src.width,
                src.height,
                dst_x,
                dst_y,
                opacity
            );
        }

        let mut pixels_composited = 0u32;

        for y in 0..src.height {
            for x in 0..src.width {
                let src_x = x as i32;
                let src_y = y as i32;
                let target_x = dst_x + src_x;
                let target_y = dst_y + src_y;

                // Check bounds
                if target_x >= 0
                    && target_x < self.width as i32
                    && target_y >= 0
                    && target_y < self.height as i32
                {
                    let src_pixel = src.data[(y * src.width + x) as usize];
                    let dst_idx = (target_y * self.width as i32 + target_x) as usize;

                    // Alpha blending
                    self.data[dst_idx] = Self::blend_pixels(self.data[dst_idx], src_pixel, opacity);
                    pixels_composited += 1;
                }
            }
        }

        if crate::debug::is_enabled() {
            crate::logln!(
                "[Buffer] composite: {} pixels composited",
                pixels_composited
            );
        }
    }

    /// Composite another buffer into this buffer with a clip rect in destination space.
    pub fn composite_clipped(
        &mut self,
        src: &Buffer,
        dst_x: i32,
        dst_y: i32,
        opacity: f32,
        clip_x: i32,
        clip_y: i32,
        clip_w: i32,
        clip_h: i32,
    ) {
        if clip_w <= 0 || clip_h <= 0 {
            return;
        }

        let src_w = src.width as i32;
        let src_h = src.height as i32;
        let dst_w = self.width as i32;
        let dst_h = self.height as i32;

        let dst_left = dst_x.max(clip_x).max(0);
        let dst_top = dst_y.max(clip_y).max(0);
        let dst_right = (dst_x + src_w).min(clip_x + clip_w).min(dst_w);
        let dst_bottom = (dst_y + src_h).min(clip_y + clip_h).min(dst_h);

        if dst_right <= dst_left || dst_bottom <= dst_top {
            return;
        }

        let mut pixels_composited = 0u32;
        for target_y in dst_top..dst_bottom {
            let src_y = target_y - dst_y;
            let src_row = (src_y * src.width as i32) as usize;
            let dst_row = (target_y * self.width as i32) as usize;
            for target_x in dst_left..dst_right {
                let src_x = target_x - dst_x;
                let src_pixel = src.data[(src_row + src_x as usize)];
                let dst_idx = dst_row + target_x as usize;
                self.data[dst_idx] = Self::blend_pixels(self.data[dst_idx], src_pixel, opacity);
                pixels_composited += 1;
            }
        }

        if crate::debug::is_enabled() {
            crate::logln!(
                "[Buffer] composite_clipped: {} pixels composited",
                pixels_composited
            );
        }
    }

    /// Composite another buffer into this buffer with a rounded clip rect in destination space.
    pub fn composite_clipped_rounded(
        &mut self,
        src: &Buffer,
        dst_x: i32,
        dst_y: i32,
        opacity: f32,
        clip_x: i32,
        clip_y: i32,
        clip_w: i32,
        clip_h: i32,
        radius: f32,
    ) {
        if clip_w <= 0 || clip_h <= 0 {
            return;
        }
        if radius <= 0.0 {
            self.composite_clipped(src, dst_x, dst_y, opacity, clip_x, clip_y, clip_w, clip_h);
            return;
        }

        let src_w = src.width as i32;
        let src_h = src.height as i32;
        let dst_w = self.width as i32;
        let dst_h = self.height as i32;

        let dst_left = dst_x.max(clip_x).max(0);
        let dst_top = dst_y.max(clip_y).max(0);
        let dst_right = (dst_x + src_w).min(clip_x + clip_w).min(dst_w);
        let dst_bottom = (dst_y + src_h).min(clip_y + clip_h).min(dst_h);

        if dst_right <= dst_left || dst_bottom <= dst_top {
            return;
        }

        let max_radius = (clip_w.min(clip_h) as f32) / 2.0;
        let radius = radius.max(0.0).min(max_radius);
        let radius_sq = radius * radius;

        for target_y in dst_top..dst_bottom {
            let src_y = target_y - dst_y;
            let src_row = (src_y * src.width as i32) as usize;
            let dst_row = (target_y * self.width as i32) as usize;
            let center_y = (target_y - clip_y) as f32 + 0.5;
            let dy_top = center_y;
            let dy_bottom = (clip_h as f32) - center_y;
            for target_x in dst_left..dst_right {
                let center_x = (target_x - clip_x) as f32 + 0.5;
                let dx_left = center_x;
                let dx_right = (clip_w as f32) - center_x;

                let in_center = (dx_left >= radius && dx_right >= radius)
                    || (dy_top >= radius && dy_bottom >= radius);
                if !in_center {
                    let corner_dx = radius - dx_left.min(dx_right);
                    let corner_dy = radius - dy_top.min(dy_bottom);
                    let dist_sq = corner_dx * corner_dx + corner_dy * corner_dy;
                    if dist_sq > radius_sq {
                        let dist = libm::sqrtf(dist_sq);
                        let coverage = (radius + 0.5 - dist).max(0.0).min(1.0);
                        if coverage <= 0.0 {
                            continue;
                        }
                        let src_x = target_x - dst_x;
                        let src_pixel = src.data[(src_row + src_x as usize)];
                        let dst_idx = dst_row + target_x as usize;
                        self.data[dst_idx] =
                            Self::blend_pixels(self.data[dst_idx], src_pixel, opacity * coverage);
                        continue;
                    }
                }

                let src_x = target_x - dst_x;
                let src_pixel = src.data[(src_row + src_x as usize)];
                let dst_idx = dst_row + target_x as usize;
                self.data[dst_idx] = Self::blend_pixels(self.data[dst_idx], src_pixel, opacity);
            }
        }
    }

    /// Blend two pixels with alpha
    ///
    /// Pixel format: 0xAARRGGBB in memory, becomes BGRA in little-endian
    fn blend_pixels(dst: u32, src: u32, opacity: f32) -> u32 {
        // Extract BGRA channels (format: 0xAARRGGBB)
        let dst_b = (dst & 0xFF) as u32;
        let dst_g = ((dst >> 8) & 0xFF) as u32;
        let dst_r = ((dst >> 16) & 0xFF) as u32;
        let dst_a = ((dst >> 24) & 0xFF) as u32;

        let src_b = (src & 0xFF) as u32;
        let src_g = ((src >> 8) & 0xFF) as u32;
        let src_r = ((src >> 16) & 0xFF) as u32;
        let src_a = ((src >> 24) & 0xFF) as u32;

        // If source is fully opaque, just copy
        if src_a == 255 && opacity >= 1.0 {
            return src;
        }

        // If source is fully transparent, keep destination
        if src_a == 0 || opacity <= 0.0 {
            return dst;
        }

        // Apply opacity to source alpha
        let src_alpha = (src_a as f32 * opacity).min(255.0);

        // Alpha blending: over operator
        // result = src * src_alpha + dst * (1 - src_alpha) / 255
        let inv_a = 255.0 - src_alpha;

        let b = (src_b as f32 * src_alpha + dst_b as f32 * inv_a) / 255.0;
        let g = (src_g as f32 * src_alpha + dst_g as f32 * inv_a) / 255.0;
        let r = (src_r as f32 * src_alpha + dst_r as f32 * inv_a) / 255.0;
        let a_final = (dst_a as f32 + (255.0 - dst_a as f32) * src_alpha / 255.0);

        // Clamp and pack as 0xAARRGGBB
        (b.min(255.0) as u32 & 0xFF)
            | ((g.min(255.0) as u32 & 0xFF) << 8)
            | ((r.min(255.0) as u32 & 0xFF) << 16)
            | ((a_final.min(255.0) as u32 & 0xFF) << 24)
    }

    /// Set a pixel at the given position
    ///
    /// # Arguments
    /// * `x` - X position
    /// * `y` - Y position
    /// * `pixel` - Pixel value in BGRA format
    pub fn set_pixel(&mut self, x: u32, y: u32, pixel: u32) {
        if x < self.width && y < self.height {
            self.data[(y * self.width + x) as usize] = pixel;
        }
    }

    /// Get a pixel at the given position
    ///
    /// Returns None if position is out of bounds
    pub fn get_pixel(&self, x: u32, y: u32) -> Option<u32> {
        if x < self.width && y < self.height {
            Some(self.data[(y * self.width + x) as usize])
        } else {
            None
        }
    }

    /// Fill a rectangle with a color
    pub fn fill_rect(&mut self, x: u32, y: u32, width: u32, height: u32, color: Color) {
        let pixel = color.to_bgra();
        for dy in 0..height {
            for dx in 0..width {
                let px = x + dx;
                let py = y + dy;
                if px < self.width && py < self.height {
                    self.data[(py * self.width + px) as usize] = pixel;
                }
            }
        }
    }

    /// Get the pixel data as a u8 slice (BGRA format)
    ///
    /// This converts the internal u32 slice to a u8 slice for compatibility
    /// with drawing APIs that expect byte-level access.
    pub fn data(&self) -> &[u8] {
        unsafe { core::slice::from_raw_parts(self.data.as_ptr() as *const u8, self.data.len() * 4) }
    }

    /// Get the pixel data as a mutable u8 slice (BGRA format)
    ///
    /// This converts the internal u32 slice to a u8 slice for compatibility
    /// with drawing APIs that expect byte-level access.
    pub fn data_mut(&mut self) -> &mut [u8] {
        unsafe {
            core::slice::from_raw_parts_mut(self.data.as_mut_ptr() as *mut u8, self.data.len() * 4)
        }
    }
}
