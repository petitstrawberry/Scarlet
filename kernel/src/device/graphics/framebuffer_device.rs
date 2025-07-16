//! # Framebuffer Character Device Module
//!
//! This module provides character device interface for framebuffer access.
//! It integrates with the GraphicsManager to provide user-space access to
//! framebuffer resources through the standard character device interface.
//!
//! ## Overview
//!
//! The FramebufferCharDevice provides:
//! - Basic read/write operations to framebuffer memory
//! - Integration with GraphicsManager for resource management
//! - Standard character device interface for user programs
//! - Future support for ioctl operations and memory mapping

extern crate alloc;

use core::{any::Any};
use alloc::string::String;
use spin::Mutex;

use crate::device::{
    char::CharDevice,
    graphics::manager::GraphicsManager,
    Device, DeviceType,
};

/// Framebuffer character device implementation
/// 
/// This device provides character-based access to framebuffer memory.
/// It acts as a bridge between user-space programs and the graphics
/// hardware through the GraphicsManager.
pub struct FramebufferCharDevice {
    /// The logical name of the framebuffer (e.g., "fb0")
    fb_name: String,
    /// Current read/write position in the framebuffer
    position: Mutex<usize>,
}

impl FramebufferCharDevice {
    /// Create a new framebuffer character device
    ///
    /// # Arguments
    ///
    /// * `fb_name` - The logical name of the framebuffer to access
    ///
    /// # Returns
    ///
    /// A new FramebufferCharDevice instance
    pub fn new(fb_name: String) -> Self {
        Self {
            fb_name,
            position: Mutex::new(0),
        }
    }

    /// Get the framebuffer name this device represents
    pub fn get_framebuffer_name(&self) -> &str {
        &self.fb_name
    }

    /// Get the current position in the framebuffer
    pub fn get_position(&self) -> usize {
        *self.position.lock()
    }

    /// Set the current position in the framebuffer
    pub fn set_position(&self, pos: usize) {
        *self.position.lock() = pos;
    }

    /// Reset position to the beginning of the framebuffer
    pub fn reset_position(&self) {
        *self.position.lock() = 0;
    }
}

impl Device for FramebufferCharDevice {
    fn device_type(&self) -> DeviceType {
        DeviceType::Char
    }

    fn name(&self) -> &'static str {
        "framebuffer"
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn as_char_device(&self) -> Option<&dyn CharDevice> {
        Some(self)
    }
}

impl CharDevice for FramebufferCharDevice {
    /// Read a single byte from the framebuffer at the current position
    ///
    /// # Returns
    ///
    /// The byte at the current position, or None if at end of framebuffer
    fn read_byte(&self) -> Option<u8> {
        let graphics_manager = GraphicsManager::get_manager();
        let fb_resource = graphics_manager.get_framebuffer(&self.fb_name)?;
        
        let mut position = self.position.lock();
        let current_pos = *position;
        if current_pos >= fb_resource.size {
            return None; // End of framebuffer
        }

        // Read byte from framebuffer memory
        let byte = unsafe {
            let fb_ptr = fb_resource.physical_addr as *const u8;
            *fb_ptr.add(current_pos)
        };

        // Advance position
        *position = current_pos + 1;
        Some(byte)
    }

    /// Write a single byte to the framebuffer at the current position
    ///
    /// # Arguments
    ///
    /// * `byte` - The byte to write
    ///
    /// # Returns
    ///
    /// Result indicating success or failure
    fn write_byte(&self, byte: u8) -> Result<(), &'static str> {
        let graphics_manager = GraphicsManager::get_manager();
        let fb_resource = graphics_manager
            .get_framebuffer(&self.fb_name)
            .ok_or("Framebuffer not found")?;

        let mut position = self.position.lock();
        let current_pos = *position;
        if current_pos >= fb_resource.size {
            return Err("End of framebuffer reached");
        }

        // Write byte to framebuffer memory
        unsafe {
            let fb_ptr = fb_resource.physical_addr as *mut u8;
            *fb_ptr.add(current_pos) = byte;
        }

