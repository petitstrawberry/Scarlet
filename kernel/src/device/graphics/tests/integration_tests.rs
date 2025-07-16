//! Integration test for GraphicsManager and FramebufferCharDevice
//! 
//! This test demonstrates the basic functionality of the GraphicsManager
//! and FramebufferCharDevice integration.

#[cfg(test)]
mod integration_tests {
    use alloc::{string::ToString, sync::Arc};
    use spin::RwLock;
    
    use crate::device::{
        graphics::{
            manager::{GraphicsManager, FramebufferResource},
            framebuffer_device::FramebufferCharDevice,
            GenericGraphicsDevice, FramebufferConfig, PixelFormat,
        },
        char::CharDevice,
        manager::DeviceManager,
        Device, DeviceType,
    };

    #[test_case]
    fn test_graphics_manager_basic_integration() {
        // Create a test GraphicsManager instance (separate from singleton)
        let mut graphics_manager = GraphicsManager::new();
        
        // Create a mock VirtIO GPU-like device
        let mut test_device = GenericGraphicsDevice::new("test-virtio-gpu");
        let config = FramebufferConfig::new(1024, 768, PixelFormat::BGRA8888);
        test_device.set_framebuffer_config(config.clone());
        
        // Allocate memory for framebuffer
        let fb_size = config.size();
        let fb_pages = (fb_size + 4095) / 4096;
        let fb_addr = crate::mem::page::allocate_raw_pages(fb_pages) as usize;
        test_device.set_framebuffer_address(fb_addr);
        
        let shared_device: Arc<dyn Device> = Arc::new(test_device);
        
        // Register the device with GraphicsManager
        let result = graphics_manager.register_framebuffer_from_device("gpu0", shared_device);
        assert!(result.is_ok(), "Failed to register framebuffer device");
        
        // Verify the framebuffer was registered
        assert_eq!(graphics_manager.get_framebuffer_count(), 1);
        let fb_names = graphics_manager.get_framebuffer_names();
        assert_eq!(fb_names.len(), 1);
        assert_eq!(fb_names[0], "fb0");
        
        // Get the framebuffer resource
        let fb_resource = graphics_manager.get_framebuffer("fb0").unwrap();
        assert_eq!(fb_resource.source_device_name, "gpu0");
        assert_eq!(fb_resource.logical_name, "fb0");
        assert_eq!(fb_resource.config.width, 1024);
        assert_eq!(fb_resource.config.height, 768);
        assert_eq!(fb_resource.config.format, PixelFormat::BGRA8888);
        assert_eq!(fb_resource.physical_addr, fb_addr);
        assert_eq!(fb_resource.size, 1024 * 768 * 4);
        
        crate::early_println!("[Test] GraphicsManager basic integration test passed");
    }

    #[test_case]
    fn test_framebuffer_char_device_integration() {
        // Setup clean graphics manager for this test  
        let graphics_manager = GraphicsManager::get_mut_manager();
        graphics_manager.clear_for_test();
        
        let mut test_device = GenericGraphicsDevice::new("test-gpu");
        let config = FramebufferConfig::new(640, 480, PixelFormat::RGBA8888);
        test_device.set_framebuffer_config(config.clone());
        
        let fb_size = config.size();
        let fb_pages = (fb_size + 4095) / 4096;
        let fb_addr = crate::mem::page::allocate_raw_pages(fb_pages) as usize;
        test_device.set_framebuffer_address(fb_addr);
        
        let shared_device: Arc<dyn Device> = Arc::new(test_device);
        graphics_manager.register_framebuffer_from_device("gpu0", shared_device).unwrap();
        
        // Get the framebuffer resource that was created
        let fb_resource = graphics_manager.get_framebuffer("fb0").expect("Framebuffer should exist");
        
        // Create FramebufferCharDevice
        let char_device = FramebufferCharDevice::new(fb_resource);
        
        // Test device properties
        assert_eq!(char_device.device_type(), DeviceType::Char);
        assert_eq!(char_device.name(), "framebuffer");
        assert_eq!(char_device.get_framebuffer_name(), "fb0");
        assert_eq!(char_device.get_position(), 0);
        
        // Test device capabilities
        assert!(char_device.can_read());
        assert!(char_device.can_write());
        
        // Test write operation
        let test_pattern = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF];
        let written = char_device.write(&test_pattern).unwrap();
        assert_eq!(written, test_pattern.len());
        assert_eq!(char_device.get_position(), test_pattern.len());
        
