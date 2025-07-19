//! Integration tests for ControlOps functionality
//!
//! This module tests the complete integration of ControlOps through the
//! DevFS, DevFileObject, and FramebufferCharDevice layers.

use crate::{
    device::{
        graphics::{
            framebuffer_device::{FramebufferCharDevice, framebuffer_commands, FbVarScreenInfo, FbFixScreenInfo},
            manager::GraphicsManager,
            GenericGraphicsDevice, FramebufferConfig, PixelFormat
        },
        manager::DeviceManager,
        Device
    },
    object::capability::ControlOps
};
use alloc::{sync::Arc, string::ToString};

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_framebuffer_control_ops_basic() {
        // Setup clean graphics manager
        let graphics_manager = GraphicsManager::get_mut_manager();
        graphics_manager.clear_for_test();
        
        // Create a test graphics device
        let mut test_device = GenericGraphicsDevice::new("test-fb-basic-control");
        let config = FramebufferConfig::new(640, 480, PixelFormat::RGBA8888);
        test_device.set_framebuffer_config(config.clone());
        
        // Allocate memory for test framebuffer
        let fb_size = config.size();
        let fb_pages = (fb_size + 4095) / 4096;
        let fb_addr = crate::mem::page::allocate_raw_pages(fb_pages) as usize;
        test_device.set_framebuffer_address(fb_addr);
        
        let shared_device: Arc<dyn Device> = Arc::new(test_device);
        
        // Register device with DeviceManager
        let device_manager = DeviceManager::get_mut_manager();
        device_manager.clear_for_test();
        let device_id = device_manager.register_device_with_name("test-fb-basic-control".to_string(), shared_device.clone());
        
        // Register with GraphicsManager (this should create the character device)
        graphics_manager.register_framebuffer_from_device(device_id, shared_device).unwrap();
        
        // Get the framebuffer resource
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
        
        // Test that control operations work properly
        let commands = fb_device.supported_control_commands();
        assert!(!commands.is_empty(), "Should support control commands");
        
        // Test FBIO_FLUSH
        let result = fb_device.control(framebuffer_commands::FBIO_FLUSH, 0);
        assert!(result.is_ok(), "FBIO_FLUSH should succeed");
        assert_eq!(result.unwrap(), 0);
        
        // Test unsupported command
        let result = fb_device.control(0xFFFF, 0);
        assert!(result.is_err(), "Unsupported command should fail");
    }

    #[test_case] 
    fn test_control_ops_error_propagation() {
        // Test that control operations properly handle error cases
        
        // Setup clean graphics manager
        let graphics_manager = GraphicsManager::get_mut_manager();
        graphics_manager.clear_for_test();
        
        // Create a framebuffer device
        let mut test_device = GenericGraphicsDevice::new("test-fb-errors-control");
        let config = FramebufferConfig::new(100, 100, PixelFormat::RGB565);
        test_device.set_framebuffer_config(config.clone());
        
        let fb_size = config.size();
        let fb_pages = (fb_size + 4095) / 4096;
        let fb_addr = crate::mem::page::allocate_raw_pages(fb_pages) as usize;
        test_device.set_framebuffer_address(fb_addr);
        
        let shared_device: Arc<dyn Device> = Arc::new(test_device);
        
        // Register device
        let device_manager = DeviceManager::get_mut_manager();
        device_manager.clear_for_test();
        let device_id = device_manager.register_device_with_name("test-fb-errors-control".to_string(), shared_device.clone());
        graphics_manager.register_framebuffer_from_device(device_id, shared_device).unwrap();
        
        // Get the framebuffer resource
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
        
        // Test error cases
        
        // Test with null pointer for FBIOGET_VSCREENINFO
        let result = fb_device.control(framebuffer_commands::FBIOGET_VSCREENINFO, 0);
        assert!(result.is_err(), "Should fail with null pointer");
        assert_eq!(result.unwrap_err(), "Invalid argument pointer");
        
        // Test with null pointer for FBIOGET_FSCREENINFO
        let result = fb_device.control(framebuffer_commands::FBIOGET_FSCREENINFO, 0);
        assert!(result.is_err(), "Should fail with null pointer");
        assert_eq!(result.unwrap_err(), "Invalid argument pointer");
        
        // Test FBIOPUT_VSCREENINFO (not supported)
        let result = fb_device.control(framebuffer_commands::FBIOPUT_VSCREENINFO, 0);
        assert!(result.is_err(), "Should fail for unsupported operation");
        assert_eq!(result.unwrap_err(), "Setting screen information not supported");
    }
}