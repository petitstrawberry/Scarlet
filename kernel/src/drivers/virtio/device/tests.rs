use crate::{drivers::virtio::queue::VirtQueue, mem::page::allocate_raw_pages};
use core::cell::UnsafeCell;

use super::*;

struct TestVirtioDevice {
    base_addr: usize,
    virtqueues: [UnsafeCell<VirtQueue<'static>>; 2],
}

impl TestVirtioDevice {
    fn new(base_addr: usize, queue_size: usize) -> Self {
        Self {
            base_addr,
            virtqueues: [
                UnsafeCell::new(VirtQueue::new(queue_size)),
                UnsafeCell::new(VirtQueue::new(queue_size)),
            ],
        }
    }
}

impl VirtioDevice for TestVirtioDevice {
    fn get_base_addr(&self) -> usize {
        self.base_addr
    }
    
    fn get_virtqueue_count(&self) -> usize {
        self.virtqueues.len()
    }

    fn get_virtqueue_size(&self, queue_idx: usize) -> usize {
        if queue_idx < self.virtqueues.len() {
            unsafe { (*self.virtqueues[queue_idx].get()).get_queue_size() }
        } else {
            0
        }
    }

    fn get_queue_desc_addr(&self, queue_idx: usize) -> Option<u64> {
        if queue_idx < self.virtqueues.len() {
            Some(self.base_addr as u64 + (queue_idx * 0x1000) as u64) // Example offset
        } else {
            None
        }
    }
    
    fn get_queue_device_addr(&self, queue_idx: usize) -> Option<u64> {
        if queue_idx < self.virtqueues.len() {
            Some(self.base_addr as u64 + (queue_idx * 0x1000 + 0x80) as u64) // Example offset
        } else {
            None
        }
    }

    fn get_queue_driver_addr(&self, queue_idx: usize) -> Option<u64> {
        if queue_idx < self.virtqueues.len() {
            Some(self.base_addr as u64 + (queue_idx * 0x1000 + 0x100) as u64) // Example offset
        } else {
            None
        }
    }
}

#[test_case]
fn read_write_register() {
    let page = allocate_raw_pages(1);
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

#[test_case]
fn test_device_initialization() {
    let page = allocate_raw_pages(1);
    let base_addr = page as usize;
    let mut device = TestVirtioDevice::new(base_addr, 2);
    
    // Set the magic value
    device.write32_register(Register::MagicValue, 0x74726976); // "virt" in little-endian
    
    // Test the behavior of the Status register
    device.reset();
    assert_eq!(device.read32_register(Register::Status), 0);
    
    device.acknowledge();
    assert!(DeviceStatus::Acknowledge.is_set(device.read32_register(Register::Status)));
    
    device.driver();
    assert!(DeviceStatus::Driver.is_set(device.read32_register(Register::Status)));
    
    device.driver_ok();
    assert!(DeviceStatus::DriverOK.is_set(device.read32_register(Register::Status)));
}

#[test_case]
fn test_feature_negotiation() {
    let page = allocate_raw_pages(1);
    let base_addr = page as usize;
    let mut device = TestVirtioDevice::new(base_addr, 2);
    
    // Set device features
    let device_features = 0x12345678;
    device.write32_register(Register::DeviceFeatures, device_features);
    
    // Perform negotiation
    let negotiated = device.negotiate_features().unwrap();
    assert_eq!(negotiated, device_features);
    
    // Verify that the FeaturesOK status bit is set
    let status = device.read32_register(Register::Status);
    assert!(DeviceStatus::FeaturesOK.is_set(status));
}

#[test_case]
fn test_queue_setup() {
    let page = allocate_raw_pages(1);
    let base_addr = page as usize;
    let mut device = TestVirtioDevice::new(base_addr, 2);
    
    // Set the QueueNumMax register
    device.write32_register(Register::QueueNumMax, 16);
    
    // Set up the queue
    assert!(device.setup_queue(0, 16));
    
    // Verify that the queue is correctly configured
    let queue_num = device.read32_register(Register::QueueNum);
    assert_eq!(queue_num, 16);
    
    let queue_ready = device.read32_register(Register::QueueReady);
    assert_eq!(queue_ready, 1);
}

#[test_case]
fn test_config_read_write() {
    let page = allocate_raw_pages(1);
    let base_addr = page as usize;
    let device = TestVirtioDevice::new(base_addr, 2);
    
    // Write a value to the device configuration space
    let test_value: u32 = 0xDEADBEEF;
    device.write_config(0, test_value);
    
    // Read and verify
    let read_value: u32 = device.read_config(0);
    assert_eq!(read_value, test_value);
    
    // Test with another type
    let test_value2: u16 = 0xABCD;
    device.write_config(4, test_value2);
    let read_value2: u16 = device.read_config(4);
    assert_eq!(read_value2, test_value2);
}

#[test_case]
fn test_interrupt_handling() {
    let page = allocate_raw_pages(1);
    let base_addr = page as usize;
    let mut device = TestVirtioDevice::new(base_addr, 2);
    
    // Set the interrupt status
    device.write32_register(Register::InterruptStatus, 0x3); // Both bits set
    
    // Verify the interrupt status
    let status = device.get_interrupt_status();
    assert_eq!(status, 0x3);
    
    // Process the interrupt
    let processed = device.process_interrupts();
    assert_eq!(processed, 0x3);

    // Simulate acknowledging the interrupt
    let current_status = device.read32_register(Register::InterruptStatus);
    let new_status = current_status & !0x3;  // Clear the bits being acknowledged
    unsafe { core::ptr::write_volatile((device.get_base_addr() + Register::InterruptStatus.offset()) as *mut u32, new_status) };
    
    // Verify that the interrupt is cleared
    let status_after = device.get_interrupt_status();
    assert_eq!(status_after, 0); // All interrupts acknowledged
}
