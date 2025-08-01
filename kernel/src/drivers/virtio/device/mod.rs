//! Virtio device driver interface module.
//! 

use core::result::Result;

use alloc::{boxed::Box, sync::Arc, vec};

use crate::{device::{manager::{DeviceManager, DriverPriority}, platform::{resource::PlatformDeviceResourceType, PlatformDeviceDriver, PlatformDeviceInfo}, Device}, driver_initcall, drivers::{block::virtio_blk::VirtioBlockDevice, graphics::virtio_gpu::VirtioGpuDevice, network::virtio_net::VirtioNetDevice, virtio::queue}};

/// Register enum for Virtio devices
/// 
/// This enum represents the registers of the Virtio device.
/// Each variant corresponds to a specific register offset.
/// The offsets are defined in the Virtio specification.
/// The register offsets are used to access the device's configuration and status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Register {
    MagicValue = 0x00,
    Version = 0x04,
    DeviceId = 0x08,
    VendorId = 0x0c,
    DeviceFeatures = 0x10,
    DeviceFeaturesSel = 0x14,
    DriverFeatures = 0x20,
    DriverFeaturesSel = 0x24,
    GuestPageSize = 0x28,
    QueueSel = 0x30,
    QueueNumMax = 0x34,
    QueueNum = 0x38,
    QueueAlign = 0x3c,
    QueuePfn = 0x40,
    QueueReady = 0x44,
    QueueNotify = 0x50,
    InterruptStatus = 0x60,
    InterruptAck = 0x64,
    Status = 0x70,
    QueueDescLow = 0x80,
    QueueDescHigh = 0x84,
    DriverDescLow = 0x90,
    DriverDescHigh = 0x94,
    DeviceDescLow = 0xa0,
    DeviceDescHigh = 0xa4,
    DeviceConfig = 0x100,
}

impl Register {
    pub fn offset(&self) -> usize {
        *self as usize
    }

    pub fn from_offset(offset: usize) -> Self {
        match offset {
            0x00 => Register::MagicValue,
            0x04 => Register::Version,
            0x08 => Register::DeviceId,
            0x0c => Register::VendorId,
            0x10 => Register::DeviceFeatures,
            0x14 => Register::DeviceFeaturesSel,
            0x20 => Register::DriverFeatures,
            0x24 => Register::DriverFeaturesSel,
            0x28 => Register::GuestPageSize,
            0x30 => Register::QueueSel,
            0x34 => Register::QueueNumMax,
            0x38 => Register::QueueNum,
            0x3c => Register::QueueAlign,
            0x40 => Register::QueuePfn,
            0x44 => Register::QueueReady,
            0x50 => Register::QueueNotify,
            0x60 => Register::InterruptStatus,
            0x64 => Register::InterruptAck,
            0x70 => Register::Status,
            0x80 => Register::QueueDescLow,
            0x84 => Register::QueueDescHigh,
            0x90 => Register::DriverDescLow,
            0x94 => Register::DriverDescHigh,
            0xa0 => Register::DeviceDescLow,
            0xa4 => Register::DeviceDescHigh,
            _ => panic!("Invalid register offset"),
        }
    }
}

/// DeviceStatus enum for Virtio devices
/// 
/// This enum represents the status of the Virtio device.
/// Each variant corresponds to a specific status bit.
/// The status bits are defined in the Virtio specification.
#[derive(Debug, Clone, Copy)]
pub enum DeviceStatus {
    Reset = 0x00,
    Acknowledge = 0x01,
    Driver = 0x02,
    DriverOK = 0x04,
    FeaturesOK = 0x08,
    DeviceNeedReset = 0x40,
    Failed = 0x80,
}

impl DeviceStatus {
    /// Check if the status is set
    /// 
    /// This method checks if the specified status bit is set in the given status.
    /// 
    /// # Arguments
    /// 
    /// * `status` - The status to check.
    /// 
    /// # Returns
    /// 
    /// Returns true if the status bit is set, false otherwise.
    pub fn is_set(&self, status: u32) -> bool {
        (status & *self as u32) != 0
    }

