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
        let result = graphics_manager.register_framebuffer_from_device(0, shared_device);
        assert!(result.is_ok(), "Failed to register framebuffer device");
        
        // Verify the framebuffer was registered
        assert_eq!(graphics_manager.get_framebuffer_count(), 1);
        let fb_names = graphics_manager.get_framebuffer_names();
        assert_eq!(fb_names.len(), 1);
        assert_eq!(fb_names[0], "fb0");
        
        // Get the framebuffer resource
        let fb_resource = graphics_manager.get_framebuffer("fb0").unwrap();
        assert_eq!(fb_resource.source_device_id, 0);
        assert_eq!(fb_resource.logical_name, "fb0");
        assert_eq!(fb_resource.config.width, 1024);
        assert_eq!(fb_resource.config.height, 768);
        assert_eq!(fb_resource.config.format, PixelFormat::BGRA8888);
        assert_eq!(fb_resource.physical_addr, fb_addr);
        assert_eq!(fb_resource.size, 1024 * 768 * 4);
        
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
        
        // Register device with DeviceManager first (this is what happens in real kernel)
        let device_manager = DeviceManager::get_manager();
        let device_id = device_manager.register_device_with_name("test-gpu".to_string(), shared_device.clone());
        
        // Then register framebuffer with GraphicsManager
        graphics_manager.register_framebuffer_from_device(device_id, shared_device).unwrap();
        
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
        
        // Register devices with DeviceManager first
        let device_manager = DeviceManager::get_manager();
        let device_id1 = device_manager.register_device_with_name("gpu-1".to_string(), shared_device1.clone());
        let device_id2 = device_manager.register_device_with_name("gpu-2".to_string(), shared_device2.clone());
        
        // Register both devices with GraphicsManager
        assert!(graphics_manager.register_framebuffer_from_device(device_id1, shared_device1).is_ok());
        assert!(graphics_manager.register_framebuffer_from_device(device_id2, shared_device2).is_ok());

        // Verify both framebuffers are registered
        assert_eq!(graphics_manager.get_framebuffer_count(), 2);
        let fb_names = graphics_manager.get_framebuffer_names();
        assert_eq!(fb_names.len(), 2);
        assert!(fb_names.contains(&"fb0".to_string()));
        assert!(fb_names.contains(&"fb1".to_string()));
        
        // Test both framebuffers
        let fb0 = graphics_manager.get_framebuffer("fb0").unwrap();
        let fb1 = graphics_manager.get_framebuffer("fb1").unwrap();
        
        assert_eq!(fb0.source_device_id, device_id1);
        assert_eq!(fb1.source_device_id, device_id2);
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
        
        // Register device with DeviceManager first
        let device_manager = DeviceManager::get_manager();
        let device_id = device_manager.register_device_with_name("test-gpu".to_string(), shared_device.clone());
        
        // Then register with GraphicsManager
        graphics_manager.register_framebuffer_from_device(device_id, shared_device).unwrap();
        
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
            source_device_id: 0,
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

        // Register device with DeviceManager first
        let device_manager = DeviceManager::get_manager();
        let device_id = device_manager.register_device_with_name("small-gpu".to_string(), shared_device.clone());
        
        // Then register with GraphicsManager
        graphics_manager.register_framebuffer_from_device(device_id, shared_device).unwrap();

        // Get the framebuffer name that was assigned to this specific device
        let fb_names = graphics_manager.get_framebuffer_names();
        let fb_name = fb_names.iter()
            .find(|name| {
                if let Some(fb_resource) = graphics_manager.get_framebuffer(name) {
                    fb_resource.source_device_id == device_id
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
        
    }

    #[test_case]
    fn test_devfs_framebuffer_write() {
        use crate::fs::vfs_v2::drivers::devfs::{DevFS, DevFileObject};
        use crate::fs::{FileType, DeviceFileInfo};
        use crate::object::capability::StreamOps;
        
        // Setup clean graphics manager for this test
        let graphics_manager = GraphicsManager::get_mut_manager();
        graphics_manager.clear_for_test();
        
        // Create a test framebuffer device
        let mut device = GenericGraphicsDevice::new("devfs-test-gpu");
        let config = FramebufferConfig::new(100, 100, PixelFormat::RGBA8888);
        device.set_framebuffer_config(config.clone());
        let fb_addr = crate::mem::page::allocate_raw_pages((config.size() + 4095) / 4096) as usize;
        device.set_framebuffer_address(fb_addr);
        let shared_device: Arc<dyn Device> = Arc::new(device);
        
        // Register device with DeviceManager first
        let device_manager = DeviceManager::get_manager();
        let device_id = device_manager.register_device_with_name("devfs-test-gpu".to_string(), shared_device.clone());
        
        // Register with GraphicsManager
        graphics_manager.register_framebuffer_from_device(device_id, shared_device).unwrap();

        // Get the framebuffer and its character device ID
        let fb_resource = graphics_manager.get_framebuffer("fb0").unwrap();
        let char_device_id = fb_resource.created_char_device_id.read().unwrap();
        
        // Create DevFS filesystem
        let _devfs = DevFS::new();
        
        // Create a DevFileObject directly for the framebuffer device
        let device_file_info = DeviceFileInfo {
            device_id: char_device_id,
            device_type: DeviceType::Char,
        };
        let file_type = FileType::CharDevice(device_file_info);
        
        // Create a mock DevNode for the test
        use crate::fs::vfs_v2::drivers::devfs::DevNode;
        let dev_node = Arc::new(DevNode::new_device_file(
            "fb0".to_string(),
            file_type,
            char_device_id as u64,
        ));
        
        // Create the DevFileObject
        let dev_file_object = DevFileObject::new(dev_node, char_device_id, DeviceType::Char).unwrap();
        
        // Test writing through DevFS
        let test_pattern = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF];
        let bytes_written = dev_file_object.write(&test_pattern).unwrap();
        assert_eq!(bytes_written, test_pattern.len());
        
        // Read back through DevFS to verify
        let mut read_buffer = [0u8; 6];
        
        // Note: For DevFS, we need to use the underlying character device for reading
        // since DevFileObject uses read_byte which advances the device position
        let char_device = FramebufferCharDevice::new(fb_resource.clone());
        char_device.reset_position(); // Reset to beginning
        let bytes_read = char_device.read(&mut read_buffer);
        assert_eq!(bytes_read, 6);
        assert_eq!(read_buffer, test_pattern);
        
        // Test writing more data through DevFS
        let second_pattern = [0x11, 0x22, 0x33, 0x44];
        char_device.set_position(10); // Set position to 10
        
        // Write through DevFS again - note that DevFS uses write_byte which doesn't respect position
        // So we write additional data that will be appended
        let bytes_written2 = dev_file_object.write(&second_pattern).unwrap();
        assert_eq!(bytes_written2, second_pattern.len());
        
        // Verify the second write by reading from position 6 onwards
        char_device.set_position(6);
        let mut read_buffer2 = [0u8; 4];
        let bytes_read2 = char_device.read(&mut read_buffer2);
        assert_eq!(bytes_read2, 4);
        assert_eq!(read_buffer2, second_pattern);
        
    }

    #[test_case]
    fn test_devfs_integration_with_device_manager() {
        // Setup clean managers for this test
        let graphics_manager = GraphicsManager::get_mut_manager();
        graphics_manager.clear_for_test();
        
        let device_manager = DeviceManager::get_manager();
        
        // Create multiple framebuffer devices
        let device_names = ["devfs-test-gpu-0", "devfs-test-gpu-1", "devfs-test-gpu-2"];
        for (i, device_name) in device_names.iter().enumerate() {
            let mut device = GenericGraphicsDevice::new(device_name);
            let config = FramebufferConfig::new((64 + i * 32) as u32, (64 + i * 32) as u32, PixelFormat::RGB888);
            device.set_framebuffer_config(config.clone());
            let fb_addr = crate::mem::page::allocate_raw_pages((config.size() + 4095) / 4096) as usize;
            device.set_framebuffer_address(fb_addr);
            let shared_device: Arc<dyn Device> = Arc::new(device);
            
            // Register device with DeviceManager first
            let device_id = device_manager.register_device_with_name(device_name.to_string(), shared_device.clone());
            
            // Then register with GraphicsManager
            graphics_manager.register_framebuffer_from_device(
                device_id,
                shared_device
            ).unwrap();
        }
        
        // Verify all devices are registered
        assert_eq!(graphics_manager.get_framebuffer_count(), 3);
        let fb_names = graphics_manager.get_framebuffer_names();
        assert_eq!(fb_names.len(), 3);
        
        // Test DevFS can access all framebuffer devices
        use crate::fs::vfs_v2::drivers::devfs::{DevFS, DevFileObject};
        use crate::fs::{FileType, DeviceFileInfo};
        use crate::object::capability::StreamOps;
        
        let _devfs = DevFS::new();
        
        for (idx, fb_name) in fb_names.iter().enumerate() {
            let fb_resource = graphics_manager.get_framebuffer(fb_name).unwrap();
            let char_device_id = fb_resource.created_char_device_id.read().unwrap();
            
            // Create DevFileObject for this framebuffer
            let device_file_info = DeviceFileInfo {
                device_id: char_device_id,
                device_type: DeviceType::Char,
            };
            let file_type = FileType::CharDevice(device_file_info);
            
            use crate::fs::vfs_v2::drivers::devfs::DevNode;
            let dev_node = Arc::new(DevNode::new_device_file(
                fb_name.clone(),
                file_type,
                char_device_id as u64,
            ));
            
            let dev_file_object = DevFileObject::new(dev_node, char_device_id, DeviceType::Char).unwrap();
            
            // Write unique pattern to each framebuffer
            let pattern = [0x10 + idx as u8, 0x20 + idx as u8, 0x30 + idx as u8];
            let bytes_written = dev_file_object.write(&pattern).unwrap();
            assert_eq!(bytes_written, pattern.len());
            
            // Verify the write using direct character device access
            let char_device = FramebufferCharDevice::new(fb_resource.clone());
            char_device.reset_position();
            let mut read_buffer = [0u8; 3];
            let bytes_read = char_device.read(&mut read_buffer);
            assert_eq!(bytes_read, 3);
            assert_eq!(read_buffer, pattern);
        }
        
    }

    #[test_case]
    fn test_dev_fb0_gradient_drawing() {
        // Setup clean graphics manager for this test
        let graphics_manager = GraphicsManager::get_mut_manager();
        graphics_manager.clear_for_test();
        
        // Create a VirtIO GPU device suitable for gradient drawing
        use crate::drivers::graphics::virtio_gpu::VirtioGpuDevice;
        use crate::device::graphics::GraphicsDevice;
        
        // Use a mock VirtIO GPU base address for testing
        let virtio_gpu_base_addr = 0x10002000; // Typical VirtIO GPU address
        let mut device = VirtioGpuDevice::new(virtio_gpu_base_addr);

        
        // Try to initialize the VirtIO GPU graphics capabilities
        device.init_graphics().expect("Failed to initialize VirtIO GPU device");
        // Get framebuffer configuration
        let config = device.get_framebuffer_config().unwrap();
        assert_eq!(config.width, 1024);
        assert_eq!(config.height, 768);
        assert_eq!(config.format, PixelFormat::BGRA8888);
        let shared_device = Arc::new(device);

        

        // Register device with DeviceManager first  
        let device_manager = DeviceManager::get_manager();
        let device_id = device_manager.register_device_with_name("gradient-gpu".to_string(), shared_device.clone());
        
        // Then register with GraphicsManager
        graphics_manager.register_framebuffer_from_device(device_id, shared_device).unwrap();
        
        // Get the framebuffer resource
        let fb_resource = graphics_manager.get_framebuffer("fb0").expect("Framebuffer should exist");
        
        // Create FramebufferCharDevice representing /dev/fb0
        let fb_char_device = FramebufferCharDevice::new(fb_resource.clone());
        
        // Verify device properties
        assert_eq!(fb_char_device.device_type(), DeviceType::Char);
        assert_eq!(fb_char_device.name(), "framebuffer");
        assert_eq!(fb_char_device.get_framebuffer_name(), "fb0");
        
        // Draw a gradient pattern similar to test_virtio_gpu_framebuffer_operations
        // Each pixel is 4 bytes: R, G, B, A
        let width = config.width;
        let height = config.height;
        let bytes_per_pixel = 4;
        
        // Write gradient data row by row (same pattern as VirtIO GPU test)
        for y in 0..height {
            for x in 0..width {
                // Create a simple gradient: red increasing with x, blue with y
                let red = if width > 1 { ((x * 255) / (width - 1)) as u8 } else { 0 };
                let blue = if height > 1 { ((y * 255) / (height - 1)) as u8 } else { 0 };
                let green = 0x80u8; // Fixed green component
                let alpha = 0xFFu8; // Fully opaque
                
                // Write pixel in RGBA format for character device
                let pixel = [red, green, blue, alpha];
                
                // Write pixel to framebuffer through character device
                let written = fb_char_device.write(&pixel).unwrap();
                assert_eq!(written, bytes_per_pixel);
            }
        }

        {
            // Flush the framebuffer using control operation (ioctl-equivalent)
            use crate::device::graphics::framebuffer_device::framebuffer_commands::FBIO_FLUSH;
            use crate::object::capability::ControlOps;
            let result = fb_char_device.control(FBIO_FLUSH, 0);
            assert!(result.is_ok(), "Failed to flush framebuffer via control operation");
        }
        
        // Verify we've written the entire framebuffer
        let expected_total_bytes = (width * height * bytes_per_pixel as u32) as usize;
        assert_eq!(fb_char_device.get_position(), expected_total_bytes);
        
        // Test reading back some pixels to verify the gradient
        fb_char_device.reset_position();
        
        // Read top-left pixel (should be red=0, blue=0, green=0x80)
        let mut top_left_pixel = [0u8; 4];
        let read_count = fb_char_device.read(&mut top_left_pixel);
        assert_eq!(read_count, 4);
        assert_eq!(top_left_pixel[0], 0);    // Red should be 0 at x=0
        assert_eq!(top_left_pixel[1], 0x80); // Fixed green component
        assert_eq!(top_left_pixel[2], 0);    // Blue should be 0 at y=0
        assert_eq!(top_left_pixel[3], 0xFF); // Full alpha
        
        // Skip to bottom-right pixel position
        let bottom_right_pos = expected_total_bytes - bytes_per_pixel;
        fb_char_device.set_position(bottom_right_pos);
        
        // Read bottom-right pixel (should be red=255, blue=255, green=0x80)
        let mut bottom_right_pixel = [0u8; 4];
        let read_count = fb_char_device.read(&mut bottom_right_pixel);
        assert_eq!(read_count, 4);
        assert_eq!(bottom_right_pixel[0], 255); // Red should be max at x=width-1
        assert_eq!(bottom_right_pixel[1], 0x80); // Fixed green component
        assert_eq!(bottom_right_pixel[2], 255);  // Blue should be max at y=height-1
        assert_eq!(bottom_right_pixel[3], 0xFF); // Full alpha
        
        // Test middle pixel (should have intermediate values)
        let middle_pos = (width / 2 * bytes_per_pixel as u32 + (height / 2) * width * bytes_per_pixel as u32) as usize;
        fb_char_device.set_position(middle_pos);
        
        let mut middle_pixel = [0u8; 4];
        let read_count = fb_char_device.read(&mut middle_pixel);
        assert_eq!(read_count, 4);
        // Middle pixel should have intermediate values
        let expected_red = ((width / 2) * 255 / (width - 1)) as u8;
        let expected_blue = ((height / 2) * 255 / (height - 1)) as u8;
        assert_eq!(middle_pixel[0], expected_red);  // Red based on x position
        assert_eq!(middle_pixel[1], 0x80);          // Fixed green component
        assert_eq!(middle_pixel[2], expected_blue); // Blue based on y position
        assert_eq!(middle_pixel[3], 0xFF);          // Full alpha
        
        // Verify we can't write beyond framebuffer boundary
        fb_char_device.set_position(expected_total_bytes);
        assert!(fb_char_device.write_byte(0xFF).is_err());
        assert!(!fb_char_device.can_write());
        
        // Reset position to within valid range and test read capability
        fb_char_device.set_position(0);
        assert!(fb_char_device.can_read());
        assert!(fb_char_device.can_write());
        
        // Gradient successfully drawn through /dev/fb0 character device
        // Framebuffer: 256x256 pixels, gradient pattern matching test_virtio_gpu_framebuffer_operations
        // Red increases with x (left to right), blue increases with y (top to bottom), green fixed at 0x80
        // Verified gradient colors in top-left, bottom-right, and middle pixels
    }
}