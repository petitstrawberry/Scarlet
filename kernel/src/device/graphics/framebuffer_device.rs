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
//! - Control operations (ioctl-equivalent) for device configuration
//! - Integration with GraphicsManager for resource management
//! - Standard character device interface for user programs
//! - Support for Linux-compatible framebuffer ioctls

extern crate alloc;

use core::{any::Any};
use alloc::{string::String, sync::Arc, vec::Vec, vec};
use spin::Mutex;

use crate::device::{
    char::CharDevice, graphics::manager::{FramebufferResource, GraphicsManager}, manager::DeviceManager, Device, DeviceType
};
use crate::object::capability::ControlOps;

/// Linux framebuffer ioctl command constants
/// These provide compatibility with Linux framebuffer applications
pub mod framebuffer_commands {
    /// Get variable screen information
    pub const FBIOGET_VSCREENINFO: u32 = 0x4600;
    /// Set variable screen information  
    pub const FBIOPUT_VSCREENINFO: u32 = 0x4601;
    /// Get fixed screen information
    pub const FBIOGET_FSCREENINFO: u32 = 0x4602;
    /// Flush framebuffer to display
    pub const FBIO_FLUSH: u32 = 0x4620;
}

/// Variable screen information structure (Linux fb_var_screeninfo compatible)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct FbVarScreenInfo {
    /// Visible resolution width
    pub xres: u32,
    /// Visible resolution height  
    pub yres: u32,
    /// Virtual resolution width
    pub xres_virtual: u32,
    /// Virtual resolution height
    pub yres_virtual: u32,
    /// Offset from virtual to visible resolution
    pub xoffset: u32,
    /// Offset from virtual to visible resolution
    pub yoffset: u32,
    /// Bits per pixel
    pub bits_per_pixel: u32,
    /// Grayscale != 0 means graylevels instead of colors
    pub grayscale: u32,
    /// Red bitfield
    pub red: FbBitfield,
    /// Green bitfield
    pub green: FbBitfield,
    /// Blue bitfield
    pub blue: FbBitfield,
    /// Transparency bitfield
    pub transp: FbBitfield,
    /// Non-zero if not grayscale
    pub nonstd: u32,
    /// Activate settings
    pub activate: u32,
    /// Screen height in mm
    pub height: u32,
    /// Screen width in mm
    pub width: u32,
    /// Acceleration flags
    pub accel_flags: u32,
    /// Pixel clock in picoseconds
    pub pixclock: u32,
    /// Time from sync to picture
    pub left_margin: u32,
    /// Time from picture to sync
    pub right_margin: u32,
    /// Time from sync to picture
    pub upper_margin: u32,
    /// Time from picture to sync
    pub lower_margin: u32,
    /// Length of horizontal sync
    pub hsync_len: u32,
    /// Length of vertical sync
    pub vsync_len: u32,
    /// Sync flags
    pub sync: u32,
    /// Video mode flags
    pub vmode: u32,
    /// Rotation angle (0=normal, 1=90°, 2=180°, 3=270°)
    pub rotate: u32,
    /// Color space for frame buffer
    pub colorspace: u32,
    /// Reserved for future use
    pub reserved: [u32; 4],
}

impl Default for FbVarScreenInfo {
    fn default() -> Self {
        Self {
            xres: 0,
            yres: 0,
            xres_virtual: 0,
            yres_virtual: 0,
            xoffset: 0,
            yoffset: 0,
            bits_per_pixel: 0,
            grayscale: 0,
            red: FbBitfield::default(),
            green: FbBitfield::default(),
            blue: FbBitfield::default(),
            transp: FbBitfield::default(),
            nonstd: 0,
            activate: 0,
            height: 0,
            width: 0,
            accel_flags: 0,
            pixclock: 0,
            left_margin: 0,
            right_margin: 0,
            upper_margin: 0,
            lower_margin: 0,
            hsync_len: 0,
            vsync_len: 0,
            sync: 0,
            vmode: 0,
            rotate: 0,
            colorspace: 0,
            reserved: [0; 4],
        }
    }
}

