//! Virtio device driver interface module.
//! 

use super::queue::VirtQueue;

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
    fn init(&mut self) {
        // Reset device
        self.reset();

        // Verify device (Magic Value should be "virt")
        if self.read32_register(Register::MagicValue) != 0x74726976 {
            self.set_failed();
            return;
        }

        // Acknowledge device
        self.acknowledge();

        // Set driver status
        self.driver();

        // Negotiate features
        if !self.negotiate_features() {
            self.set_failed();
            return;
        }

        // Set up virtqueues
        for i in 0..self.get_virtqueue_count() {
            if !self.setup_queue(i) {
                self.set_failed();
                return;
            }
        }

        // Mark driver OK
        self.driver_ok();
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
    /// Returns true if feature negotiation was successful, false otherwise
    fn negotiate_features(&mut self) -> bool {
        // Read device features
        let device_features = self.read32_register(Register::DeviceFeatures);
        
        // Select supported features
        let driver_features = self.get_supported_features(device_features);
        
        // Write driver features
        self.write32_register(Register::DriverFeatures, driver_features);
        
        // Set FEATURES_OK status bit
        let mut status = self.read32_register(Register::Status);
        DeviceStatus::FeaturesOK.set(&mut status);
        self.write32_register(Register::Status, status);
        
        // Verify FEATURES_OK status bit
        let status = self.read32_register(Register::Status);
        DeviceStatus::FeaturesOK.is_set(status)
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
    fn setup_queue(&mut self, queue_idx: usize) -> bool {
        if queue_idx >= self.get_virtqueue_count() {
            return false;
        }
        
        // Select the queue
        self.write32_register(Register::QueueSel, queue_idx as u32);
        
        // Get maximum queue size
        let queue_size = self.read32_register(Register::QueueNumMax);
        if queue_size == 0 {
            return false; // Queue not available
        }
        
        // Set queue size
        self.write32_register(Register::QueueNum, queue_size);
        
        let virtqueue = self.get_virtqueue(queue_idx);

        // Set the queue descriptor address
        let desc_addr = virtqueue.get_raw_ptr() as u64;
        let desc_addr_low = (desc_addr & 0xffffffff) as u32;
        let desc_addr_high = (desc_addr >> 32) as u32;
        self.write32_register(Register::QueueDescLow, desc_addr_low);
        self.write32_register(Register::QueueDescHigh, desc_addr_high);

        // Set the driver area (available ring)  address
        let driver_addr = virtqueue.avail.ring.as_ptr() as u64;
        let driver_addr_low = (driver_addr & 0xffffffff) as u32;
        let driver_addr_high = (driver_addr >> 32) as u32;
        self.write32_register(Register::DriverDescLow, driver_addr_low);
        self.write32_register(Register::DriverDescHigh, driver_addr_high);

        // Set the device area (used ring) address
        let device_addr = virtqueue.used.ring.as_ptr() as u64;
        let device_addr_low = (device_addr & 0xffffffff) as u32;
        let device_addr_high = (device_addr >> 32) as u32;
        self.write32_register(Register::DeviceDescLow, device_addr_low);
        self.write32_register(Register::DeviceDescHigh, device_addr_high);
        
        // Mark queue as ready
        self.write32_register(Register::QueueReady, 1);
        
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
            self.write32_register(Register::InterruptAck, status);
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
    fn notify(&mut self, virtqueue_idx: usize) {
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
    fn get_virtqueue(&self, queue_idx: usize) -> &VirtQueue;
}

#[cfg(test)]
mod tests;