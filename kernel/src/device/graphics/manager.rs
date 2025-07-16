//! # Graphics Manager Module
//!
//! This module provides functionality for managing graphics devices and resources in the kernel.
//!
//! ## Overview
//!
//! The GraphicsManager is responsible for:
//! - Managing framebuffer resources from graphics devices
//! - Coordinating with DeviceManager for device discovery
//! - Creating and managing character devices for framebuffer access
//! - Future support for multi-display configurations and mmap operations
//!
//! ## Key Components
//!
//! - `GraphicsManager`: The main graphics management system
//! - `FramebufferResource`: Resource information extracted from graphics devices
//! - `DisplayConfiguration`: Configuration for display setups (future use)
//! - `MmapRegion`: Memory mapping region tracking (future use)

extern crate alloc;

use alloc::{format, string::{String, ToString}, sync::Arc, vec::Vec};
use hashbrown::HashMap;
use spin::{Mutex, RwLock};

use crate::device::{
    graphics::{FramebufferConfig, GraphicsDevice},
    manager::{DeviceManager, SharedDevice},
    DeviceType,
};

/// Framebuffer resource extracted from graphics devices
#[derive(Debug)]
pub struct FramebufferResource {
    /// DeviceManager's device id
    pub source_device_id: usize,
    /// Logical name for user access (e.g., "fb0")
    pub logical_name: String,
    /// Framebuffer configuration (resolution, format, etc.)
    pub config: FramebufferConfig,
    /// Physical memory address of the framebuffer
    pub physical_addr: usize,
    /// Size of the framebuffer in bytes
    pub size: usize,
    /// ID of the created /dev/fbX character device (if any)
    pub created_char_device_id: RwLock<Option<usize>>,
}

impl FramebufferResource {
    /// Create a new framebuffer resource
    pub fn new(
        source_device_id: usize,
        logical_name: String,
        config: FramebufferConfig,
        physical_addr: usize,
        size: usize,
    ) -> Self {
        Self {
            source_device_id,
            logical_name,
            config,
            physical_addr,
            size,
            created_char_device_id: RwLock::new(None),
        }
    }
}

/// Display configuration for multi-display setups (future use)
#[derive(Debug, Clone)]
pub struct DisplayConfiguration {
    /// Display identifier
    pub display_id: String,
    /// Associated framebuffer logical name
    pub framebuffer_name: String,
    /// Display position in multi-display setup
    pub position: (u32, u32),
    /// Display resolution
    pub resolution: (u32, u32),
    /// Whether this display is the primary display
    pub is_primary: bool,
}

/// Memory mapped region tracking (future use)
#[derive(Debug, Clone)]
pub struct MmapRegion {
    /// Virtual address of the mapped region
    pub virtual_addr: usize,
    /// Physical address of the mapped region  
    pub physical_addr: usize,
    /// Size of the mapped region
    pub size: usize,
    /// Associated framebuffer name
    pub framebuffer_name: String,
}

/// Graphics Manager - singleton for managing graphics resources
pub struct GraphicsManager {
    /// Framebuffer resources mapped by logical name
    framebuffers: Mutex<Option<HashMap<String, Arc<FramebufferResource>>>>,
    /// Multi-display configuration (future use)
    display_configs: Mutex<Vec<DisplayConfiguration>>,
    /// Active mmap regions (future use)
    active_mappings: Mutex<Vec<MmapRegion>>,
}

static mut MANAGER: GraphicsManager = GraphicsManager::new();

impl GraphicsManager {
    /// Create a new GraphicsManager instance
    pub const fn new() -> Self {
        Self {
            framebuffers: Mutex::new(None),
            display_configs: Mutex::new(Vec::new()),
            active_mappings: Mutex::new(Vec::new()),
        }
    }

