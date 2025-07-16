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
use alloc::{string::String, sync::Arc};
use spin::Mutex;

use crate::device::{
    char::CharDevice, graphics::manager::{FramebufferResource, GraphicsManager}, manager::DeviceManager, Device, DeviceType
};

/// Framebuffer character device implementation
/// 
/// This device provides character-based access to framebuffer memory.
/// It acts as a bridge between user-space programs and the graphics
/// hardware through the GraphicsManager.
pub struct FramebufferCharDevice {
    /// Current read/write position in the framebuffer
    position: Mutex<usize>,
    /// The framebuffer resource this device represents
    fb_resource: Arc<FramebufferResource>,
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
    pub fn new(fb_resource: Arc<FramebufferResource>) -> Self {
        Self {
            fb_resource,
            position: Mutex::new(0),
        }
    }

    /// Get the framebuffer name this device represents
    pub fn get_framebuffer_name(&self) -> &str {
        &self.fb_resource.logical_name
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
        let fb_resource = &self.fb_resource;

        // Check if framebuffer address is valid
        if fb_resource.physical_addr == 0 {
            return None;
        }

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
        let fb_resource = &self.fb_resource;

        // Check if framebuffer address is valid
        if fb_resource.physical_addr == 0 {
            return Err("Invalid framebuffer address");
        }

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

        {
            // crate::println!("FramebufferCharDevice: framebuffer device: {}", self.fb_resource.source_device_id);
            let device = DeviceManager::get_mut_manager()
                .get_device(self.fb_resource.source_device_id)
                .expect("Framebuffer device should exist");
            // Notify the device manager that data was written
            let _ = device.as_graphics_device()
                .expect("Device should be a graphics device")
                .flush_framebuffer(0, 0, fb_resource.config.width, fb_resource.config.height);
        }

        Ok(())
    }

    /// Check if the device is ready for reading
    ///
    /// # Returns
    ///
    /// True if there is data available to read
    fn can_read(&self) -> bool {
        let fb_resource = &self.fb_resource;
        fb_resource.physical_addr != 0 && *self.position.lock() < fb_resource.size
    }

    /// Check if the device is ready for writing
    ///
    /// # Returns
    ///
    /// True if there is space available to write
    fn can_write(&self) -> bool {
        let fb_resource = &self.fb_resource;
        fb_resource.physical_addr != 0 && *self.position.lock() < fb_resource.size
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
        let fb_resource = &self.fb_resource;

        // Check if framebuffer address is valid
        if fb_resource.physical_addr == 0 {
            return 0;
        }

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
        let fb_resource = &self.fb_resource;

        // Check if framebuffer address is valid
        if fb_resource.physical_addr == 0 {
            return Err("Invalid framebuffer address");
        }

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

        {
            let device = DeviceManager::get_mut_manager()
                .get_device(self.fb_resource.source_device_id)
                .expect("Framebuffer device should exist");
            // Notify the device manager that data was written
            let _ = device.as_graphics_device()
                .expect("Device should be a graphics device")
                .flush_framebuffer(0, 0, fb_resource.config.width, fb_resource.config.height);
        }
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
    use spin::RwLock;

    /// Test utility to setup a clean global GraphicsManager for each test
    fn setup_clean_graphics_manager() -> &'static mut GraphicsManager {
        let manager = GraphicsManager::get_mut_manager();
        // Clear any existing state from previous tests
        manager.clear_for_test();
        manager
    }

    /// Create a test FramebufferResource for testing
    fn create_test_framebuffer_resource(logical_name: &str) -> FramebufferResource {
        let config = FramebufferConfig::new(800, 600, PixelFormat::RGBA8888);
        FramebufferResource::new(
            0,
            logical_name.to_string(),
            config,
            0x80000000, // test physical address
            800 * 600 * 4, // size
        )
    }

    #[test_case]
    fn test_framebuffer_char_device_creation() {
        // Create test framebuffer resource
        let config = FramebufferConfig::new(1024, 768, PixelFormat::RGBA8888);
        let fb_resource = Arc::new(FramebufferResource::new(
            0,
            "fb0".to_string(),
            config,
            0x80000000,
            1024 * 768 * 4,
        ));

        let device = FramebufferCharDevice::new(fb_resource);
        assert_eq!(device.get_framebuffer_name(), "fb0");
        assert_eq!(device.get_position(), 0);
        assert_eq!(device.device_type(), DeviceType::Char);
        assert_eq!(device.name(), "framebuffer");
    }

