use alloc::{boxed::Box, vec::Vec};

use super::BlockDevice;

extern crate alloc;

/// Block device manager
///
/// This struct manages block devices in the system.
/// It provides methods for registering, accessing, and managing block devices.
pub struct BlockDeviceManager {
    devices: Vec<Box<dyn BlockDevice>>,
}

static mut INSTANCE: BlockDeviceManager = BlockDeviceManager::new();

impl BlockDeviceManager {
    /// Create a new block device manager
    ///
    /// This function creates a new empty block device manager.
    const fn new() -> Self {
        Self {
            devices: Vec::new(),
        }
    }

    /// Get a reference to the global block device manager
    ///
    /// # Returns
    ///
    /// A reference to the global block device manager instance
    #[allow(static_mut_refs)]
    pub fn get_manager() -> &'static Self {
        unsafe { &INSTANCE }
    }

    /// Get a mutable reference to the global block device manager
    ///
    /// # Returns
    ///
    /// A mutable reference to the global block device manager instance
    #[allow(static_mut_refs)]
    pub fn get_mut_manager() -> &'static mut Self {
        unsafe { &mut INSTANCE }
    }

    /// Register a block device with the manager
    ///
    /// # Arguments
    ///
    /// * `device` - The block device to register
    pub fn register_device(&mut self, device: Box<dyn BlockDevice>) {
        self.devices.push(device);
    }

    /// Get a block device by ID
    ///
    /// # Arguments
    ///
    /// * `id` - The ID of the device to get
    ///
    /// # Returns
    ///
    /// An option containing a reference to the device, or None if not found
    pub fn get_device(&self, id: usize) -> Option<&dyn BlockDevice> {
        for device in &self.devices {
            if device.get_id() == id {
                return Some(&**device);
            }
        }
        None
    }

    /// Get a mutable reference to a block device by ID
    ///
    /// # Arguments
    ///
    /// * `id` - The ID of the device to get
    ///
    /// # Returns
    ///
    /// An option containing a mutable reference to the device, or None if not found
    pub fn get_mut_device(&mut self, id: usize) -> Option<&mut dyn BlockDevice> {
        for device in &mut self.devices {
            if device.get_id() == id {
                return Some(&mut **device);
            }
        }
        None
    }

    /// Get all registered block devices
    ///
    /// # Returns
    ///
    /// A slice containing all registered block devices
    pub fn get_devices(&self) -> &[Box<dyn BlockDevice>] {
        &self.devices
    }

    /// Get the number of registered block devices
    ///
    /// # Returns
    ///
    /// The number of registered block devices
    pub fn get_devices_count(&self) -> usize {
        self.devices.len()
    }
}