    /// Get immutable reference to the global GraphicsManager instance
    #[allow(static_mut_refs)]
    pub fn get_manager() -> &'static GraphicsManager {
        unsafe { &MANAGER }
    }

    /// Get mutable reference to the global GraphicsManager instance
    #[allow(static_mut_refs)]
    pub fn get_mut_manager() -> &'static mut GraphicsManager {
        unsafe { &mut MANAGER }
    }

    /// Discover and register graphics devices from DeviceManager
    ///
    /// This method scans all devices in the DeviceManager for graphics devices
    /// and extracts their framebuffer resources for management.
    pub fn discover_graphics_devices(&mut self) {
        let device_manager = DeviceManager::get_manager();
        let device_count = device_manager.get_devices_count();

        for device_id in 0..device_count {
            let device = match device_manager.get_device(device_id) {
                Some(device) => device,
                None => {
                    crate::early_println!("[GraphicsManager] Device not found: {}", device_id);
                    continue;
                }
            };

            if device.device_type() == DeviceType::Graphics {
                if let Err(e) = self.register_framebuffer_from_device(device_id, device) {
                    crate::early_println!("[GraphicsManager] Failed to register framebuffer from device {}: {}", device_id, e);
                } else {
                    crate::early_println!("[GraphicsManager] Successfully registered framebuffer from device {}", device_id);
                }
            }
        }
    }

    /// Register a framebuffer resource from a specific graphics device
    ///
    /// # Arguments
    ///
    /// * `device_name` - The name of the device in DeviceManager
    /// * `device` - The shared device reference
    ///
    /// # Returns
    ///
    /// Result indicating success or failure
    pub fn register_framebuffer_from_device(
        &mut self,
        device_id: usize,
        device: SharedDevice,
    ) -> Result<(), &'static str> {
        // Cast to graphics device
        let graphics_device = device
            .as_graphics_device()
            .ok_or("Device is not a graphics device")?;

        // Initialize the graphics device if needed
        if let Some(mut_device) = device.as_any().downcast_ref::<crate::drivers::graphics::virtio_gpu::VirtioGpuDevice>() {
            // For VirtioGpuDevice, we need to ensure it's initialized
            let mut_ptr = mut_device as *const _ as *mut crate::drivers::graphics::virtio_gpu::VirtioGpuDevice;
            unsafe {
                if let Err(e) = (*mut_ptr).init_graphics() {
                    return Err(e);
                }
            }
        }

        // Extract framebuffer configuration
        let config = graphics_device.get_framebuffer_config()?;
        
        // Extract framebuffer address
        let physical_addr = graphics_device.get_framebuffer_address()?;
        
        // Calculate framebuffer size
        let size = config.size();

        // Generate logical name (fb0, fb1, etc.)
        let mut framebuffers = self.framebuffers.lock();
        if framebuffers.is_none() {
            *framebuffers = Some(HashMap::new());
        }
        let map = framebuffers.as_ref().unwrap();
        let logical_name = format!("fb{}", map.len());
        drop(framebuffers);

        // Create framebuffer resource
        let resource = Arc::new(FramebufferResource::new(
            device_id,
            logical_name.clone(),
            config,
            physical_addr,
            size,
        ));

        // Store the resource
        let mut framebuffers = self.framebuffers.lock();
        if framebuffers.is_none() {
            *framebuffers = Some(HashMap::new());
        }
        framebuffers.as_mut().unwrap().insert(logical_name.clone(), resource);
        drop(framebuffers);

        crate::early_println!("[GraphicsManager] Registered framebuffer resource: {} -> {}", device_id, logical_name);

        // Automatically create and register the character device
        if let Err(e) = self.create_framebuffer_char_device(&logical_name) {
            crate::early_println!("[GraphicsManager] Warning: Failed to create character device for {}: {}", logical_name, e);
        }
        
        Ok(())
    }

    /// Get a framebuffer resource by logical name
    ///
    /// # Arguments
    ///
    /// * `fb_name` - The logical name of the framebuffer (e.g., "fb0")
    ///
    /// # Returns
    ///
    /// Optional reference to the framebuffer resource
    pub fn get_framebuffer(&self, fb_name: &str) -> Option<Arc<FramebufferResource>> {
        let framebuffers = self.framebuffers.lock();
        framebuffers.as_ref()?.get(fb_name).cloned()
    }

    /// Get all registered framebuffer names
    ///
    /// # Returns
    ///
    /// Vector of logical framebuffer names
    pub fn get_framebuffer_names(&self) -> Vec<String> {
        let framebuffers = self.framebuffers.lock();
        if let Some(map) = framebuffers.as_ref() {
            map.keys().cloned().collect()
        } else {
            Vec::new()
        }
    }

    /// Get number of registered framebuffers
    ///
    /// # Returns
    ///
    /// Number of registered framebuffers
    pub fn get_framebuffer_count(&self) -> usize {
        let framebuffers = self.framebuffers.lock();
        if let Some(map) = framebuffers.as_ref() {
            map.len()
        } else {
            0
        }
    }

    /// Create a FramebufferCharDevice and register it with DeviceManager
    ///
    /// # Arguments
    ///
    /// * `fb_name` - The logical name of the framebuffer (e.g., "fb0")
    ///
    /// # Returns
    ///
    /// Result indicating success or failure
    pub fn create_framebuffer_char_device(&mut self, fb_name: &str) -> Result<(), &'static str> {
        use crate::device::{graphics::framebuffer_device::FramebufferCharDevice, manager::DeviceManager};
        use alloc::sync::Arc;
        
        // Get framebuffer resource
        let fb_resource = {
            let framebuffers = self.framebuffers.lock();
            framebuffers.as_ref()
                .and_then(|map| map.get(fb_name))
                .cloned()
                .ok_or("Framebuffer not found")?
        };
        
        // Create the character device
        let fb_char_device = FramebufferCharDevice::new(fb_resource);
        
        // Register with DeviceManager (this will automatically publish to DevFS)
        let device_manager = DeviceManager::get_mut_manager();
        let device_id = device_manager.register_device_with_name(
            fb_name.to_string(),
            Arc::new(fb_char_device)
        );
        
        // Update the framebuffer resource with the device ID
        self.set_char_device_id(fb_name, device_id)?;
        
        crate::early_println!("[GraphicsManager] Created framebuffer character device: /dev/{}", fb_name);
        Ok(())
    }

    /// Update the character device ID for a framebuffer resource
    ///
    /// # Arguments
    ///
    /// * `fb_name` - The logical name of the framebuffer
    /// * `char_device_id` - The character device ID from DeviceManager
    ///
    /// # Returns
    ///
    /// Result indicating success or failure
    pub fn set_char_device_id(
        &mut self,
        fb_name: &str,
        char_device_id: usize,
    ) -> Result<(), &'static str> {
        let mut framebuffers = self.framebuffers.lock();
        if let Some(map) = framebuffers.as_mut() {
            if let Some(resource) = map.get_mut(fb_name) {
                *resource.created_char_device_id.write() = Some(char_device_id);
                Ok(())
            } else {
                Err("Framebuffer not found")
            }
        } else {
            Err("Framebuffer not found")
        }
    }

    /// Read a single byte from the specified framebuffer
    ///
    /// # Arguments
    ///
    /// * `fb_name` - The logical name of the framebuffer
    /// * `position` - The position to read from
    ///
    /// # Returns
    ///
    /// The byte at the specified position, or None if invalid
    pub fn read_byte_from_framebuffer(&self, fb_name: &str, position: usize) -> Option<u8> {
        let fb_resource = self.get_framebuffer(fb_name)?;
        
        if position >= fb_resource.size {
            return None;
        }
        
        // Read byte from framebuffer memory
        unsafe {
            let fb_ptr = fb_resource.physical_addr as *const u8;
            Some(*fb_ptr.add(position))
        }
    }

    /// Write a single byte to the specified framebuffer
    ///
    /// # Arguments
    ///
    /// * `fb_name` - The logical name of the framebuffer
    /// * `position` - The position to write to
    /// * `byte` - The byte to write
    ///
    /// # Returns
    ///
    /// Result indicating success or failure
    pub fn write_byte_to_framebuffer(&self, fb_name: &str, position: usize, byte: u8) -> Result<(), &'static str> {
        let fb_resource = self.get_framebuffer(fb_name)
            .ok_or("Framebuffer not found")?;
        
        if position >= fb_resource.size {
            return Err("Position beyond framebuffer size");
        }
        
        // Write byte to framebuffer memory
        unsafe {
            let fb_ptr = fb_resource.physical_addr as *mut u8;
            *fb_ptr.add(position) = byte;
        }
        
        Ok(())
    }

    /// Read multiple bytes from the specified framebuffer
    ///
    /// # Arguments
    ///
    /// * `fb_name` - The logical name of the framebuffer
    /// * `position` - The starting position to read from
    /// * `buffer` - The buffer to read data into
    ///
    /// # Returns
    ///
    /// The number of bytes actually read
    pub fn read_framebuffer(&self, fb_name: &str, position: usize, buffer: &mut [u8]) -> usize {
        let fb_resource = match self.get_framebuffer(fb_name) {
            Some(resource) => resource,
            None => return 0,
        };
        
        let available_bytes = fb_resource.size.saturating_sub(position);
        let bytes_to_read = buffer.len().min(available_bytes);
        
        if bytes_to_read == 0 {
            return 0;
        }
        
        // Read bytes from framebuffer memory
        unsafe {
            let fb_ptr = fb_resource.physical_addr as *const u8;
            let src = fb_ptr.add(position);
            core::ptr::copy_nonoverlapping(src, buffer.as_mut_ptr(), bytes_to_read);
        }
        
        bytes_to_read
    }

    /// Write multiple bytes to the specified framebuffer
    ///
    /// # Arguments
    ///
    /// * `fb_name` - The logical name of the framebuffer
    /// * `position` - The starting position to write to
    /// * `buffer` - The buffer containing data to write
    ///
    /// # Returns
    ///
    /// Result containing the number of bytes written or an error
    pub fn write_framebuffer(&self, fb_name: &str, position: usize, buffer: &[u8]) -> Result<usize, &'static str> {
        let fb_resource = self.get_framebuffer(fb_name)
            .ok_or("Framebuffer not found")?;
        
        let available_space = fb_resource.size.saturating_sub(position);
        let bytes_to_write = buffer.len().min(available_space);
        
        if bytes_to_write == 0 {
            return Err("No space available in framebuffer");
        }
        
        // Write bytes to framebuffer memory
        unsafe {
            let fb_ptr = fb_resource.physical_addr as *mut u8;
            let dst = fb_ptr.add(position);
            core::ptr::copy_nonoverlapping(buffer.as_ptr(), dst, bytes_to_write);
        }
        
        Ok(bytes_to_write)
    }

    /// Clear all framebuffers (for testing only)
    /// This allows tests to start with a clean GraphicsManager state
    #[cfg(test)]
    pub fn clear_for_test(&mut self) {
        use crate::device::manager::DeviceManager;
        
        // Clear GraphicsManager state
        let mut framebuffers = self.framebuffers.lock();
        *framebuffers = None;
        
        let mut display_configs = self.display_configs.lock();
        display_configs.clear();
        
        let mut active_mappings = self.active_mappings.lock();
        active_mappings.clear();
        
        // Also clear DeviceManager state to ensure test isolation
        DeviceManager::get_mut_manager().clear_for_test();
    }
}

