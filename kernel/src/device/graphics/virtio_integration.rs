//! # GraphicsManager VirtIO GPU Integration Example
//!
//! This module demonstrates how the GraphicsManager would integrate with
//! the existing VirtIO GPU driver. This is not meant to be compiled in
//! the main kernel but serves as documentation for how the integration
//! would work in practice.

use alloc::{format, sync::Arc, string::String};
use crate::device::{
    graphics::{
        manager::GraphicsManager,
        framebuffer_device::FramebufferCharDevice,
    },
    manager::DeviceManager,
    char::CharDevice,
    Device,
};
use crate::drivers::graphics::virtio_gpu::VirtioGpuDevice;

/// Example function showing how to set up GraphicsManager with VirtIO GPU devices
/// This would typically be called during kernel initialization after device discovery.
pub fn initialize_graphics_subsystem() -> Result<(), &'static str> {
    crate::early_println!("[Graphics] Initializing graphics subsystem...");
    
    // Get the GraphicsManager singleton
    let graphics_manager = GraphicsManager::get_mut_manager();
    
    // Discover and register all graphics devices from DeviceManager
    graphics_manager.discover_graphics_devices();
    
    // Create character devices for each registered framebuffer
    let framebuffer_names = graphics_manager.get_framebuffer_names();
    let device_manager = DeviceManager::get_manager();
    
    for fb_name in framebuffer_names {
        // Create a FramebufferCharDevice for this framebuffer
        let char_device = FramebufferCharDevice::new(fb_name.clone());
        let shared_char_device: Arc<dyn Device> = Arc::new(char_device);
        
        // Register the character device with DeviceManager
        let device_name = format!("dev_{}", fb_name); // e.g., "dev_fb0"
        let char_device_id = device_manager.register_device_with_name(device_name, shared_char_device);
        
        // Update the GraphicsManager with the character device ID
        graphics_manager.set_char_device_id(&fb_name, char_device_id)?;
        
        crate::early_println!("[Graphics] Created character device {} for framebuffer {}", char_device_id, fb_name);
    }
    
    crate::early_println!("[Graphics] Graphics subsystem initialized with {} framebuffers", 
        graphics_manager.get_framebuffer_count());
    
    Ok(())
}

/// Example function showing how to manually register a VirtIO GPU device
/// This demonstrates the integration with an existing VirtIO GPU driver.
pub fn register_virtio_gpu_device(gpu_device: Arc<VirtioGpuDevice>, device_name: String) -> Result<(), &'static str> {
    crate::early_println!("[Graphics] Registering VirtIO GPU device: {}", device_name);
    
    // First register with DeviceManager
    let device_manager = DeviceManager::get_manager();
    let device_id = device_manager.register_device_with_name(device_name.clone(), gpu_device.clone());
    
    crate::early_println!("[Graphics] VirtIO GPU device registered with DeviceManager as ID {}", device_id);
    
    // Then register with GraphicsManager 
    let graphics_manager = GraphicsManager::get_mut_manager();
    graphics_manager.register_framebuffer_from_device(&device_name, gpu_device)?;
    
    crate::early_println!("[Graphics] VirtIO GPU device registered with GraphicsManager");
    
    Ok(())
}

/// Example function showing how to access framebuffer through character device
pub fn demonstrate_framebuffer_access() -> Result<(), &'static str> {
    let graphics_manager = GraphicsManager::get_manager();
    
    // Get the first available framebuffer
    let fb_names = graphics_manager.get_framebuffer_names();
    if fb_names.is_empty() {
        return Err("No framebuffers available");
    }
    
    let fb_name = &fb_names[0];
    let fb_resource = graphics_manager.get_framebuffer(fb_name).ok_or("Framebuffer not found")?;
    
    crate::early_println!("[Graphics] Using framebuffer: {} ({}x{} @ {:#x})", 
        fb_resource.logical_name,
        fb_resource.config.width,
        fb_resource.config.height,
        fb_resource.physical_addr);
    
    // Create character device for access
    let char_device = FramebufferCharDevice::new(fb_name.clone());
    
    // Example: Clear framebuffer to black
    let pixel_size = fb_resource.config.format.bytes_per_pixel();
    let total_pixels = (fb_resource.config.width * fb_resource.config.height) as usize;
    
    // Write black pixels one by one (just for demonstration)
    for _pixel in 0..total_pixels.min(100) { // Limit for demo purposes
        for _byte in 0..pixel_size {
            char_device.write_byte(0x00)?; // Black pixel
        }
    }
    
    crate::early_println!("[Graphics] Wrote {} pixels to framebuffer", total_pixels.min(100));
    
    // Example: Read back some data
    char_device.reset_position();
    let mut read_buffer = [0u8; 16];
    let bytes_read = char_device.read(&mut read_buffer);
    
    crate::early_println!("[Graphics] Read {} bytes from framebuffer", bytes_read);
    
    Ok(())
}