    /// Set the status bit
    /// 
    /// This method sets the specified status bit in the given status.
    /// 
    /// # Arguments
    /// 
    /// * `status` - A mutable reference to the status to modify.
    /// 
    pub fn set(&self, status: &mut u32) {
        *status |= *self as u32;
    }

    /// Clear the status bit
    /// 
    /// This method clears the specified status bit in the given status.
    /// 
    /// # Arguments
    /// 
    /// * `status` - A mutable reference to the status to modify.
    /// 
    pub fn clear(&self, status: &mut u32) {
        *status &= !(*self as u32);
    }

    /// Toggle the status bit
    /// 
    /// This method toggles the specified status bit in the given status.
    /// 
    /// # Arguments
    /// 
    /// * `status` - A mutable reference to the status to modify.
    /// 
    pub fn toggle(&self, status: &mut u32) {
        *status ^= *self as u32;
    }

    /// Convert from u32 to DeviceStatus
    /// 
    /// This method converts a u32 value to the corresponding DeviceStatus variant.
    /// 
    /// # Arguments
    /// 
    /// * `status` - The u32 value to convert.
    /// 
    /// # Returns
    /// 
    /// Returns the corresponding DeviceStatus variant.
    ///
    pub fn from_u32(status: u32) -> Self {
        match status {
            0x00 => DeviceStatus::Reset,
            0x01 => DeviceStatus::Acknowledge,
            0x02 => DeviceStatus::Driver,
            0x04 => DeviceStatus::DriverOK,
            0x08 => DeviceStatus::FeaturesOK,
            0x40 => DeviceStatus::DeviceNeedReset,
            0x80 => DeviceStatus::Failed,
            _ => panic!("Invalid device status"),
        }
    }
    
    /// Convert DeviceStatus to u32
    /// 
    /// This method converts the DeviceStatus variant to its corresponding u32 value.
    /// 
    /// # Returns
    /// 
    /// Returns the u32 value corresponding to the DeviceStatus variant.
    /// 
    pub fn to_u32(&self) -> u32 {
        *self as u32
    }
}

/// VirtioDevice trait
/// 
/// This trait defines the interface for VirtIO devices.
/// It provides methods for initializing the device, accessing registers,
/// and performing device operations according to the VirtIO specification.
pub trait VirtioDevice {
    /// Initialize the device
    ///
    /// This method performs the standard VirtIO initialization sequence:
    /// 1. Reset the device
    /// 2. Acknowledge the device
    /// 3. Set driver status
    /// 4. Negotiate features
    /// 5. Set up virtqueues
    /// 6. Set driver OK status
    ///
    /// # Returns
    ///
    /// Returns Ok(negotiated_features) if initialization was successful,
    /// Err message otherwise
    fn init(&mut self) -> Result<u32, &'static str> {
        // Verify device (Magic Value should be "virt")
        if self.read32_register(Register::MagicValue) != 0x74726976 {
            self.set_failed();
            return Err("Invalid Magic Value");
        }

        // Check device version
        let version = self.read32_register(Register::Version);
        if version != 2 {
            self.set_failed();
            return Err("Invalid Version");
        }

        // Reset device
        self.reset();
        
        // Acknowledge device
        self.acknowledge();

        // Set driver status
        self.driver();

        // Negotiate features
        let negotiated_features = match self.negotiate_features() {
            Ok(features) => features,
            Err(e) => {
                self.set_failed();
                return Err(e);
            }
        };

        // Set up virtqueues
        for i in 0..self.get_virtqueue_count() {
            if !self.setup_queue(i, self.get_virtqueue_size(i)) {
                self.set_failed();
                return Err("Failed to set up virtqueue");
            }
        }