#[cfg(test)]
mod test_utils {
    use super::*;

    /// Create an independent GraphicsManager for testing
    /// This allows each test to have its own isolated manager instance
    pub fn create_test_graphics_manager() -> GraphicsManager {
        GraphicsManager::new()
    }

    /// Setup a clean GraphicsManager for testing
    /// This clears the global singleton and returns a mutable reference to it
    /// ensuring each test starts with a clean state
    pub fn setup_clean_global_graphics_manager() -> &'static mut GraphicsManager {
        let manager = GraphicsManager::get_mut_manager();
        manager.clear_for_test();
        manager
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
    fn test_framebuffer_resource_creation() {
        let config = FramebufferConfig::new(1024, 768, PixelFormat::RGBA8888);
        let resource = FramebufferResource::new(
            0,
            "fb0".to_string(),
            config.clone(),
            0x80000000,
            config.size(),
        );

        assert_eq!(resource.source_device_id, 0);
        assert_eq!(resource.logical_name, "fb0");
        assert_eq!(resource.config.width, 1024);
        assert_eq!(resource.config.height, 768);
        assert_eq!(resource.physical_addr, 0x80000000);
        assert_eq!(resource.size, 1024 * 768 * 4);
        assert_eq!(*resource.created_char_device_id.read(), None);
    }