/// Example function showing how to get framebuffer information
pub fn list_framebuffer_information() {
    let graphics_manager = GraphicsManager::get_manager();
    let framebuffer_names = graphics_manager.get_framebuffer_names();
    
    crate::early_println!("[Graphics] Available framebuffers: {}", framebuffer_names.len());
    
    for fb_name in framebuffer_names {
        if let Some(fb_resource) = graphics_manager.get_framebuffer(&fb_name) {
            crate::early_println!("[Graphics]   {} (source: {})", fb_resource.logical_name, fb_resource.source_device_name);
            crate::early_println!("[Graphics]     Resolution: {}x{}", fb_resource.config.width, fb_resource.config.height);
            crate::early_println!("[Graphics]     Format: {:?}", fb_resource.config.format);
            crate::early_println!("[Graphics]     Physical Address: {:#x}", fb_resource.physical_addr);
            crate::early_println!("[Graphics]     Size: {} bytes", fb_resource.size);
            if let Some(char_device_id) = fb_resource.created_char_device_id {
                crate::early_println!("[Graphics]     Character Device ID: {}", char_device_id);
            }
        }
    }
}

/// Example of how the graphics subsystem would be initialized during kernel boot
pub fn kernel_graphics_init_example() {
    crate::early_println!("[Graphics] Kernel graphics initialization example");
    
    // Step 1: DeviceManager discovers VirtIO GPU devices (already happens)
    
    // Step 2: Initialize graphics subsystem
    if let Err(e) = initialize_graphics_subsystem() {
        crate::early_println!("[Graphics] Failed to initialize graphics subsystem: {}", e);
        return;
    }
    
    // Step 3: List available framebuffers
    list_framebuffer_information();
    
    // Step 4: Demonstrate framebuffer access
    if let Err(e) = demonstrate_framebuffer_access() {
        crate::early_println!("[Graphics] Failed to demonstrate framebuffer access: {}", e);
    }
    
    crate::early_println!("[Graphics] Graphics initialization example completed");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::graphics::{GenericGraphicsDevice, FramebufferConfig, PixelFormat};

    #[test_case]
    fn test_graphics_subsystem_integration() {
        // Create a mock environment
        let mut graphics_manager = GraphicsManager::new();
        
        // Simulate device registration
        let mut device = GenericGraphicsDevice::new("test-virtio-gpu");
        let config = FramebufferConfig::new(1024, 768, PixelFormat::BGRA8888);
        device.set_framebuffer_config(config);
        device.set_framebuffer_address(0x80000000);
        
        let shared_device: Arc<dyn Device> = Arc::new(device);
        
        // Register device
        let result = graphics_manager.register_framebuffer_from_device("gpu0", shared_device);
        assert!(result.is_ok());
        
        // Verify registration
        assert_eq!(graphics_manager.get_framebuffer_count(), 1);
        let fb_names = graphics_manager.get_framebuffer_names();
        assert_eq!(fb_names.len(), 1);
        assert_eq!(fb_names[0], "fb0");
        
        // Create character device
        let char_device = FramebufferCharDevice::new("fb0".into());
        
        // Test basic operations
        assert!(char_device.can_read());
        assert!(char_device.can_write());
        
        crate::early_println!("[Test] Graphics subsystem integration test passed");
    }
}