        // Advance position
        *position = current_pos + 1;
        Ok(())
    }

    /// Check if the device is ready for reading
    ///
    /// # Returns
    ///
    /// True if there is data available to read
    fn can_read(&self) -> bool {
        let graphics_manager = GraphicsManager::get_manager();
        if let Some(fb_resource) = graphics_manager.get_framebuffer(&self.fb_name) {
            *self.position.lock() < fb_resource.size
        } else {
            false
        }
    }

    /// Check if the device is ready for writing
    ///
    /// # Returns
    ///
    /// True if there is space available to write
    fn can_write(&self) -> bool {
        let graphics_manager = GraphicsManager::get_manager();
        if let Some(fb_resource) = graphics_manager.get_framebuffer(&self.fb_name) {
            *self.position.lock() < fb_resource.size
        } else {
            false
        }
    }

    /// Read multiple bytes from the framebuffer
    ///
    /// # Arguments
    ///
    /// * `buffer` - The buffer to read data into
    ///
    /// # Returns
    ///
    /// The number of bytes actually read
    fn read(&self, buffer: &mut [u8]) -> usize {
        let graphics_manager = GraphicsManager::get_manager();
        let fb_resource = match graphics_manager.get_framebuffer(&self.fb_name) {
            Some(resource) => resource,
            None => return 0,
        };

        let mut position = self.position.lock();
        let current_pos = *position;
        let available_bytes = fb_resource.size.saturating_sub(current_pos);
        let bytes_to_read = buffer.len().min(available_bytes);

        if bytes_to_read == 0 {
            return 0;
        }

        // Read bytes from framebuffer memory
        unsafe {
            let fb_ptr = fb_resource.physical_addr as *const u8;
            let src = fb_ptr.add(current_pos);
            core::ptr::copy_nonoverlapping(src, buffer.as_mut_ptr(), bytes_to_read);
        }

        // Update position
        *position = current_pos + bytes_to_read;
        bytes_to_read
    }

    /// Write multiple bytes to the framebuffer
    ///
    /// # Arguments
    ///
    /// * `buffer` - The buffer containing data to write
    ///
    /// # Returns
    ///
    /// Result containing the number of bytes written or an error
    fn write(&self, buffer: &[u8]) -> Result<usize, &'static str> {
        let graphics_manager = GraphicsManager::get_manager();
        let fb_resource = graphics_manager
            .get_framebuffer(&self.fb_name)
            .ok_or("Framebuffer not found")?;

        let mut position = self.position.lock();
        let current_pos = *position;
        let available_space = fb_resource.size.saturating_sub(current_pos);
        let bytes_to_write = buffer.len().min(available_space);

        if bytes_to_write == 0 {
            return Err("No space available in framebuffer");
        }

        // Write bytes to framebuffer memory
        unsafe {
            let fb_ptr = fb_resource.physical_addr as *mut u8;
            let dst = fb_ptr.add(current_pos);
            core::ptr::copy_nonoverlapping(buffer.as_ptr(), dst, bytes_to_write);
        }

        // Update position
        *position = current_pos + bytes_to_write;
        Ok(bytes_to_write)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::{
        graphics::{manager::GraphicsManager, GenericGraphicsDevice, FramebufferConfig, PixelFormat},
        Device,
    };
    use alloc::{string::ToString, sync::Arc};

    #[test_case]
    fn test_framebuffer_char_device_creation() {
        let device = FramebufferCharDevice::new("fb0".to_string());
        assert_eq!(device.get_framebuffer_name(), "fb0");
        assert_eq!(device.get_position(), 0);
        assert_eq!(device.device_type(), DeviceType::Char);
        assert_eq!(device.name(), "framebuffer");
    }

    #[test_case]
    fn test_framebuffer_char_device_position() {
        let device = FramebufferCharDevice::new("fb0".to_string());
        
        // Test initial position
        assert_eq!(device.get_position(), 0);
        
        // Test setting position
        device.set_position(100);
        assert_eq!(device.get_position(), 100);
        
        // Test reset position
        device.reset_position();
        assert_eq!(device.get_position(), 0);
    }

    #[test_case]
    fn test_framebuffer_char_device_read_write() {
        // Setup graphics manager with a test framebuffer
        let mut graphics_manager = GraphicsManager::new();
        let mut test_device = GenericGraphicsDevice::new("test-gpu");
        let config = FramebufferConfig::new(100, 100, PixelFormat::RGBA8888);
        test_device.set_framebuffer_config(config.clone());
        
        // Allocate memory for test framebuffer
        let fb_size = config.size();
        let fb_pages = (fb_size + 4095) / 4096;
        let fb_addr = crate::mem::page::allocate_raw_pages(fb_pages) as usize;
        test_device.set_framebuffer_address(fb_addr);
        
        let shared_device: Arc<dyn Device> = Arc::new(test_device);
        graphics_manager.register_framebuffer_from_device("test_gpu", shared_device).unwrap();
        
        // Create framebuffer character device
        let char_device = FramebufferCharDevice::new("fb0".to_string());
        
        // Test write operation
        let test_data = [0x12, 0x34, 0x56, 0x78];
        let written = char_device.write(&test_data).unwrap();
        assert_eq!(written, 4);
        assert_eq!(char_device.get_position(), 4);
        
        // Reset position and test read operation
        char_device.reset_position();
        let mut read_buffer = [0u8; 4];
        let read_count = char_device.read(&mut read_buffer);
        assert_eq!(read_count, 4);
        assert_eq!(read_buffer, test_data);
        assert_eq!(char_device.get_position(), 4);
    }

    #[test_case]
    fn test_framebuffer_char_device_byte_operations() {
        // Setup graphics manager with a test framebuffer
        let mut graphics_manager = GraphicsManager::new();
        let mut test_device = GenericGraphicsDevice::new("test-gpu");
        let config = FramebufferConfig::new(50, 50, PixelFormat::RGB888);
        test_device.set_framebuffer_config(config.clone());
        
        let fb_size = config.size();
        let fb_pages = (fb_size + 4095) / 4096;
        let fb_addr = crate::mem::page::allocate_raw_pages(fb_pages) as usize;
        test_device.set_framebuffer_address(fb_addr);
        
        let shared_device: Arc<dyn Device> = Arc::new(test_device);
        graphics_manager.register_framebuffer_from_device("test_gpu", shared_device).unwrap();
        
        let char_device = FramebufferCharDevice::new("fb0".to_string());
        
        // Test single byte write
        assert!(char_device.write_byte(0xAB).is_ok());
        assert_eq!(char_device.get_position(), 1);
        
        // Test single byte read
        char_device.reset_position();
        let byte = char_device.read_byte().unwrap();
        assert_eq!(byte, 0xAB);
        assert_eq!(char_device.get_position(), 1);
    }

    #[test_case]
    fn test_framebuffer_char_device_can_read_write() {
        // Setup graphics manager with a small test framebuffer
        let mut graphics_manager = GraphicsManager::new();
        let mut test_device = GenericGraphicsDevice::new("test-gpu");
        let config = FramebufferConfig::new(10, 10, PixelFormat::RGB565); // Small framebuffer
        test_device.set_framebuffer_config(config.clone());
        
        let fb_size = config.size();
        let fb_pages = (fb_size + 4095) / 4096;
        let fb_addr = crate::mem::page::allocate_raw_pages(fb_pages) as usize;
        test_device.set_framebuffer_address(fb_addr);
        
        let shared_device: Arc<dyn Device> = Arc::new(test_device);
        graphics_manager.register_framebuffer_from_device("test_gpu", shared_device).unwrap();
        
        let char_device = FramebufferCharDevice::new("fb0".to_string());
        
        // Initially should be able to read and write
        assert!(char_device.can_read());
        assert!(char_device.can_write());
        
        // Move to end of framebuffer
        char_device.set_position(fb_size);
        
        // At end, should not be able to read or write
        assert!(!char_device.can_read());
        assert!(!char_device.can_write());
        
        // Move to just before end
        char_device.set_position(fb_size - 1);
        assert!(char_device.can_read());
        assert!(char_device.can_write());
    }

    #[test_case]
    fn test_framebuffer_char_device_boundary_conditions() {
        // Setup graphics manager with a test framebuffer
        let mut graphics_manager = GraphicsManager::new();
        let mut test_device = GenericGraphicsDevice::new("test-gpu");
        let config = FramebufferConfig::new(5, 5, PixelFormat::RGB888); // Very small framebuffer (75 bytes)
        test_device.set_framebuffer_config(config.clone());
        
        let fb_size = config.size();
        let fb_pages = (fb_size + 4095) / 4096;
        let fb_addr = crate::mem::page::allocate_raw_pages(fb_pages) as usize;
        test_device.set_framebuffer_address(fb_addr);
        
        let shared_device: Arc<dyn Device> = Arc::new(test_device);
        graphics_manager.register_framebuffer_from_device("test_gpu", shared_device).unwrap();
        
        let char_device = FramebufferCharDevice::new("fb0".to_string());
        
        // Write data until framebuffer is full
        let large_data = [0xFF; 100]; // More data than framebuffer can hold
        let written = char_device.write(&large_data).unwrap();
        assert_eq!(written, fb_size); // Should only write what fits
        assert_eq!(char_device.get_position(), fb_size);
        
        // Try to write more - should fail
        assert!(char_device.write_byte(0x00).is_err());
        
        // Reset and read all data
        char_device.reset_position();
        let mut read_buffer = [0u8; 100];
        let read_count = char_device.read(&mut read_buffer);
        assert_eq!(read_count, fb_size);
        
        // Verify all read data is 0xFF
        for i in 0..fb_size {
            assert_eq!(read_buffer[i], 0xFF);
        }
        
        // Try to read more - should return None
        assert!(char_device.read_byte().is_none());
    }

    #[test_case]
    fn test_framebuffer_char_device_non_existent_framebuffer() {
        let char_device = FramebufferCharDevice::new("non_existent".to_string());
        
        // Operations on non-existent framebuffer should fail gracefully
        assert!(!char_device.can_read());
        assert!(!char_device.can_write());
        assert!(char_device.read_byte().is_none());
        assert!(char_device.write_byte(0x00).is_err());
        
        let mut buffer = [0u8; 10];
        assert_eq!(char_device.read(&mut buffer), 0);
        assert!(char_device.write(&[0x00, 0x01]).is_err());
    }
}