    #[test_case]
    fn test_graphics_manager_initialization() {
        let manager = GraphicsManager::new();
        assert_eq!(manager.get_framebuffer_count(), 0);
        assert_eq!(manager.get_framebuffer_names().len(), 0);
    }

    #[test_case]
    fn test_graphics_manager_singleton() {
        let manager1 = GraphicsManager::get_manager();
        let manager2 = GraphicsManager::get_manager();
        
        // Both should point to the same instance
        assert_eq!(manager1 as *const _, manager2 as *const _);
    }

    #[test_case]
    fn test_framebuffer_registration() {
        let mut manager = test_utils::create_test_graphics_manager();
        
        // Create a test graphics device
        let mut device = GenericGraphicsDevice::new("test-gpu");
        let config = FramebufferConfig::new(800, 600, PixelFormat::BGRA8888);
        device.set_framebuffer_config(config.clone());
        device.set_framebuffer_address(0x90000000);
        
        let shared_device: SharedDevice = Arc::new(device);
        
        // Register the device
        let result = manager.register_framebuffer_from_device(0, shared_device);
        assert!(result.is_ok());
        
        // Check that framebuffer was registered
        assert_eq!(manager.get_framebuffer_count(), 1);
        let names = manager.get_framebuffer_names();
        assert_eq!(names.len(), 1);
        assert_eq!(names[0], "fb0");
        
        // Check framebuffer details
        let fb = manager.get_framebuffer("fb0").unwrap();
        assert_eq!(fb.source_device_id, 0);
        assert_eq!(fb.logical_name, "fb0");
        assert_eq!(fb.config.width, 800);
        assert_eq!(fb.config.height, 600);
        assert_eq!(fb.physical_addr, 0x90000000);
        assert_eq!(fb.size, 800 * 600 * 4);
    }

