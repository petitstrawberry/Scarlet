use std::vec::Vec;
use crate::geometry::{Rect, Size};

#[derive(Clone)]
pub struct Buffer {
    data: Vec<u8>, // BGRA (matches framebuffer format)
    size: Size,
    stride: usize,
}

pub type BufferRef = Buffer;

impl Buffer {
    pub fn new(size: Size) -> Self {
        // Clamp size to reasonable maximum to prevent overflow
        // Use i32::MAX as reasonable limit (covers all practical screen sizes)
        const MAX_DIM: f32 = 65536.0; // 64K pixels is more than enough
        let clamped_size = Size::new(
            size.width.clamp(0.0, MAX_DIM),
            size.height.clamp(0.0, MAX_DIM),
        );

        let width = libm::ceilf(clamped_size.width) as usize;
        let height = libm::ceilf(clamped_size.height) as usize;

        // Ensure minimum 1x1 size
        let width = width.max(1);
        let height = height.max(1);

        let stride = width * 4; // BGRA

        // Check for overflow in stride calculation
        let data_len = stride.saturating_mul(height);
        let data = std::vec![0; data_len];

        std::println!("[Buffer::new] size={:?} -> clamped={:?} width={} height={} stride={} data.len()={}",
                      size, clamped_size, width, height, stride, data.len());

        Self {
            data,
            size: clamped_size,
            stride,
        }
    }

    pub fn size(&self) -> Size {
        self.size
    }

    pub fn width(&self) -> usize {
        libm::ceilf(self.size.width) as usize
    }

    pub fn height(&self) -> usize {
        libm::ceilf(self.size.height) as usize
    }

    pub fn stride(&self) -> usize {
        self.stride
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.data
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut self.data
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// Fill a rectangle with a solid color in LOCAL coordinates
    /// color: [u8; 4] should be in BGRA format
    pub fn fill_rect(&mut self, rect: Rect, color: [u8; 4]) {
        let x_start = libm::ceilf(rect.origin.x) as usize;
        let y_start = libm::ceilf(rect.origin.y) as usize;
        let x_end = libm::ceilf(rect.origin.x + rect.size.width) as usize;
        let y_end = libm::ceilf(rect.origin.y + rect.size.height) as usize;

        // Clamp to buffer bounds
        let x_start = x_start.clamp(0, self.width());
        let x_end = x_end.clamp(0, self.width());
        let y_start = y_start.clamp(0, self.height());
        let y_end = y_end.clamp(0, self.height());

        // Skip if empty
        if x_start >= x_end || y_start >= y_end {
            return;
        }

        // Check bounds before accessing data
        let row_width = (x_end - x_start) * 4;
        for y in y_start..y_end {
            let offset = y * self.stride + x_start * 4;
            if offset + row_width > self.data.len() {
                break; // Safety: avoid out-of-bounds access
            }
            for x in x_start..x_end {
                let i = offset + (x - x_start) * 4;
                if i + 4 <= self.data.len() {
                    self.data[i] = color[0]; // B
                    self.data[i + 1] = color[1]; // G
                    self.data[i + 2] = color[2]; // R
                    self.data[i + 3] = color[3]; // A
                }
            }
        }
    }

    /// Blit from another buffer at the specified position in LOCAL coordinates
    /// Performs alpha blending for transparent pixels
    pub fn blit_from(&mut self, src: &Buffer, dest_rect: Rect) {
        let src_width = src.width();
        let src_height = src.height();

        let dest_x = libm::ceilf(dest_rect.origin.x) as usize;
        let dest_y = libm::ceilf(dest_rect.origin.y) as usize;
        let dest_width = libm::ceilf(dest_rect.size.width) as usize;
        let dest_height = libm::ceilf(dest_rect.size.height) as usize;

        // Clamp to buffer bounds
        let dest_x = dest_x.clamp(0, self.width());
        let dest_y = dest_y.clamp(0, self.height());

        let copy_width = dest_width
            .min(src_width)
            .min(self.width() - dest_x);
        let copy_height = dest_height
            .min(src_height)
            .min(self.height() - dest_y);

        for y in 0..copy_height {
            for x in 0..copy_width {
                let src_offset = y * src.stride + x * 4;
                let dest_offset = (dest_y + y) * self.stride + (dest_x + x) * 4;

                let src_b = src.data[src_offset];
                let src_g = src.data[src_offset + 1];
                let src_r = src.data[src_offset + 2];
                let src_a = src.data[src_offset + 3];

                // Alpha blending
                if src_a == 255 {
                    // Opaque: copy directly
                    self.data[dest_offset] = src_b;
                    self.data[dest_offset + 1] = src_g;
                    self.data[dest_offset + 2] = src_r;
                    self.data[dest_offset + 3] = src_a;
                } else if src_a > 0 {
                    // Semi-transparent: blend with destination
                    let alpha = src_a as f32 / 255.0;
                    let inv_alpha = 1.0 - alpha;

                    self.data[dest_offset] = (src_b as f32 * alpha + self.data[dest_offset] as f32 * inv_alpha) as u8;
                    self.data[dest_offset + 1] = (src_g as f32 * alpha + self.data[dest_offset + 1] as f32 * inv_alpha) as u8;
                    self.data[dest_offset + 2] = (src_r as f32 * alpha + self.data[dest_offset + 2] as f32 * inv_alpha) as u8;
                    self.data[dest_offset + 3] = (src_a as f32 * alpha + self.data[dest_offset + 3] as f32 * inv_alpha) as u8;
                }
                // If alpha == 0, keep destination pixel unchanged
            }
        }
    }

    /// Clear the entire buffer to transparent black
    pub fn clear(&mut self) {
        self.data.fill(0);
    }
}
