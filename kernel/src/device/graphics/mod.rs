//! Graphics device interface
//! 
//! This module defines the interface for graphics devices in the kernel.
//! It provides abstractions for framebuffer operations and graphics device management.

use core::any::Any;
use alloc::{boxed::Box, vec::Vec};
use spin::Mutex;

use alloc::sync::Arc;

use super::{Device, DeviceType, manager::DeviceManager};

/// Get the first available graphics device
/// 
/// This is a convenience function to get the first graphics device registered in the system.
/// Returns None if no graphics devices are available.
pub fn get_graphics_device() -> Option<Arc<dyn Device>> {
    let manager = DeviceManager::get_manager();
    if let Some(device_id) = manager.get_first_device_by_type(DeviceType::Graphics) {
        return manager.get_device(device_id);
    }
    None
}

/// Pixel format for framebuffer
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    /// 32-bit RGBA (8 bits per channel)
    RGBA8888,
    /// 32-bit BGRA (8 bits per channel)  
    BGRA8888,
    /// 24-bit RGB (8 bits per channel)
    RGB888,
    /// 16-bit RGB (5-6-5 bits)
    RGB565,
}

impl PixelFormat {
    /// Get bytes per pixel for this format
    pub fn bytes_per_pixel(&self) -> usize {
        match self {
            PixelFormat::RGBA8888 | PixelFormat::BGRA8888 => 4,
            PixelFormat::RGB888 => 3,
            PixelFormat::RGB565 => 2,
        }
    }
}

/// Framebuffer configuration
#[derive(Debug, Clone)]
pub struct FramebufferConfig {
    /// Width in pixels
    pub width: u32,
    /// Height in pixels
    pub height: u32,
    /// Pixel format
    pub format: PixelFormat,
    /// Stride (bytes per row)
    pub stride: u32,
}

impl FramebufferConfig {
    /// Create a new framebuffer configuration
    pub fn new(width: u32, height: u32, format: PixelFormat) -> Self {
        let stride = width * format.bytes_per_pixel() as u32;
        Self { width, height, format, stride }
    }
    
    /// Get the total size of the framebuffer in bytes
    pub fn size(&self) -> usize {
        (self.stride * self.height) as usize
    }
}

/// Graphics operation requests
#[derive(Debug)]
pub enum GraphicsRequest {
    /// Get framebuffer configuration
    GetFramebufferConfig,
    /// Map framebuffer memory
    MapFramebuffer,
    /// Flush framebuffer changes to display
    FlushFramebuffer { x: u32, y: u32, width: u32, height: u32 },
}

/// Result of graphics operations
#[derive(Debug)]
pub struct GraphicsResult {
    pub request: Box<GraphicsRequest>,
    pub result: Result<GraphicsResponse, &'static str>,
}

/// Response from graphics operations
#[derive(Debug)]
pub enum GraphicsResponse {
    /// Framebuffer configuration
    FramebufferConfig(FramebufferConfig),
    /// Framebuffer memory address
    FramebufferAddress(usize),
    /// Operation completed successfully
    Success,
}

/// Graphics device interface
/// 
/// This trait defines the interface for graphics devices.
/// It provides methods for framebuffer management and display operations.
pub trait GraphicsDevice: Device {
    /// Get the device display name
    fn get_display_name(&self) -> &'static str;
    
    /// Get framebuffer configuration
    fn get_framebuffer_config(&self) -> Result<FramebufferConfig, &'static str>;
    
    /// Get framebuffer memory address
    fn get_framebuffer_address(&self) -> Result<usize, &'static str>;
    
    /// Flush framebuffer region to display
    fn flush_framebuffer(&self, x: u32, y: u32, width: u32, height: u32) -> Result<(), &'static str>;
    
    /// Initialize the graphics device
    fn init_graphics(&mut self) -> Result<(), &'static str>;
}

/// A generic implementation of a graphics device
pub struct GenericGraphicsDevice {
    display_name: &'static str,
    config: Option<FramebufferConfig>,
    framebuffer_addr: Option<usize>,
    request_queue: Mutex<Vec<Box<GraphicsRequest>>>,
}

impl GenericGraphicsDevice {
    pub fn new(display_name: &'static str) -> Self {
        Self {
            display_name,
            config: None,
            framebuffer_addr: None,
            request_queue: Mutex::new(Vec::new()),
        }
    }
    
    /// Set framebuffer configuration
    pub fn set_framebuffer_config(&mut self, config: FramebufferConfig) {
        self.config = Some(config);
    }
    
    /// Set framebuffer address
    pub fn set_framebuffer_address(&mut self, addr: usize) {
        self.framebuffer_addr = Some(addr);
    }
}

impl Device for GenericGraphicsDevice {
    fn device_type(&self) -> super::DeviceType {
        super::DeviceType::Graphics
    }

    fn name(&self) -> &'static str {
        self.display_name
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
    
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    
    fn as_graphics_device(&self) -> Option<&dyn GraphicsDevice> {
        Some(self)
    }
}