    #[test_case]
    fn test_multiple_framebuffer_registration() {
        let mut manager = test_utils::create_test_graphics_manager();
        
        // Create first device
        let mut device1 = GenericGraphicsDevice::new("test-gpu1");
        let config1 = FramebufferConfig::new(1920, 1080, PixelFormat::RGBA8888);
        device1.set_framebuffer_config(config1.clone());
        device1.set_framebuffer_address(0x80000000);
        let shared_device1: SharedDevice = Arc::new(device1);
        
        // Create second device
        let mut device2 = GenericGraphicsDevice::new("test-gpu2");
        let config2 = FramebufferConfig::new(1024, 768, PixelFormat::BGRA8888);
        device2.set_framebuffer_config(config2.clone());
        device2.set_framebuffer_address(0x90000000);
        let shared_device2: SharedDevice = Arc::new(device2);
        
        // Register both devices
        assert!(manager.register_framebuffer_from_device(1, shared_device1).is_ok());
        assert!(manager.register_framebuffer_from_device(2, shared_device2).is_ok());
        
        // Check registration
        assert_eq!(manager.get_framebuffer_count(), 2);
        let names = manager.get_framebuffer_names();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"fb0".to_string()));
        assert!(names.contains(&"fb1".to_string()));
        
        // Check individual framebuffers
        let fb0 = manager.get_framebuffer("fb0").unwrap();
        let fb1 = manager.get_framebuffer("fb1").unwrap();

        assert_eq!(fb0.source_device_id, 1);
        assert_eq!(fb1.source_device_id, 2);
        assert_ne!(fb0.physical_addr, fb1.physical_addr);
    }

    #[test_case]
    fn test_char_device_id_assignment() {
        let mut manager = test_utils::create_test_graphics_manager();
        
        // Create and register a device
        let mut device = GenericGraphicsDevice::new("test-gpu");
        let config = FramebufferConfig::new(640, 480, PixelFormat::RGB888);
        device.set_framebuffer_config(config);
        device.set_framebuffer_address(0x80000000);
        let shared_device: SharedDevice = Arc::new(device);

        manager.register_framebuffer_from_device(0, shared_device).unwrap();

        // Set character device ID
        assert!(manager.set_char_device_id("fb0", 42).is_ok());
        
        // Verify the ID was set
        let fb = manager.get_framebuffer("fb0").unwrap();
        assert_eq!(*fb.created_char_device_id.read(), Some(42));
        
        // Test setting ID for non-existent framebuffer
        assert!(manager.set_char_device_id("fb999", 123).is_err());
    }

    #[test_case]
    fn test_framebuffer_not_found() {
        let manager = GraphicsManager::new();
        
        // Try to get non-existent framebuffer
        assert!(manager.get_framebuffer("non_existent").is_none());
        
        // Empty manager should return empty results
        assert_eq!(manager.get_framebuffer_count(), 0);
        assert_eq!(manager.get_framebuffer_names().len(), 0);
    }
}