        // Mark driver OK
        self.driver_ok();
        Ok(negotiated_features)
    }

    /// Reset the device by writing 0 to the Status register
    fn reset(&mut self) {
        self.write32_register(Register::Status, 0);
    }

    /// Set ACKNOWLEDGE status bit
    fn acknowledge(&mut self) {
        let mut status = self.read32_register(Register::Status);
        DeviceStatus::Acknowledge.set(&mut status);
        self.write32_register(Register::Status, status);
    }

    /// Set DRIVER status bit
    fn driver(&mut self) {
        let mut status = self.read32_register(Register::Status);
        DeviceStatus::Driver.set(&mut status);
        self.write32_register(Register::Status, status);
    }

    /// Set DRIVER_OK status bit
    fn driver_ok(&mut self) {
        let mut status = self.read32_register(Register::Status);
        DeviceStatus::DriverOK.set(&mut status);
        self.write32_register(Register::Status, status);
    }

    /// Set FAILED status bit
    fn set_failed(&mut self) {
        let mut status = self.read32_register(Register::Status);
        DeviceStatus::Failed.set(&mut status);
        self.write32_register(Register::Status, status);
    }

    /// Negotiate device features
    ///
    /// This method reads device features, selects supported features, 
    /// sets driver features, and verifies features OK status.
    ///
    /// # Returns
    ///
    /// Returns Ok(negotiated_features) if feature negotiation was successful, 
    /// Err message otherwise
    fn negotiate_features(&mut self) -> Result<u32, &'static str> {
        // Read device features
        let device_features = self.read32_register(Register::DeviceFeatures);
        // Select supported features
        let driver_features = self.get_supported_features(device_features);
        
        #[cfg(test)]
        {
            use crate::early_println;
            early_println!("[virtio] Negotiating features: device=0x{:x}, driver=0x{:x}", device_features, driver_features);
        }
        
        // Write driver features
        self.write32_register(Register::DriverFeatures, driver_features);
        
        // Set FEATURES_OK status bit
        let mut status = self.read32_register(Register::Status);
        DeviceStatus::FeaturesOK.set(&mut status);
        self.write32_register(Register::Status, status);
        
        // Verify FEATURES_OK status bit
        let final_status = self.read32_register(Register::Status);
        let success = DeviceStatus::FeaturesOK.is_set(final_status);
        
        #[cfg(test)]
        {
            use crate::early_println;
            early_println!("[virtio] Feature negotiation result: success={}, status=0x{:x}", success, final_status);
        }
        
        if success {
            Ok(driver_features)
        } else {
            Err("Feature negotiation failed")
        }
    }
    
    /// Get device features supported by this driver
    ///
    /// This method can be overridden by specific device implementations
    /// to select which features to support.
    ///
    /// # Arguments
    ///
    /// * `device_features` - The features offered by the device
    ///
    /// # Returns
    ///
    /// The features supported by the driver
    fn get_supported_features(&self, device_features: u32) -> u32 {
        // By default, accept all device features
        // Device-specific implementations should override this
        device_features
    }
    
    /// Set up a virtqueue
    ///
    /// This method configures a virtqueue by setting the queue selection,
    /// size, alignment, and ready status.
    ///
    /// # Arguments
    ///
    /// * `queue_idx` - The index of the queue to set up
    ///
    /// # Returns
    ///
    /// Returns true if queue setup was successful, false otherwise
    fn setup_queue(&mut self, queue_idx: usize, queue_size: usize) -> bool {
        if queue_idx >= self.get_virtqueue_count() {
            return false;
        }
        
        // Select the queue
        self.write32_register(Register::QueueSel, queue_idx as u32);
        // Check if the queue is ready
        let ready = self.read32_register(Register::QueueReady);
        if ready != 0 {
            return false; // Queue already set up
        }
        
        // Get maximum queue size
        let queue_size_max = self.read32_register(Register::QueueNumMax);
        if queue_size > queue_size_max as usize {
            return false; // Requested size exceeds maximum
        }
        
        // Set queue size
        self.write32_register(Register::QueueNum, queue_size as u32);
        
        // Get queue addresses directly - safer than closures
        let desc_addr = self.get_queue_desc_addr(queue_idx);
        let driver_addr = self.get_queue_driver_addr(queue_idx);
        let device_addr = self.get_queue_device_addr(queue_idx);
        
        if desc_addr.is_none() || driver_addr.is_none() || device_addr.is_none() {
            return false;
        }
        
        let desc_addr = desc_addr.unwrap();
        let driver_addr = driver_addr.unwrap();
        let device_addr = device_addr.unwrap();
        
        // Set the queue descriptor address
        let desc_addr_low = (desc_addr & 0xffffffff) as u32;
        let desc_addr_high = (desc_addr >> 32) as u32;
        self.write32_register(Register::QueueDescLow, desc_addr_low);
        self.write32_register(Register::QueueDescHigh, desc_addr_high);

        // Set the driver area (available ring) address
        let driver_addr_low = (driver_addr & 0xffffffff) as u32;
        let driver_addr_high = (driver_addr >> 32) as u32;
        self.write32_register(Register::DriverDescLow, driver_addr_low);
        self.write32_register(Register::DriverDescHigh, driver_addr_high);

        // Set the device area (used ring) address
        let device_addr_low = (device_addr & 0xffffffff) as u32;
        let device_addr_high = (device_addr >> 32) as u32;
        self.write32_register(Register::DeviceDescLow, device_addr_low);
        self.write32_register(Register::DeviceDescHigh, device_addr_high);

        // Check the status of the queue
        let status = self.read32_register(Register::Status);
        if DeviceStatus::Failed.is_set(status) {
            return false; // Queue setup failed
        }
        
        // Mark queue as ready
        self.write32_register(Register::QueueReady, 1);

        // Check the status of the queue
        let status = self.read32_register(Register::Status);
        if DeviceStatus::Failed.is_set(status) {
            return false; // Queue setup failed
        }
        
        true
    }
    
    /// Read device-specific configuration
    ///
    /// This method reads configuration data from the device-specific configuration space.
    ///
    /// # Arguments
    ///
    /// * `offset` - The offset within the configuration space
    ///
    /// # Returns
    ///
    /// The configuration value of type T
    fn read_config<T: Sized>(&self, offset: usize) -> T {
        let addr = self.get_base_addr() + Register::DeviceConfig.offset() + offset;
        unsafe { core::ptr::read_volatile(addr as *const T) }
    }
    
    /// Write device-specific configuration
    ///
    /// This method writes configuration data to the device-specific configuration space.
    ///
    /// # Arguments
    ///
    /// * `offset` - The offset within the configuration space
    /// * `value` - The value to write
    fn write_config<T: Sized>(&self, offset: usize, value: T) {
        let addr = self.get_base_addr() + Register::DeviceConfig.offset() + offset;
        unsafe { core::ptr::write_volatile(addr as *mut T, value) }
    }
    
    /// Get device and vendor IDs
    ///
    /// # Returns
    ///
    /// A tuple containing (device_id, vendor_id)
    fn get_device_info(&self) -> (u32, u32) {
        let device_id = self.read32_register(Register::DeviceId);
        let vendor_id = self.read32_register(Register::VendorId);
        (device_id, vendor_id)
    }
    
    /// Get interrupt status
    ///
    /// # Returns
    ///
    /// The interrupt status register value
    fn get_interrupt_status(&self) -> u32 {
        self.read32_register(Register::InterruptStatus)
    }
    
    /// Process interrupts (polling method)
    ///
    /// This method checks for interrupts and acknowledges them.
    ///
    /// # Returns
    ///
    /// The interrupt status before acknowledgment
    fn process_interrupts(&mut self) -> u32 {
        let status = self.get_interrupt_status();
        if status != 0 {
            self.write32_register(Register::InterruptAck, status & 0x03);
        }
        status
    }
    
    /// Memory barrier for ensuring memory operations ordering
    fn memory_barrier(&self) {
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
    }
    
    /// Notify the device about new buffers in a specified virtqueue
    ///
    /// This method notifies the device that new buffers are available in the specified virtqueue.
    /// It selects the queue using the QueueSel register and then writes to the QueueNotify register.
    ///
    /// # Arguments
    ///
    /// * `virtqueue_idx` - The index of the virtqueue to notify
    ///
    /// # Panics
    ///
    /// Panics if the virtqueue index is invalid
    fn notify(&self, virtqueue_idx: usize) {
        if virtqueue_idx >= self.get_virtqueue_count() {
            panic!("Invalid virtqueue index");
        }
        // Insert memory barrier before notification
        self.memory_barrier();
        self.write32_register(Register::QueueNotify, virtqueue_idx as u32);
    }

    /// Read a 32-bit value from a device register
    ///
    /// # Arguments
    ///
    /// * `register` - The register to read from
    ///
    /// # Returns
    ///
    /// The 32-bit value read from the register
    fn read32_register(&self, register: Register) -> u32 {
        let addr = self.get_base_addr() + register.offset();
        unsafe { core::ptr::read_volatile(addr as *const u32) }
    }

    /// Write a 32-bit value to a device register
    ///
    /// # Arguments
    ///
    /// * `register` - The register to write to
    /// * `value` - The 32-bit value to write
    fn write32_register(&self, register: Register, value: u32) {
        let addr = self.get_base_addr() + register.offset();
        unsafe { core::ptr::write_volatile(addr as *mut u32, value) }
    }

    /// Read a 64-bit value from a device register
    ///
    /// # Arguments
    ///
    /// * `register` - The register to read from
    ///
    /// # Returns
    ///
    /// The 64-bit value read from the register
    fn read64_register(&self, register: Register) -> u64 {
        let addr = self.get_base_addr() + register.offset();
        unsafe { core::ptr::read_volatile(addr as *const u64) }
    }

    /// Write a 64-bit value to a device register
    ///
    /// # Arguments
    ///
    /// * `register` - The register to write to
    /// * `value` - The 64-bit value to write
    fn write64_register(&self, register: Register, value: u64) {
        let addr = self.get_base_addr() + register.offset();
        unsafe { core::ptr::write_volatile(addr as *mut u64, value) }
    }

    // Required methods to be implemented by specific device types

    fn get_base_addr(&self) -> usize;
    fn get_virtqueue_count(&self) -> usize;
    fn get_virtqueue_size(&self, queue_idx: usize) -> usize;
    
    /// Get the descriptor address for a virtqueue
    fn get_queue_desc_addr(&self, queue_idx: usize) -> Option<u64>;
    
    /// Get the driver area address for a virtqueue
    fn get_queue_driver_addr(&self, queue_idx: usize) -> Option<u64>;
    
    /// Get the device area address for a virtqueue
    fn get_queue_device_addr(&self, queue_idx: usize) -> Option<u64>;
}