        // Test read operation
        char_device.reset_position();
        let mut read_buffer = [0u8; 6];
        let read_count = char_device.read(&mut read_buffer);
        assert_eq!(read_count, 6);
        assert_eq!(read_buffer, test_pattern);
        
        // Test byte operations
        char_device.reset_position();
        assert!(char_device.write_byte(0x12).is_ok());
        assert_eq!(char_device.get_position(), 1);
        
        char_device.reset_position();
        let byte = char_device.read_byte().unwrap();
        assert_eq!(byte, 0x12);
        
        crate::early_println!("[Test] FramebufferCharDevice integration test passed");
    }

    #[test_case]
    fn test_multiple_framebuffer_management() {
        // Setup clean graphics manager for this test
        let graphics_manager = GraphicsManager::get_mut_manager();
        graphics_manager.clear_for_test();
        
        // Create first framebuffer device
        let mut device1 = GenericGraphicsDevice::new("gpu-1");
        let config1 = FramebufferConfig::new(1920, 1080, PixelFormat::RGBA8888);
        device1.set_framebuffer_config(config1.clone());
        let fb_addr1 = crate::mem::page::allocate_raw_pages((config1.size() + 4095) / 4096) as usize;
        device1.set_framebuffer_address(fb_addr1);
        let shared_device1: Arc<dyn Device> = Arc::new(device1);
        
        // Create second framebuffer device
        let mut device2 = GenericGraphicsDevice::new("gpu-2");
        let config2 = FramebufferConfig::new(1024, 768, PixelFormat::BGRA8888);
        device2.set_framebuffer_config(config2.clone());
        let fb_addr2 = crate::mem::page::allocate_raw_pages((config2.size() + 4095) / 4096) as usize;
        device2.set_framebuffer_address(fb_addr2);
        let shared_device2: Arc<dyn Device> = Arc::new(device2);
        
        // Register both devices
        assert!(graphics_manager.register_framebuffer_from_device("gpu0", shared_device1).is_ok());
        assert!(graphics_manager.register_framebuffer_from_device("gpu1", shared_device2).is_ok());
        
        // Verify both framebuffers are registered
        assert_eq!(graphics_manager.get_framebuffer_count(), 2);
        let fb_names = graphics_manager.get_framebuffer_names();
        assert_eq!(fb_names.len(), 2);
        assert!(fb_names.contains(&"fb0".to_string()));
        assert!(fb_names.contains(&"fb1".to_string()));
        
        // Test both framebuffers
        let fb0 = graphics_manager.get_framebuffer("fb0").unwrap();
        let fb1 = graphics_manager.get_framebuffer("fb1").unwrap();
        
        assert_eq!(fb0.source_device_name, "gpu0");
        assert_eq!(fb1.source_device_name, "gpu1");
        assert_ne!(fb0.physical_addr, fb1.physical_addr);
        assert_ne!(fb0.size, fb1.size); // Different resolutions
        
        // Test character devices for both framebuffers
        let char_device0 = FramebufferCharDevice::new(fb0.clone());
        let char_device1 = FramebufferCharDevice::new(fb1.clone());
        
        // Write different patterns to each framebuffer
        let pattern0 = [0x10, 0x20, 0x30, 0x40];
        let pattern1 = [0x50, 0x60, 0x70, 0x80];
        
        assert!(char_device0.write(&pattern0).is_ok());
        assert!(char_device1.write(&pattern1).is_ok());
        
        // Read back and verify
        char_device0.reset_position();
        char_device1.reset_position();
        
        let mut read0 = [0u8; 4];
        let mut read1 = [0u8; 4];
        
        assert_eq!(char_device0.read(&mut read0), 4);
        assert_eq!(char_device1.read(&mut read1), 4);
        
        assert_eq!(read0, pattern0);
        assert_eq!(read1, pattern1);
        
        crate::early_println!("[Test] Multiple framebuffer management test passed");
    }

    #[test_case]
    fn test_char_device_id_assignment() {
        // Setup clean graphics manager for this test
        let graphics_manager = GraphicsManager::get_mut_manager();
        graphics_manager.clear_for_test();
        
        // Register a device
        let mut device = GenericGraphicsDevice::new("test-gpu");
        let config = FramebufferConfig::new(800, 600, PixelFormat::RGB888);
        device.set_framebuffer_config(config.clone());
        let fb_addr = crate::mem::page::allocate_raw_pages((config.size() + 4095) / 4096) as usize;
        device.set_framebuffer_address(fb_addr);
        let shared_device: Arc<dyn Device> = Arc::new(device);
        
        graphics_manager.register_framebuffer_from_device("gpu0", shared_device).unwrap();
        
        // Character device should be automatically created and registered
        let fb = graphics_manager.get_framebuffer("fb0").unwrap();
        assert!(fb.created_char_device_id.read().is_some(), "Character device should be automatically created");
        let _initial_device_id = fb.created_char_device_id.read().unwrap();
        
        // Test setting a different character device ID
        assert!(graphics_manager.set_char_device_id("fb0", 42).is_ok());
        
        // Verify the ID was updated
        let fb = graphics_manager.get_framebuffer("fb0").unwrap();
        assert_eq!(*fb.created_char_device_id.read(), Some(42));
        
        // Test error case
        assert!(graphics_manager.set_char_device_id("fb999", 123).is_err());
        
        crate::early_println!("[Test] Character device ID assignment test passed");
    }

    #[test_case]
    fn test_error_conditions() {
        // Test GraphicsManager with non-existent framebuffer
        let graphics_manager = GraphicsManager::new();
        assert!(graphics_manager.get_framebuffer("non_existent").is_none());
        assert_eq!(graphics_manager.get_framebuffer_count(), 0);
        assert_eq!(graphics_manager.get_framebuffer_names().len(), 0);
        
        // Test FramebufferCharDevice with invalid framebuffer
        let invalid_config = FramebufferConfig::new(10, 10, PixelFormat::RGB888);
        let invalid_resource = Arc::new(FramebufferResource {
            source_device_name: "none".to_string(),
            logical_name: "invalid".to_string(),
            config: invalid_config.clone(),
            physical_addr: 0, // Invalid address
            size: invalid_config.size(),
            created_char_device_id: RwLock::new(None),
        });
        
        let char_device = FramebufferCharDevice::new(invalid_resource);
        assert!(!char_device.can_read());
        assert!(!char_device.can_write());
        assert!(char_device.read_byte().is_none());
        assert!(char_device.write_byte(0x00).is_err());
        
        let mut buffer = [0u8; 10];
        assert_eq!(char_device.read(&mut buffer), 0);
        assert!(char_device.write(&[0x00, 0x01]).is_err());
        
        crate::early_println!("[Test] Error conditions test passed");
    }

    #[test_case]
    fn test_framebuffer_boundary_conditions() {
        // Setup clean graphics manager for this test
        let graphics_manager = GraphicsManager::get_mut_manager();
        graphics_manager.clear_for_test();
        
        // Create a very small framebuffer
        let mut device = GenericGraphicsDevice::new("small-gpu");
        let config = FramebufferConfig::new(2, 2, PixelFormat::RGB565); // 8 bytes total
        device.set_framebuffer_config(config.clone());
        let fb_addr = crate::mem::page::allocate_raw_pages(1) as usize; // One page
        device.set_framebuffer_address(fb_addr);
        let shared_device: Arc<dyn Device> = Arc::new(device);
        
        graphics_manager.register_framebuffer_from_device("small_gpu", shared_device).unwrap();
        
        // Get the framebuffer name that was assigned to this specific device
        let fb_names = graphics_manager.get_framebuffer_names();
        let fb_name = fb_names.iter()
            .find(|name| {
                if let Some(fb_resource) = graphics_manager.get_framebuffer(name) {
                    fb_resource.source_device_name == "small_gpu"
                } else {
                    false
                }
            })
            .expect("Should have framebuffer for this device")
            .clone();
        let fb_resource = graphics_manager.get_framebuffer(&fb_name).expect("Framebuffer should exist");
        let char_device = FramebufferCharDevice::new(fb_resource);
        
        // Fill the entire framebuffer
        let data = [0xFF; 10]; // More than framebuffer size
        let written = char_device.write(&data).unwrap();
        assert_eq!(written, 8); // Should only write 8 bytes (framebuffer size)
        assert_eq!(char_device.get_position(), 8);
        
        // Try to write more - should fail
        assert!(char_device.write_byte(0x00).is_err());
        assert!(!char_device.can_write());
        
        // Read back all data
        char_device.reset_position();
        let mut read_buffer = [0u8; 10];
        let read_count = char_device.read(&mut read_buffer);
        assert_eq!(read_count, 8);
        
        // Verify all bytes are 0xFF
        for i in 0..8 {
            assert_eq!(read_buffer[i], 0xFF);
        }
        
        // At end of framebuffer, can't read more
        assert!(char_device.read_byte().is_none());
        assert!(!char_device.can_read());
        
        crate::early_println!("[Test] Framebuffer boundary conditions test passed");
    }
}