/// Fixed screen information structure (Linux fb_fix_screeninfo compatible)
#[repr(C)]
#[derive(Debug, Clone)]
pub struct FbFixScreenInfo {
    /// Identification string
    pub id: [u8; 16],
    /// Start of frame buffer memory (physical address)
    pub smem_start: usize,
    /// Length of frame buffer memory
    pub smem_len: u32,
    /// Framebuffer type
    pub type_: u32,
    /// Type of auxiliary display
    pub type_aux: u32,
    /// Visual type
    pub visual: u32,
    /// Zero if no hardware panning
    pub xpanstep: u16,
    /// Zero if no hardware panning
    pub ypanstep: u16,
    /// Zero if no hardware ywrap
    pub ywrapstep: u16,
    /// Length of a line in bytes
    pub line_length: u32,
    /// Start of memory mapped I/O
    pub mmio_start: usize,
    /// Length of memory mapped I/O
    pub mmio_len: u32,
    /// Acceleration type
    pub accel: u32,
    /// Capabilities
    pub capabilities: u16,
    /// Reserved for future compatibility
    pub reserved: [u16; 2],
}

impl Default for FbFixScreenInfo {
    fn default() -> Self {
        Self {
            id: [0; 16],
            smem_start: 0,
            smem_len: 0,
            type_: 0,
            type_aux: 0,
            visual: 0,
            xpanstep: 0,
            ypanstep: 0,
            ywrapstep: 0,
            line_length: 0,
            mmio_start: 0,
            mmio_len: 0,
            accel: 0,
            capabilities: 0,
            reserved: [0; 2],
        }
    }
}

/// Bitfield information for color components
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct FbBitfield {
    /// Beginning of bitfield (LSB is 0)
    pub offset: u32,
    /// Length of bitfield
    pub length: u32,
    /// MSB position (0 = MSB is rightmost)
    pub msb_right: u32,
}

impl Default for FbBitfield {
    fn default() -> Self {
        Self {
            offset: 0,
            length: 0,
            msb_right: 0,
        }
    }
}

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

        Ok(bytes_to_write)
    }
}

impl ControlOps for FramebufferCharDevice {
    fn control(&self, command: u32, arg: usize) -> Result<i32, &'static str> {
        use framebuffer_commands::*;
        
        match command {
            FBIOGET_VSCREENINFO => {
                self.handle_get_vscreeninfo(arg)
            }
            FBIOGET_FSCREENINFO => {
                self.handle_get_fscreeninfo(arg)
            }
            FBIO_FLUSH => {
                self.handle_flush(arg)
            }
            FBIOPUT_VSCREENINFO => {
                self.handle_put_vscreeninfo(arg)
            }
            _ => {
                Err("Unsupported framebuffer control command")
            }
        }
    }
    
    fn supported_control_commands(&self) -> Vec<(u32, &'static str)> {
        use framebuffer_commands::*;
        vec![
            (FBIOGET_VSCREENINFO, "Get variable screen information"),
            (FBIOGET_FSCREENINFO, "Get fixed screen information"),
            (FBIO_FLUSH, "Flush framebuffer to display"),
            (FBIOPUT_VSCREENINFO, "Set variable screen information"),
        ]
    }
}

impl FramebufferCharDevice {
    /// Handle FBIOGET_VSCREENINFO control command
    fn handle_get_vscreeninfo(&self, arg: usize) -> Result<i32, &'static str> {
        if arg == 0 {
            return Err("Invalid argument pointer");
        }
        
        let fb_resource = &self.fb_resource;
        let config = &fb_resource.config;
        
        // Create variable screen info structure
        let mut var_info = FbVarScreenInfo::default();
        var_info.xres = config.width;
        var_info.yres = config.height;
        var_info.xres_virtual = config.width;
        var_info.yres_virtual = config.height;
        var_info.bits_per_pixel = (config.format.bytes_per_pixel() * 8) as u32;
        
        // Set color bitfields based on format
        match config.format {
            super::PixelFormat::RGBA8888 => {
                var_info.red = FbBitfield { offset: 0, length: 8, msb_right: 0 };
                var_info.green = FbBitfield { offset: 8, length: 8, msb_right: 0 };
                var_info.blue = FbBitfield { offset: 16, length: 8, msb_right: 0 };
                var_info.transp = FbBitfield { offset: 24, length: 8, msb_right: 0 };
            }
            super::PixelFormat::BGRA8888 => {
                var_info.blue = FbBitfield { offset: 0, length: 8, msb_right: 0 };
                var_info.green = FbBitfield { offset: 8, length: 8, msb_right: 0 };
                var_info.red = FbBitfield { offset: 16, length: 8, msb_right: 0 };
                var_info.transp = FbBitfield { offset: 24, length: 8, msb_right: 0 };
            }
            super::PixelFormat::RGB888 => {
                var_info.red = FbBitfield { offset: 0, length: 8, msb_right: 0 };
                var_info.green = FbBitfield { offset: 8, length: 8, msb_right: 0 };
                var_info.blue = FbBitfield { offset: 16, length: 8, msb_right: 0 };
                var_info.transp = FbBitfield { offset: 0, length: 0, msb_right: 0 };
            }
            super::PixelFormat::RGB565 => {
                var_info.red = FbBitfield { offset: 11, length: 5, msb_right: 0 };
                var_info.green = FbBitfield { offset: 5, length: 6, msb_right: 0 };
                var_info.blue = FbBitfield { offset: 0, length: 5, msb_right: 0 };
                var_info.transp = FbBitfield { offset: 0, length: 0, msb_right: 0 };
            }
        }
        