/// Device type enum for Virtio devices
/// 
/// This enum represents the different types of Virtio devices.
/// Each variant corresponds to a specific device type.
/// The types are defined in the Virtio specification.
pub enum VirtioDeviceType {
    Invalid = 0,
    Net = 1,
    Block = 2,
    Console = 3,
    Rng = 4,
    GPU = 16,
}

impl VirtioDeviceType {
    /// Convert from u32 to VirtioDeviceType
    /// 
    /// This method converts a u32 value to the corresponding VirtioDeviceType variant.
    /// 
    /// # Arguments
    /// 
    /// * `device_type` - The u32 value to convert.
    /// 
    /// # Returns
    /// 
    /// Returns the corresponding VirtioDeviceType variant.
    pub fn from_u32(device_type: u32) -> Self {
        match device_type {
            0 => VirtioDeviceType::Invalid,
            1 => VirtioDeviceType::Net,
            2 => VirtioDeviceType::Block,
            3 => VirtioDeviceType::Console,
            4 => VirtioDeviceType::Rng,
            16 => VirtioDeviceType::GPU,
            _ => panic!("Not supported device type"),
        }
    }
}

/// Virtio Common Device
/// 
/// Only use this struct for checking the device info.
/// It should not be used for actual device operations.
/// 
struct VirtioDeviceCommon {
    base_addr: usize,
}

