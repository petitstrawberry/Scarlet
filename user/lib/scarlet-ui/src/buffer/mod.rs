use std::vec::Vec;
use crate::geometry::{Rect, Size};

#[derive(Clone)]
pub struct Buffer {
    data: Vec<u8>, // RGBA
    size: Size,
    stride: usize,
}

pub type BufferRef = Buffer;

impl Buffer {
    pub fn new(size: Size) -> Self {
        let width = libm::ceilf(size.width) as usize;
        let height = libm::ceilf(size.height) as usize;
        let stride = width * 4; // RGBA
        let data = std::vec![0; stride * height];

        Self {
            data,
            size,
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

    /// Fill a rectangle with a solid color in LOCAL coordinates
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

        for y in y_start..y_end {
            let offset = y * self.stride + x_start * 4;
            for x in x_start..x_end {
                let i = offset + (x - x_start) * 4;
                self.data[i] = color[0]; // R
                self.data[i + 1] = color[1]; // G
                self.data[i + 2] = color[2]; // B
                self.data[i + 3] = color[3]; // A
            }
        }
    }

    /// Blit from another buffer at the specified position in LOCAL coordinates
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
            let src_offset = y * src.stride;
            let dest_offset = (dest_y + y) * self.stride + dest_x * 4;
            let row = &src.data[src_offset..src_offset + copy_width * 4];
            self.data[dest_offset..dest_offset + copy_width * 4].copy_from_slice(row);
        }
    }

    /// Clear the entire buffer to transparent black
    pub fn clear(&mut self) {
        self.data.fill(0);
    }
}
