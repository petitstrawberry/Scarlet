//! View buffer management
//!
//! This module provides ViewBuffer, which represents an offscreen buffer
//! for rendering views. Buffers are used for repaint boundaries and
//! for efficient composition.

extern crate alloc;
use alloc::vec;
use alloc::vec::Vec;

use crate::graphics::Canvas;
use crate::layout::Size;
use scarlet_std::fmt;

/// Offscreen buffer for view rendering
///
/// ViewBuffer represents a pixel buffer that can be drawn to and composited.
/// It's used for repaint boundaries to isolate redraws.
pub struct ViewBuffer {
    /// Pixel data (RGBA format)
    data: Vec<u8>,
    /// Buffer size
    size: Size,
    /// Canvas for drawing to this buffer
    canvas: Option<Canvas<'static>>,
}

impl ViewBuffer {
    /// Create a new buffer with the specified size
    pub fn new(size: Size) -> Self {
        let pixel_count = (size.width * size.height) as usize;
        let data = vec![0; pixel_count * 4]; // RGBA = 4 bytes per pixel

        Self {
            data,
            size,
            canvas: None,
        }
    }

    /// Create a new buffer with allocated data
    pub fn with_data(size: Size, data: Vec<u8>) -> Self {
        assert_eq!(data.len(), (size.width * size.height) as usize * 4);
        Self {
            data,
            size,
            canvas: None,
        }
    }

    /// Get the buffer size
    pub fn size(&self) -> Size {
        self.size
    }

    /// Get the raw pixel data
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// Get mutable access to the raw pixel data
    pub fn data_mut(&mut self) -> &mut [u8] {
        &mut self.data
    }

    /// Check if the buffer is large enough for the given size
    pub fn can_fit(&self, size: Size) -> bool {
        self.size.width >= size.width && self.size.height >= size.height
    }

    /// Resize the buffer (grow-only strategy)
    ///
    /// This will reallocate if the new size is larger than the current size.
    /// If the new size is smaller, the buffer is not shrunk (grow-only).
    pub fn resize(&mut self, new_size: Size) -> bool {
        if new_size.width <= self.size.width && new_size.height <= self.size.height {
            return false; // No resize needed (grow-only)
        }

        let new_pixel_count = (new_size.width * new_size.height) as usize;
        self.data = vec![0; new_pixel_count * 4];
        self.size = new_size;
        self.canvas = None; // Canvas needs to be recreated
        true
    }

    /// Clear the buffer to transparent black
    pub fn clear(&mut self) {
        self.data.fill(0);
    }

    /// Fill the buffer with a solid color
    pub fn fill(&mut self, color: [u8; 4]) {
        let pixel_count = (self.size.width * self.size.height) as usize;
        for i in 0..pixel_count {
            self.data[i * 4] = color[0];
            self.data[i * 4 + 1] = color[1];
            self.data[i * 4 + 2] = color[2];
            self.data[i * 4 + 3] = color[3];
        }
    }

    /// Get the canvas for drawing to this buffer
    ///
    /// This creates a canvas if one doesn't exist, or returns the existing one.
    pub fn canvas<'a>(&'a mut self) -> Option<&mut Canvas<'a>> {
        if self.canvas.is_none() {
            // Create canvas from buffer data
            // This is a placeholder - actual implementation depends on graphics backend
            self.canvas = None;
        }
        // Note: This is a simplified implementation
        // In practice, we'd need to handle lifetimes properly
        None
    }

    /// Calculate the memory usage in bytes
    pub fn memory_usage(&self) -> usize {
        self.data.len()
    }
}

impl fmt::Debug for ViewBuffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ViewBuffer")
            .field("size", &self.size)
            .field("memory_usage", &self.memory_usage())
            .finish()
    }
}

impl Clone for ViewBuffer {
    fn clone(&self) -> Self {
        Self {
            data: self.data.clone(),
            size: self.size,
            canvas: None, // Canvas cannot be cloned
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_view_buffer_new() {
        let buffer = ViewBuffer::new(Size::new(100, 100));
        assert_eq!(buffer.size(), Size::new(100, 100));
        assert_eq!(buffer.data().len(), 100 * 100 * 4);
    }

    #[test]
    fn test_view_buffer_clear() {
        let mut buffer = ViewBuffer::new(Size::new(10, 10));
        buffer.fill([255, 0, 0, 255]);
        buffer.clear();
        assert!(buffer.data().iter().all(|&x| x == 0));
    }

    #[test]
    fn test_view_buffer_fill() {
        let mut buffer = ViewBuffer::new(Size::new(10, 10));
        buffer.fill([255, 128, 64, 255]);

        // Check first pixel
        assert_eq!(buffer.data()[0], 255);
        assert_eq!(buffer.data()[1], 128);
        assert_eq!(buffer.data()[2], 64);
        assert_eq!(buffer.data()[3], 255);
    }

    #[test]
    fn test_view_buffer_can_fit() {
        let buffer = ViewBuffer::new(Size::new(100, 100));
        assert!(buffer.can_fit(Size::new(50, 50)));
        assert!(buffer.can_fit(Size::new(100, 100)));
        assert!(!buffer.can_fit(Size::new(101, 100)));
        assert!(!buffer.can_fit(Size::new(100, 101)));
    }

    #[test]
    fn test_view_buffer_resize_grow() {
        let mut buffer = ViewBuffer::new(Size::new(100, 100));
        let original_len = buffer.data().len();

        // Growing should reallocate
        assert!(buffer.resize(Size::new(200, 200)));
        assert_eq!(buffer.size(), Size::new(200, 200));
        assert!(buffer.data().len() > original_len);
    }

    #[test]
    fn test_view_buffer_resize_shrink() {
        let mut buffer = ViewBuffer::new(Size::new(100, 100));
        let original_len = buffer.data().len();

        // Shrinking should not reallocate (grow-only)
        assert!(!buffer.resize(Size::new(50, 50)));
        assert_eq!(buffer.size(), Size::new(100, 100)); // Size unchanged
        assert_eq!(buffer.data().len(), original_len); // Data unchanged
    }

    #[test]
    fn test_view_buffer_clone() {
        let mut buffer1 = ViewBuffer::new(Size::new(10, 10));
        buffer1.fill([255, 0, 0, 255]);

        let buffer2 = buffer1.clone();

        assert_eq!(buffer1.size(), buffer2.size());
        assert_eq!(buffer1.data(), buffer2.data());

        // Modifying clone should not affect original
        buffer2.clear();
        assert_ne!(buffer1.data(), buffer2.data());
    }

    #[test]
    fn test_view_buffer_memory_usage() {
        let buffer = ViewBuffer::new(Size::new(100, 50));
        assert_eq!(buffer.memory_usage(), 100 * 50 * 4);
    }
}