impl VirtioDeviceCommon {
    /// Create a new Virtio device
    ///
    /// # Arguments
    ///
    /// * `base_addr` - The base address of the device
    ///
    /// # Returns
    ///
    /// A new instance of `VirtioDeviceCommon`
    pub fn new(base_addr: usize) -> Self {
        Self { base_addr }
    }
}

impl VirtioDevice for VirtioDeviceCommon {
    fn init(&mut self) -> Result<u32, &'static str> {
        // Initialization is not required for the common device
        Ok(0)
    }

    fn get_base_addr(&self) -> usize {
        self.base_addr
    }

    fn get_virtqueue_count(&self) -> usize {
        // This should be overridden by specific device implementations
        0
    }

    fn get_virtqueue_size(&self, _queue_idx: usize) -> usize {
        // This should be overridden by specific device implementations
        0
    }

    fn get_queue_desc_addr(&self, _queue_idx: usize) -> Option<u64> {
        // This should be overridden by specific device implementations
        None
    }
    
    fn get_queue_driver_addr(&self, _queue_idx: usize) -> Option<u64> {
        // This should be overridden by specific device implementations
        None
    }
    
    fn get_queue_device_addr(&self, _queue_idx: usize) -> Option<u64> {
        // This should be overridden by specific device implementations
        None
    }
}

