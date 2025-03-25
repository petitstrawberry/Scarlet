//! Virtio device driver interface module.
//! 

/// Register enum for Virtio devices
/// 
/// This enum represents the registers of the Virtio device.
/// Each variant corresponds to a specific register offset.
/// The offsets are defined in the Virtio specification.
/// The register offsets are used to access the device's configuration and status.
#[derive(Debug, Clone, Copy)]
pub enum Register {
    MagicValue = 0x00,
    Version = 0x04,
    DeviceId = 0x08,
    VendorId = 0x0c,
    DeviceFeatures = 0x10,
    DriverFeatures = 0x20,
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
            0x20 => Register::DriverFeatures,
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
/// This trait defines the interface for Virtio devices.
/// It provides methods for initializing the device, accessing registers,
/// and reading/writing data to/from the device.
/// 
pub trait VirtioDevice {
    fn get_base_addr(&self) -> usize;
    
    fn kick(&mut self, virtqueue_idx: usize, desc_idx: usize) {
        unimplemented!()
    }

    fn read32_register(&self, register: Register) -> u32 {
        let addr = self.get_base_addr() + register.offset();
        unsafe { core::ptr::read_volatile(addr as *const u32) }
    }

    fn write32_register(&self, register: Register, value: u32) {
        let addr = self.get_base_addr() + register.offset();
        unsafe { core::ptr::write_volatile(addr as *mut u32, value) }
    }

    fn read64_register(&self, register: Register) -> u64 {
        let addr = self.get_base_addr() + register.offset();
        unsafe { core::ptr::read_volatile(addr as *const u64) }
    }

    fn write64_register(&self, register: Register, value: u64) {
        let addr = self.get_base_addr() + register.offset();
        unsafe { core::ptr::write_volatile(addr as *mut u64, value) }
    }
}

#[cfg(test)]
mod tests {
    use crate::{drivers::virtio::queue::VirtQueue, mem::page::allocate_pages};

    use super::*;

    struct TestVirtioDevice {
        base_addr: usize,
        virtqueues: [VirtQueue<'static>; 2],
    }

    impl TestVirtioDevice {
        fn new(base_addr: usize, queue_size: usize) -> Self {
            Self {
                base_addr,
                virtqueues: [
                    VirtQueue::new(queue_size),
                    VirtQueue::new(queue_size),
                ],
            }
        }
    }

    impl VirtioDevice for TestVirtioDevice {
        fn get_base_addr(&self) -> usize {
            self.base_addr
        }
        
        fn kick(&mut self, virtqueue_idx: usize, desc_idx: usize) {
            if virtqueue_idx >= self.virtqueues.len() {
                panic!("Invalid virtqueue index");
            }
            let vq = &mut self.virtqueues[virtqueue_idx];
            vq.avail.ring[*vq.avail.idx as usize] = desc_idx as u16;
            *vq.avail.idx = vq.avail.idx.wrapping_add(1) % vq.avail.ring.len() as u16;
            vq.last_used_idx = (vq.last_used_idx + 1) % vq.used.ring.len() as usize;
            /* Maybe Fence at this point */
            self.write32_register(Register::QueueSel, virtqueue_idx as u32);
            self.write32_register(Register::QueueNotify, desc_idx as u32);
        }
    }

    #[test_case]
    fn read_write_register() {
        let page = allocate_pages(1);
        let base_addr = page as usize;
        let register = Register::MagicValue;
        let value = 0x12345678;

        let device = TestVirtioDevice::new(base_addr, 2);
        device.write32_register(register, value);

        let read_value = device.read32_register(register);
        assert_eq!(read_value, value);
    }

    #[test_case]
    fn test_device_status() {
        let mut status = 0;
        DeviceStatus::DriverOK.set(&mut status);
        assert!(DeviceStatus::DriverOK.is_set(status));

        DeviceStatus::DriverOK.clear(&mut status);
        assert!(!DeviceStatus::DriverOK.is_set(status));

        DeviceStatus::DriverOK.toggle(&mut status);
        assert!(DeviceStatus::DriverOK.is_set(status));

        DeviceStatus::FeaturesOK.set(&mut status);
        assert!(DeviceStatus::FeaturesOK.is_set(status));
        assert!(DeviceStatus::DriverOK.is_set(status));
    }
}