    #[test_case]
    fn test_framebuffer_char_device_position() {
        // Create test framebuffer resource
        let config = FramebufferConfig::new(1024, 768, PixelFormat::RGBA8888);
        let fb_resource = Arc::new(FramebufferResource::new(
            0,
            "fb0".to_string(),
            config,
            0x80000000,
            1024 * 768 * 4,
        ));

        let device = FramebufferCharDevice::new(fb_resource);
        
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
        // Setup clean graphics manager for this test
        let graphics_manager = setup_clean_graphics_manager();
        let mut test_device = GenericGraphicsDevice::new("test-gpu-read-write");
        let config = FramebufferConfig::new(100, 100, PixelFormat::RGBA8888);
        test_device.set_framebuffer_config(config.clone());
        
        // Allocate memory for test framebuffer
        let fb_size = config.size();
        let fb_pages = (fb_size + 4095) / 4096;
        let fb_addr = crate::mem::page::allocate_raw_pages(fb_pages) as usize;
        test_device.set_framebuffer_address(fb_addr);
        
        let shared_device: Arc<dyn Device> = Arc::new(test_device);
        graphics_manager.register_framebuffer_from_device(0, shared_device).unwrap();

        // Get the framebuffer resource that was assigned to this specific device
        let fb_resource = {
            let fb_names = graphics_manager.get_framebuffer_names();
            let fb_name = fb_names.iter()
                .find(|name| {
                    if let Some(fb_resource) = graphics_manager.get_framebuffer(name) {
                        fb_resource.source_device_id == 0
                    } else {
                        false
                    }
                })
                .expect("Should have framebuffer for this device");
            graphics_manager.get_framebuffer(fb_name).expect("Framebuffer should exist")
        };
        let char_device = FramebufferCharDevice::new(fb_resource);
        
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
        // Setup clean graphics manager for this test
        let graphics_manager = setup_clean_graphics_manager();
        let mut test_device = GenericGraphicsDevice::new("test-gpu-byte-ops");
        let config = FramebufferConfig::new(50, 50, PixelFormat::RGB888);
        test_device.set_framebuffer_config(config.clone());
        
        let fb_size = config.size();
        let fb_pages = (fb_size + 4095) / 4096;
        let fb_addr = crate::mem::page::allocate_raw_pages(fb_pages) as usize;
        test_device.set_framebuffer_address(fb_addr);
        
        let shared_device: Arc<dyn Device> = Arc::new(test_device);
        graphics_manager.register_framebuffer_from_device(0, shared_device).unwrap();

        // Get the framebuffer resource that was assigned to this specific device
        let fb_resource = {
            let fb_names = graphics_manager.get_framebuffer_names();
            let fb_name = fb_names.iter()
                .find(|name| {
                    if let Some(fb_resource) = graphics_manager.get_framebuffer(name) {
                        fb_resource.source_device_id == 0
                    } else {
                        false
                    }
                })
                .expect("Should have framebuffer for this device");
            graphics_manager.get_framebuffer(fb_name).expect("Framebuffer should exist")
        };
        let char_device = FramebufferCharDevice::new(fb_resource);
        
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
        // Setup clean graphics manager for this test
        let graphics_manager = setup_clean_graphics_manager();
        let mut test_device = GenericGraphicsDevice::new("test-gpu-can-rw");
        let config = FramebufferConfig::new(10, 10, PixelFormat::RGB565); // Small framebuffer
        test_device.set_framebuffer_config(config.clone());
        
        let fb_size = config.size();
        let fb_pages = (fb_size + 4095) / 4096;
        let fb_addr = crate::mem::page::allocate_raw_pages(fb_pages) as usize;
        test_device.set_framebuffer_address(fb_addr);
        
        let shared_device: Arc<dyn Device> = Arc::new(test_device);
        graphics_manager.register_framebuffer_from_device(0, shared_device).unwrap();

        // Get the framebuffer resource that was assigned to this specific device
        let fb_resource = {
            let fb_names = graphics_manager.get_framebuffer_names();
            let fb_name = fb_names.iter()
                .find(|name| {
                    if let Some(fb_resource) = graphics_manager.get_framebuffer(name) {
                        fb_resource.source_device_id == 0
                    } else {
                        false
                    }
                })
                .expect("Should have framebuffer for this device");
            graphics_manager.get_framebuffer(fb_name).expect("Framebuffer should exist")
        };
        let char_device = FramebufferCharDevice::new(fb_resource);
        
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
        // Setup clean graphics manager for this test
        let graphics_manager = setup_clean_graphics_manager();
        let mut test_device = GenericGraphicsDevice::new("test-gpu-boundary");
        let config = FramebufferConfig::new(5, 5, PixelFormat::RGB888); // Very small framebuffer (75 bytes)
        test_device.set_framebuffer_config(config.clone());
        
        let fb_size = config.size();
        let fb_pages = (fb_size + 4095) / 4096;
        let fb_addr = crate::mem::page::allocate_raw_pages(fb_pages) as usize;
        test_device.set_framebuffer_address(fb_addr);
        
        let shared_device: Arc<dyn Device> = Arc::new(test_device);
        graphics_manager.register_framebuffer_from_device(0, shared_device).unwrap();

        // Get the framebuffer resource that was assigned to this specific device
        let fb_resource = {
            let fb_names = graphics_manager.get_framebuffer_names();
            let fb_name = fb_names.iter()
                .find(|name| {
                    if let Some(fb_resource) = graphics_manager.get_framebuffer(name) {
                        fb_resource.source_device_id == 0
                    } else {
                        false
                    }
                })
                .expect("Should have framebuffer for this device");
            graphics_manager.get_framebuffer(fb_name).expect("Framebuffer should exist")
        };
        let char_device = FramebufferCharDevice::new(fb_resource);
        
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
    fn test_framebuffer_char_device_invalid_framebuffer() {
        // Create an invalid framebuffer resource (zero address)
        let invalid_config = FramebufferConfig::new(10, 10, PixelFormat::RGB888);
        let invalid_resource = Arc::new(FramebufferResource {
            source_device_id: 0,
            logical_name: "invalid".to_string(),
            config: invalid_config.clone(),
            physical_addr: 0, // Invalid address
            size: invalid_config.size(),
            created_char_device_id: RwLock::new(None),
        });

        let char_device = FramebufferCharDevice::new(invalid_resource);
        
        // Operations on invalid framebuffer should fail gracefully
        assert!(!char_device.can_read());
        assert!(!char_device.can_write());
        assert!(char_device.read_byte().is_none());
        assert!(char_device.write_byte(0x00).is_err());
        
        let mut buffer = [0u8; 10];
        assert_eq!(char_device.read(&mut buffer), 0);
        assert!(char_device.write(&[0x00, 0x01]).is_err());
    }
}