fn probe_fn(device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    let res = device.get_resources();
    if res.is_empty() {
        return Err("No resources found");
    }

    // Get memory region resource (res_type == PlatformDeviceResourceType::MEM)
    let mem_res = res.iter()
        .find(|r| r.res_type == PlatformDeviceResourceType::MEM)
        .ok_or("Memory resource not found")?;
    
    let base_addr = mem_res.start as usize;

    // Create a new Virtio device
    let virtio_device = VirtioDeviceCommon::new(base_addr);
    // Check device type
    let device_type = VirtioDeviceType::from_u32(virtio_device.get_device_info().0);
    
    match device_type {
        VirtioDeviceType::Block => {
            crate::early_println!("[Virtio] Detected Virtio Block Device at {:#x}", base_addr);
            let dev: Arc<dyn Device> = Arc::new(VirtioBlockDevice::new(base_addr));
            DeviceManager::get_mut_manager().register_device(dev);
        }
        VirtioDeviceType::Net => {
            crate::early_println!("[Virtio] Detected Virtio Network Device at {:#x}", base_addr);
            let dev: Arc<dyn Device> = Arc::new(VirtioNetDevice::new(base_addr));
            DeviceManager::get_mut_manager().register_device(dev);
        }
        VirtioDeviceType::GPU => {
            crate::early_println!("[Virtio] Detected Virtio GPU Device at {:#x}", base_addr);
            let dev: Arc<dyn Device> = Arc::new(VirtioGpuDevice::new(base_addr));
            DeviceManager::get_mut_manager().register_device(dev);
        }
        _ => {
            // Unsupported device type
            return Err("Unsupported device type");
        }
    }

    Ok(())
}

fn remove_fn(_device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    Ok(())
}

fn register_driver() {
    let driver = PlatformDeviceDriver::new(
        "virtio-mmio",
        probe_fn,
        remove_fn,
        vec!["virtio,mmio"],
    );
    // Register the driver with the kernel
    DeviceManager::get_mut_manager().register_driver(Box::new(driver), DriverPriority::Standard)
}

driver_initcall!(register_driver);

#[cfg(test)]
mod tests;