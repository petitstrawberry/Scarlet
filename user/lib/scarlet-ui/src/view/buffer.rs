//! View buffer management
//!
//! Each View maintains its own buffer for efficient rendering.

use scarlet_std::vec::Vec;

/// Per-view rendering buffer (BGRA format, 4 bytes per pixel)
pub struct ViewBuffer {
    data: Vec<u8>,
    width: u32,
    height: u32,
}

impl ViewBuffer {
    /// Create a new buffer with the given dimensions
    pub fn new(width: u32, height: u32) -> Self {
        let size = (width * height * 4) as usize;
        let mut data = Vec::new();
        data.resize(size, 0);
        Self { data, width, height }
    }

    /// Get buffer data (immutable)
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// Get buffer data (mutable)
    pub fn data_mut(&mut self) -> &mut [u8] {
        &mut self.data
    }

    /// Get buffer width
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Get buffer height
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Resize buffer if needed (returns true if resized)
    pub fn resize_if_needed(&mut self, width: u32, height: u32) -> bool {
        if width != self.width || height != self.height {
            let size = (width * height * 4) as usize;
            self.data.resize(size, 0);
            self.width = width;
            self.height = height;
            true
        } else {
            false
        }
    }

    /// Clear the buffer with transparent black
    pub fn clear(&mut self) {
        for byte in &mut self.data {
            *byte = 0;
        }
    }

    /// Get the size in bytes
    pub fn size_bytes(&self) -> usize {
        self.data.len()
    }
}