impl GraphicsDevice for GenericGraphicsDevice {
    fn get_display_name(&self) -> &'static str {
        self.display_name
    }
    
    fn get_framebuffer_config(&self) -> Result<FramebufferConfig, &'static str> {
        self.config.clone().ok_or("Framebuffer not configured")
    }
    
    fn get_framebuffer_address(&self) -> Result<usize, &'static str> {
        self.framebuffer_addr.ok_or("Framebuffer address not set")
    }
    
    fn flush_framebuffer(&self, _x: u32, _y: u32, _width: u32, _height: u32) -> Result<(), &'static str> {
        // Generic implementation - no-op
        Ok(())
    }
    
    fn init_graphics(&mut self) -> Result<(), &'static str> {
        // Generic implementation - no-op
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::DeviceType;

    #[test_case]
    fn test_pixel_format_bytes_per_pixel() {
        assert_eq!(PixelFormat::RGBA8888.bytes_per_pixel(), 4);
        assert_eq!(PixelFormat::BGRA8888.bytes_per_pixel(), 4);
        assert_eq!(PixelFormat::RGB888.bytes_per_pixel(), 3);
        assert_eq!(PixelFormat::RGB565.bytes_per_pixel(), 2);
    }

    #[test_case]
    fn test_framebuffer_config() {
        let config = FramebufferConfig::new(1920, 1080, PixelFormat::RGBA8888);
        assert_eq!(config.width, 1920);
        assert_eq!(config.height, 1080);
        assert_eq!(config.format, PixelFormat::RGBA8888);
        assert_eq!(config.stride, 1920 * 4);
        assert_eq!(config.size(), 1920 * 1080 * 4);
    }

    #[test_case]
    fn test_generic_graphics_device() {
        let mut device = GenericGraphicsDevice::new("test-display");
        assert_eq!(device.get_display_name(), "test-display");
        assert_eq!(device.device_type(), DeviceType::Graphics);
        
        // Test framebuffer configuration
        let config = FramebufferConfig::new(800, 600, PixelFormat::RGB888);
        device.set_framebuffer_config(config.clone());
        
        let retrieved_config = device.get_framebuffer_config().unwrap();
        assert_eq!(retrieved_config.width, config.width);
        assert_eq!(retrieved_config.height, config.height);
        assert_eq!(retrieved_config.format, config.format);
        
        // Test framebuffer address
        device.set_framebuffer_address(0x80000000);
        assert_eq!(device.get_framebuffer_address().unwrap(), 0x80000000);
        
        // Test flush operation
        assert!(device.flush_framebuffer(0, 0, 100, 100).is_ok());
    }

    #[test_case]
    fn test_get_graphics_device_none() {
        // Test when no graphics devices are registered
        // Note: This test assumes no graphics devices are registered in the test environment
        // In a real scenario with graphics devices, this would return Some(device)
        let result = get_graphics_device();
        // We can't assert the exact result since it depends on test environment state
        // But we can ensure the function doesn't panic and returns the correct type
        match result {
            Some(device) => {
                // If a device is found, it should be a graphics device
                assert_eq!(device.device_type(), DeviceType::Graphics);
            },
            None => {
                // No graphics device found - this is expected in test environment
            }
        }
    }

    #[test_case]
    fn test_framebuffer_drawing_operations() {
        let mut device = GenericGraphicsDevice::new("test-framebuffer");
        
        // Set up a test framebuffer configuration
        let config = FramebufferConfig::new(800, 600, PixelFormat::RGBA8888);
        device.set_framebuffer_config(config.clone());
        
        // Allocate test framebuffer memory
        let fb_size = config.size();
        let fb_pages = (fb_size + 4095) / 4096;
        let fb_addr = crate::mem::page::allocate_raw_pages(fb_pages) as usize;
        assert_ne!(fb_addr, 0);
        device.set_framebuffer_address(fb_addr);
        
        // Test basic framebuffer operations
        let retrieved_config = device.get_framebuffer_config().unwrap();
        assert_eq!(retrieved_config.width, 800);
        assert_eq!(retrieved_config.height, 600);
        assert_eq!(retrieved_config.format, PixelFormat::RGBA8888);
        
        let retrieved_addr = device.get_framebuffer_address().unwrap();
        assert_eq!(retrieved_addr, fb_addr);
        
        // Draw a test pattern
        unsafe {
            let fb_ptr = fb_addr as *mut u32;
            
            // Clear framebuffer to black
            for i in 0..(config.width * config.height) as usize {
                *fb_ptr.add(i) = 0xFF000000; // Black with full alpha
            }
            
            // Draw a white rectangle in the center
            let rect_width = 200;
            let rect_height = 150;
            let start_x = (config.width - rect_width) / 2;
            let start_y = (config.height - rect_height) / 2;
            
            for y in start_y..(start_y + rect_height) {
                for x in start_x..(start_x + rect_width) {
                    let pixel_index = (y * config.width + x) as usize;
                    *fb_ptr.add(pixel_index) = 0xFFFFFFFF; // White
                }
            }
            
            // Draw colored borders
            // Red top border
            for x in 0..config.width {
                let pixel_index = x as usize;
                *fb_ptr.add(pixel_index) = 0xFF0000FF; // Red
            }
            
            // Green bottom border  
            for x in 0..config.width {
                let pixel_index = ((config.height - 1) * config.width + x) as usize;
                *fb_ptr.add(pixel_index) = 0xFF00FF00; // Green
            }
            
            // Blue left border
            for y in 0..config.height {
                let pixel_index = (y * config.width) as usize;
                *fb_ptr.add(pixel_index) = 0xFFFF0000; // Blue
            }
            
            // Yellow right border
            for y in 0..config.height {
                let pixel_index = (y * config.width + (config.width - 1)) as usize;
                *fb_ptr.add(pixel_index) = 0xFF00FFFF; // Yellow
            }
        }
        
        // Flush the framebuffer
        assert!(device.flush_framebuffer(0, 0, config.width, config.height).is_ok());
        
        // Verify the pattern was drawn correctly
        unsafe {
            let fb_ptr = fb_addr as *mut u32;
            
            // Check borders
            assert_eq!(*fb_ptr, 0xFFFF0000); // Top-left should be blue (left border priority)
            assert_eq!(*fb_ptr.add((config.width - 1) as usize), 0xFF00FFFF); // Top-right should be yellow
            assert_eq!(*fb_ptr.add(((config.height - 1) * config.width) as usize), 0xFFFF0000); // Bottom-left should be blue
            assert_eq!(*fb_ptr.add(((config.height - 1) * config.width + (config.width - 1)) as usize), 0xFF00FFFF); // Bottom-right should be yellow
            
            // Check center rectangle
            let center_x = config.width / 2;
            let center_y = config.height / 2;
            let center_pixel = *fb_ptr.add((center_y * config.width + center_x) as usize);
            assert_eq!(center_pixel, 0xFFFFFFFF); // Should be white
            
            // Check black area (outside borders and rectangle)
            let test_x = center_x / 2;
            let test_y = center_y / 2;
            let test_pixel = *fb_ptr.add((test_y * config.width + test_x) as usize);
            assert_eq!(test_pixel, 0xFF000000); // Should be black
        }
    }

    #[test_case]
    fn test_pixel_format_operations() {
        let mut device = GenericGraphicsDevice::new("test-pixel-formats");
        
        // Test different pixel formats
        let formats = [
            PixelFormat::RGBA8888,
            PixelFormat::BGRA8888, 
            PixelFormat::RGB888,
            PixelFormat::RGB565,
        ];
        
        for format in formats {
            let config = FramebufferConfig::new(100, 100, format);
            device.set_framebuffer_config(config.clone());
            
            let fb_size = config.size();
            let expected_size = match format {
                PixelFormat::RGBA8888 | PixelFormat::BGRA8888 => 100 * 100 * 4,
                PixelFormat::RGB888 => 100 * 100 * 3,
                PixelFormat::RGB565 => 100 * 100 * 2,
            };
            assert_eq!(fb_size, expected_size);
            
            // Allocate and set framebuffer
            let fb_pages = (fb_size + 4095) / 4096;
            let fb_addr = crate::mem::page::allocate_raw_pages(fb_pages) as usize;
            device.set_framebuffer_address(fb_addr);
            
            // Test pixel writing based on format
            match format {
                PixelFormat::RGBA8888 => {
                    unsafe {
                        let fb_ptr = fb_addr as *mut u32;
                        *fb_ptr = 0xFF00FF80; // Semi-transparent red-green
                        assert_eq!(*fb_ptr, 0xFF00FF80);
                    }
                },
                PixelFormat::BGRA8888 => {
                    unsafe {
                        let fb_ptr = fb_addr as *mut u32;
                        *fb_ptr = 0x80FF0080; // Semi-transparent in BGRA
                        assert_eq!(*fb_ptr, 0x80FF0080);
                    }
                },
                PixelFormat::RGB888 => {
                    unsafe {
                        let fb_ptr = fb_addr as *mut u8;
                        *fb_ptr = 0xFF;         // R
                        *fb_ptr.add(1) = 0x80;  // G
                        *fb_ptr.add(2) = 0x40;  // B
                        assert_eq!(*fb_ptr, 0xFF);
                        assert_eq!(*fb_ptr.add(1), 0x80);
                        assert_eq!(*fb_ptr.add(2), 0x40);
                    }
                },
                PixelFormat::RGB565 => {
                    unsafe {
                        let fb_ptr = fb_addr as *mut u16;
                        *fb_ptr = 0xF800; // Red in RGB565 format
                        assert_eq!(*fb_ptr, 0xF800);
                    }
                },
            }
            
            // Test flushing
            assert!(device.flush_framebuffer(0, 0, 100, 100).is_ok());
        }
    }
}