        // Copy to user space (unsafe - in real implementation would need proper memory validation)
        unsafe {
            let user_ptr = arg as *mut FbVarScreenInfo;
            *user_ptr = var_info;
        }
        
        Ok(0) // Success
    }
    
    /// Handle FBIOGET_FSCREENINFO control command
    fn handle_get_fscreeninfo(&self, arg: usize) -> Result<i32, &'static str> {
        if arg == 0 {
            return Err("Invalid argument pointer");
        }
        
        let fb_resource = &self.fb_resource;
        let config = &fb_resource.config;
        
        // Create fixed screen info structure
        let mut fix_info = FbFixScreenInfo::default();
        
        // Set identification string
        let fb_name = fb_resource.logical_name.as_bytes();
        let copy_len = fb_name.len().min(fix_info.id.len() - 1);
        fix_info.id[..copy_len].copy_from_slice(&fb_name[..copy_len]);
        
        fix_info.smem_start = fb_resource.physical_addr;
        fix_info.smem_len = fb_resource.size as u32;
        fix_info.line_length = config.stride;
        fix_info.type_ = 0; // FB_TYPE_PACKED_PIXELS
        fix_info.visual = 2; // FB_VISUAL_TRUECOLOR
        
        // Copy to user space (unsafe - in real implementation would need proper memory validation)
        unsafe {
            let user_ptr = arg as *mut FbFixScreenInfo;
            *user_ptr = fix_info;
        }
        
        Ok(0) // Success
    }
    
    /// Handle FBIO_FLUSH control command
    fn handle_flush(&self, _arg: usize) -> Result<i32, &'static str> {
        // For a simple framebuffer, flush is typically a no-op
        // In a real implementation, this might trigger a display update
        Ok(0) // Success
    }
    
    /// Handle FBIOPUT_VSCREENINFO control command  
    fn handle_put_vscreeninfo(&self, _arg: usize) -> Result<i32, &'static str> {
        // Setting screen info is not supported in this basic implementation
        // In a real implementation, this would validate and apply new settings
        Err("Setting screen information not supported")
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
        
        // Register device with DeviceManager first
        let device_manager = DeviceManager::get_manager();
        let device_id = device_manager.register_device_with_name("test-gpu-read-write".to_string(), shared_device.clone());
        
        // Then register with GraphicsManager
        graphics_manager.register_framebuffer_from_device(device_id, shared_device).unwrap();

        // Get the framebuffer resource that was assigned to this specific device
        let fb_resource = {
            let fb_names = graphics_manager.get_framebuffer_names();
            let fb_name = fb_names.iter()
                .find(|name| {
                    if let Some(fb_resource) = graphics_manager.get_framebuffer(name) {
                        fb_resource.source_device_id == device_id
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
        
        // Register device with DeviceManager first
        let device_manager = DeviceManager::get_manager();
        let device_id = device_manager.register_device_with_name("test-gpu-byte-ops".to_string(), shared_device.clone());
        
        // Then register with GraphicsManager
        graphics_manager.register_framebuffer_from_device(device_id, shared_device).unwrap();

        // Get the framebuffer resource that was assigned to this specific device
        let fb_resource = {
            let fb_names = graphics_manager.get_framebuffer_names();
            let fb_name = fb_names.iter()
                .find(|name| {
                    if let Some(fb_resource) = graphics_manager.get_framebuffer(name) {
                        fb_resource.source_device_id == device_id
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
        
        // Register device with DeviceManager first
        let device_manager = DeviceManager::get_manager();
        let device_id = device_manager.register_device_with_name("test-gpu-can-rw".to_string(), shared_device.clone());
        
        // Then register with GraphicsManager
        graphics_manager.register_framebuffer_from_device(device_id, shared_device).unwrap();

        // Get the framebuffer resource that was assigned to this specific device
        let fb_resource = {
            let fb_names = graphics_manager.get_framebuffer_names();
            let fb_name = fb_names.iter()
                .find(|name| {
                    if let Some(fb_resource) = graphics_manager.get_framebuffer(name) {
                        fb_resource.source_device_id == device_id
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
        
        // Register device with DeviceManager first
        let device_manager = DeviceManager::get_manager();
        let device_id = device_manager.register_device_with_name("test-gpu-boundary".to_string(), shared_device.clone());
        
        // Then register with GraphicsManager
        graphics_manager.register_framebuffer_from_device(device_id, shared_device).unwrap();

        // Get the framebuffer resource that was assigned to this specific device
        let fb_resource = {
            let fb_names = graphics_manager.get_framebuffer_names();
            let fb_name = fb_names.iter()
                .find(|name| {
                    if let Some(fb_resource) = graphics_manager.get_framebuffer(name) {
                        fb_resource.source_device_id == device_id
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

    #[test_case]
    fn test_framebuffer_char_device_control_operations() {
        // Setup clean graphics manager for this test
        let graphics_manager = setup_clean_graphics_manager();
        let mut test_device = GenericGraphicsDevice::new("test-gpu-control");
        let config = FramebufferConfig::new(640, 480, PixelFormat::RGBA8888);
        test_device.set_framebuffer_config(config.clone());
        
        // Allocate memory for test framebuffer
        let fb_size = config.size();
        let fb_pages = (fb_size + 4095) / 4096;
        let fb_addr = crate::mem::page::allocate_raw_pages(fb_pages) as usize;
        test_device.set_framebuffer_address(fb_addr);
        
        let shared_device: Arc<dyn Device> = Arc::new(test_device);
        
        // Register device with DeviceManager first
        let device_manager = DeviceManager::get_manager();
        let device_id = device_manager.register_device_with_name("test-gpu-control".to_string(), shared_device.clone());
        
        // Then register with GraphicsManager
        graphics_manager.register_framebuffer_from_device(device_id, shared_device).unwrap();

        // Get the framebuffer resource that was assigned to this specific device
        let fb_resource = {
            let fb_names = graphics_manager.get_framebuffer_names();
            let fb_name = fb_names.iter()
                .find(|name| {
                    if let Some(fb_resource) = graphics_manager.get_framebuffer(name) {
                        fb_resource.source_device_id == device_id
                    } else {
                        false
                    }
                })
                .expect("Should have framebuffer for this device");
            graphics_manager.get_framebuffer(fb_name).expect("Framebuffer should exist")
        };
        let fb_device = FramebufferCharDevice::new(fb_resource);
        
        // Test supported control commands
        let commands = fb_device.supported_control_commands();
        assert!(!commands.is_empty());
        assert!(commands.iter().any(|(cmd, _)| *cmd == framebuffer_commands::FBIOGET_VSCREENINFO));
        assert!(commands.iter().any(|(cmd, _)| *cmd == framebuffer_commands::FBIOGET_FSCREENINFO));
        assert!(commands.iter().any(|(cmd, _)| *cmd == framebuffer_commands::FBIO_FLUSH));
        
        // Test FBIO_FLUSH (should succeed with no operation)
        let result = fb_device.control(framebuffer_commands::FBIO_FLUSH, 0);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0);
        
        // Test unsupported command
        let result = fb_device.control(0xFFFF, 0);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Unsupported framebuffer control command");
    }

    #[test_case]
    fn test_framebuffer_char_device_vscreeninfo() {
        // Setup clean graphics manager for this test
        let graphics_manager = setup_clean_graphics_manager();
        let mut test_device = GenericGraphicsDevice::new("test-gpu-vscreen");
        let config = FramebufferConfig::new(1024, 768, PixelFormat::RGB888);
        test_device.set_framebuffer_config(config.clone());
        
        // Allocate memory for test framebuffer
        let fb_size = config.size();
        let fb_pages = (fb_size + 4095) / 4096;
        let fb_addr = crate::mem::page::allocate_raw_pages(fb_pages) as usize;
        test_device.set_framebuffer_address(fb_addr);
        
        let shared_device: Arc<dyn Device> = Arc::new(test_device);
        
        // Register device with DeviceManager first
        let device_manager = DeviceManager::get_manager();
        let device_id = device_manager.register_device_with_name("test-gpu-vscreen".to_string(), shared_device.clone());
        
        // Then register with GraphicsManager
        graphics_manager.register_framebuffer_from_device(device_id, shared_device).unwrap();

        // Get the framebuffer resource that was assigned to this specific device
        let fb_resource = {
            let fb_names = graphics_manager.get_framebuffer_names();
            let fb_name = fb_names.iter()
                .find(|name| {
                    if let Some(fb_resource) = graphics_manager.get_framebuffer(name) {
                        fb_resource.source_device_id == device_id
                    } else {
                        false
                    }
                })
                .expect("Should have framebuffer for this device");
            graphics_manager.get_framebuffer(fb_name).expect("Framebuffer should exist")
        };
        let fb_device = FramebufferCharDevice::new(fb_resource);
        
        // Allocate space for variable screen info
        let mut var_info = FbVarScreenInfo::default();
        let info_ptr = &mut var_info as *mut FbVarScreenInfo;
        
        // Test FBIOGET_VSCREENINFO
        let result = fb_device.control(framebuffer_commands::FBIOGET_VSCREENINFO, info_ptr as usize);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0);
        
        // Verify the information was filled correctly
        assert_eq!(var_info.xres, 1024);
        assert_eq!(var_info.yres, 768);
        assert_eq!(var_info.bits_per_pixel, 24); // RGB888 = 24 bits per pixel
        
        // Verify RGB bitfields for RGB888
        assert_eq!(var_info.red.offset, 0);
        assert_eq!(var_info.red.length, 8);
        assert_eq!(var_info.green.offset, 8);
        assert_eq!(var_info.green.length, 8);
        assert_eq!(var_info.blue.offset, 16);
        assert_eq!(var_info.blue.length, 8);
    }

    #[test_case]
    fn test_framebuffer_char_device_fscreeninfo() {
        // Setup clean graphics manager for this test
        let graphics_manager = setup_clean_graphics_manager();
        let mut test_device = GenericGraphicsDevice::new("test-gpu-fscreen");
        let config = FramebufferConfig::new(800, 600, PixelFormat::BGRA8888);
        test_device.set_framebuffer_config(config.clone());
        
        // Allocate memory for test framebuffer
        let fb_size = config.size();
        let fb_pages = (fb_size + 4095) / 4096;
        let fb_addr = crate::mem::page::allocate_raw_pages(fb_pages) as usize;
        test_device.set_framebuffer_address(fb_addr);
        
        let shared_device: Arc<dyn Device> = Arc::new(test_device);
        
        // Register device with DeviceManager first
        let device_manager = DeviceManager::get_manager();
        let device_id = device_manager.register_device_with_name("test-gpu-fscreen".to_string(), shared_device.clone());
        
        // Then register with GraphicsManager
        graphics_manager.register_framebuffer_from_device(device_id, shared_device).unwrap();

        // Get the framebuffer resource that was assigned to this specific device
        let fb_resource = {
            let fb_names = graphics_manager.get_framebuffer_names();
            let fb_name = fb_names.iter()
                .find(|name| {
                    if let Some(fb_resource) = graphics_manager.get_framebuffer(name) {
                        fb_resource.source_device_id == device_id
                    } else {
                        false
                    }
                })
                .expect("Should have framebuffer for this device");
            graphics_manager.get_framebuffer(fb_name).expect("Framebuffer should exist")
        };
        let fb_device = FramebufferCharDevice::new(fb_resource);
        
        // Allocate space for fixed screen info
        let mut fix_info = FbFixScreenInfo::default();
        let info_ptr = &mut fix_info as *mut FbFixScreenInfo;
        
        // Test FBIOGET_FSCREENINFO
        let result = fb_device.control(framebuffer_commands::FBIOGET_FSCREENINFO, info_ptr as usize);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0);
        
        // Verify the information was filled correctly
        assert_eq!(fix_info.smem_start, fb_addr);
        assert_eq!(fix_info.smem_len, fb_size as u32);
        assert_eq!(fix_info.line_length, config.stride);
        assert_eq!(fix_info.type_, 0); // FB_TYPE_PACKED_PIXELS
        assert_eq!(fix_info.visual, 2); // FB_VISUAL_TRUECOLOR
        
        // Verify the ID string contains the framebuffer logical name
        let id_str = core::str::from_utf8(&fix_info.id[..7]).unwrap_or("");
        assert!(id_str.starts_with("fb"));
    }
}