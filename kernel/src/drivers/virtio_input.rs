//! VirtIO Input Device Driver
//!
//! This module implements a VirtIO input device driver that supports
//! keyboards, mice, tablets, and other input devices.
//!
//! The driver integrates with Scarlet's native EventDevice to provide
//! a clean interface for input event handling.

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::ToString;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::mem::size_of;
use spin::Mutex;

use crate::device::input::event_device::EventDevice;
use crate::device::manager::DeviceManager;
use crate::drivers::virtio::device::{DeviceStatus, Register, VirtioDevice};
use crate::drivers::virtio::queue::{DescriptorFlag, VirtQueue};
use crate::early_println;
use crate::environment::PAGE_SIZE;
use crate::mem::page::ContiguousPages;

/// VirtIO Input event structure (matches Linux virtio_input_event)
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct VirtioInputEvent {
    type_: u16,
    code: u16,
    value: i32,
}

impl VirtioInputEvent {
    const fn size() -> usize {
        size_of::<Self>()
    }
}

/// VirtIO Input device configuration
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct VirtioInputConfig {
    select: u8,
    subsel: u8,
    size: u8,
    reserved: [u8; 5],
    data: [u8; 128],
}

/// Config select values
mod config_select {
    pub const VIRTIO_INPUT_CFG_UNSET: u8 = 0x00;
    pub const VIRTIO_INPUT_CFG_ID_NAME: u8 = 0x01;
    pub const VIRTIO_INPUT_CFG_ID_SERIAL: u8 = 0x02;
    pub const VIRTIO_INPUT_CFG_ID_DEVIDS: u8 = 0x03;
    pub const VIRTIO_INPUT_CFG_PROP_BITS: u8 = 0x10;
    pub const VIRTIO_INPUT_CFG_EV_BITS: u8 = 0x11;
    pub const VIRTIO_INPUT_CFG_ABS_INFO: u8 = 0x12;
}

/// VirtIO Input Device
pub struct VirtioInputDevice {
    base_addr: usize,
    eventq: Mutex<VirtQueue<'static>>, // Event queue (device -> driver)
    statusq: Mutex<VirtQueue<'static>>, // Status queue (driver -> device)
    event_device: Arc<EventDevice>,
    initialized: Mutex<bool>,
    event_buffer_alloc: Mutex<Option<ContiguousPages>>, // Single page for all event buffers
    interrupt_id: Mutex<Option<u32>>,
}

impl VirtioInputDevice {
    /// Read a u8 from configuration space
    fn read8_config(&self, offset: usize) -> u8 {
        unsafe {
            let addr = self.base_addr + Register::DeviceConfig as usize + offset;
            core::ptr::read_volatile(addr as *const u8)
        }
    }

    /// Write a u8 to configuration space
    fn write8_config(&self, offset: usize, value: u8) {
        unsafe {
            let addr = self.base_addr + Register::DeviceConfig as usize + offset;
            core::ptr::write_volatile(addr as *mut u8, value);
        }
    }

    /// Read device name from configuration
    fn read_device_name(&self) -> Option<alloc::string::String> {
        use config_select::*;

        // Select name configuration
        self.write8_config(0, VIRTIO_INPUT_CFG_ID_NAME);
        self.write8_config(1, 0); // subsel

        // Read size
        let size = self.read8_config(2);
        if size == 0 || size > 128 {
            return None;
        }

        // Read name from data field (offset 8)
        let mut name_bytes = alloc::vec::Vec::new();
        for i in 0..size {
            let byte = self.read8_config(8 + i as usize);
            if byte == 0 {
                break;
            }
            name_bytes.push(byte);
        }

        alloc::string::String::from_utf8(name_bytes).ok()
    }

    /// Determine device type from name
    fn determine_device_type(name: &str) -> &'static str {
        let name_lower = name.to_lowercase();
        if name_lower.contains("keyboard") || name_lower.contains("kbd") {
            "keyboard"
        } else if name_lower.contains("mouse") {
            "mouse"
        } else if name_lower.contains("tablet") {
            "tablet"
        } else {
            "input"
        }
    }

    /// Create a new VirtIO Input device
    ///
    /// # Arguments
    ///
    /// * `base_addr` - The base address of the device
    ///
    /// # Returns
    ///
    /// A new instance of `VirtioInputDevice`
    pub fn new(base_addr: usize) -> Self {
        // Create a temporary device to read configuration
        let temp_device = Self {
            base_addr,
            eventq: Mutex::new(VirtQueue::new(8)),
            statusq: Mutex::new(VirtQueue::new(8)),
            event_device: Arc::new(EventDevice::new("input")),
            initialized: Mutex::new(false),
            event_buffer_alloc: Mutex::new(None),
            interrupt_id: Mutex::new(None),
        };

        // Read device name from VirtIO config
        let virtio_name = temp_device
            .read_device_name()
            .unwrap_or_else(|| "Unknown Device".to_string());

        // Determine device type
        let device_type = Self::determine_device_type(&virtio_name);

        early_println!(
            "[virtio-input] Device at {:#x}: \"{}\"",
            base_addr,
            virtio_name
        );

        // Create the EventDevice with the device type (it will assign the name)
        let event_device = Arc::new(EventDevice::new(device_type));
        let device_name = event_device.get_name();

        early_println!("[virtio-input] Registered as /dev/{}", device_name);

        let mut device = Self {
            base_addr,
            eventq: Mutex::new(VirtQueue::new(8)),
            statusq: Mutex::new(VirtQueue::new(8)),
            event_device: event_device.clone(),
            initialized: Mutex::new(false),
            event_buffer_alloc: Mutex::new(None),
            interrupt_id: Mutex::new(None),
        };

        // Initialize the VirtIO device
        if let Err(e) = device.init() {
            panic!("[virtio-input] Failed to initialize: {}", e);
        }

        // Register the EventDevice with DeviceManager
        DeviceManager::get_manager()
            .register_device_with_name(device_name.to_string(), event_device);

        early_println!("[virtio-input] Device initialized successfully");

        device
    }

    /// Initialize the VirtIO input device
    fn init(&mut self) -> Result<(), &'static str> {
        // 1. Reset the device
        self.write32_register(Register::Status, 0);

        // 2. Set ACKNOWLEDGE status bit
        let mut status = 0u32;
        DeviceStatus::Acknowledge.set(&mut status);
        self.write32_register(Register::Status, status);

        // 3. Set DRIVER status bit
        DeviceStatus::Driver.set(&mut status);
        self.write32_register(Register::Status, status);

        // 4. Read device features
        self.write32_register(Register::DeviceFeaturesSel, 0);
        let _device_features = self.read32_register(Register::DeviceFeatures);

        // 5. Negotiate features (for now, accept no optional features)
        self.write32_register(Register::DriverFeaturesSel, 0);
        self.write32_register(Register::DriverFeatures, 0);

        // 6. Set FEATURES_OK
        DeviceStatus::FeaturesOK.set(&mut status);
        self.write32_register(Register::Status, status);

        // 7. Re-read status to ensure FEATURES_OK is still set
        let status_readback = self.read32_register(Register::Status);
        if !DeviceStatus::FeaturesOK.is_set(status_readback) {
            return Err("Device rejected features");
        }

        // 8. Setup virtqueues
        self.setup_queues()?;

        // 9. Set DRIVER_OK
        DeviceStatus::DriverOK.set(&mut status);
        self.write32_register(Register::Status, status);

        // 10. Prefill event queue with buffers
        self.prefill_event_queue()?;

        *self.initialized.lock() = true;

        Ok(())
    }

    /// Setup virtqueues
    fn setup_queues(&mut self) -> Result<(), &'static str> {
        // Setup eventq (queue 0)
        self.write32_register(Register::QueueSel, 0);
        let max_queue_size = self.read32_register(Register::QueueNumMax);

        if max_queue_size == 0 {
            return Err("Event queue not available");
        }

        let queue_size = core::cmp::min(max_queue_size, 8);
        self.write32_register(Register::QueueNum, queue_size);

        // Initialize the queue
        let mut eventq = self.eventq.lock();
        eventq.init();

        // Set queue addresses
        let desc_addr = eventq.get_raw_ptr() as u64;
        let driver_addr = eventq.avail.flags as *const _ as u64;
        let device_addr = eventq.used.flags as *const _ as u64;

        self.write32_register(Register::QueueDescLow, desc_addr as u32);
        self.write32_register(Register::QueueDescHigh, (desc_addr >> 32) as u32);
        self.write32_register(Register::DriverDescLow, driver_addr as u32);
        self.write32_register(Register::DriverDescHigh, (driver_addr >> 32) as u32);
        self.write32_register(Register::DeviceDescLow, device_addr as u32);
        self.write32_register(Register::DeviceDescHigh, (device_addr >> 32) as u32);

        self.write32_register(Register::QueueReady, 1);

        drop(eventq);

        // Setup statusq (queue 1) - optional, not used for basic input
        self.write32_register(Register::QueueSel, 1);
        let status_max = self.read32_register(Register::QueueNumMax);
        if status_max > 0 {
            let status_size = core::cmp::min(status_max, 8);
            self.write32_register(Register::QueueNum, status_size);

            let mut statusq = self.statusq.lock();
            statusq.init();

            let desc_addr = statusq.get_raw_ptr() as u64;
            let driver_addr = statusq.avail.flags as *const _ as u64;
            let device_addr = statusq.used.flags as *const _ as u64;

            self.write32_register(Register::QueueDescLow, desc_addr as u32);
            self.write32_register(Register::QueueDescHigh, (desc_addr >> 32) as u32);
            self.write32_register(Register::DriverDescLow, driver_addr as u32);
            self.write32_register(Register::DriverDescHigh, (driver_addr >> 32) as u32);
            self.write32_register(Register::DeviceDescLow, device_addr as u32);
            self.write32_register(Register::DeviceDescHigh, (device_addr >> 32) as u32);

            self.write32_register(Register::QueueReady, 1);
        }

        Ok(())
    }

    /// Prefill event queue with receive buffers
    fn prefill_event_queue(&mut self) -> Result<(), &'static str> {
        let queue_size = 8;
        let mut eventq = self.eventq.lock();

        // Allocate single page from PMM for all event buffers
        let buffer_alloc =
            ContiguousPages::new(1).ok_or("Failed to allocate event buffer page from PMM")?;
        let buffer_base = buffer_alloc.as_ptr() as *mut u8;
        let buffer_phys = buffer_alloc.as_paddr();

        for i in 0..queue_size {
            // Calculate offset within the page for this event
            let offset = i * VirtioInputEvent::size();
            let buffer_ptr = unsafe { buffer_base.add(offset) };

            // Allocate descriptor
            let desc_idx = eventq
                .alloc_desc()
                .ok_or("Failed to allocate event queue descriptor")?;

            // Setup descriptor - device writes events here
            eventq.desc[desc_idx].addr = (buffer_phys + offset) as u64;
            eventq.desc[desc_idx].len = VirtioInputEvent::size() as u32;
            eventq.desc[desc_idx].flags = DescriptorFlag::Write as u16; // Device writes
            eventq.desc[desc_idx].next = 0; // No chaining

            // Add to available ring
            eventq
                .push(desc_idx)
                .map_err(|_| "Failed to push descriptor to event queue")?;
        }

        // Store the allocation for cleanup
        *self.event_buffer_alloc.lock() = Some(buffer_alloc);

        // Notify device that buffers are available
        self.write32_register(Register::QueueNotify, 0);

        Ok(())
    }

    /// Handle input events from the device
    ///
    /// This should be called from the interrupt handler or periodically polled
    pub fn handle_interrupt(&self) {
        // Read and acknowledge interrupt status
        let isr_status = self.read32_register(Register::InterruptStatus);
        if isr_status == 0 {
            return;
        }

        self.write32_register(Register::InterruptAck, isr_status);

        // Process events from the queue
        self.process_events();
    }

    /// Poll for events (for testing without interrupt support)
    ///
    /// This can be called periodically to check for events when
    /// interrupt handling is not yet implemented
    pub fn poll_events(&self) {
        self.process_events();
    }

    /// Process events from the event queue
    fn process_events(&self) {
        let mut eventq = self.eventq.lock();

        while let Some(desc_idx) = eventq.pop() {
            let buffer_addr = eventq.desc[desc_idx].addr;
            let length = eventq.desc[desc_idx].len;

            if length != VirtioInputEvent::size() as u32 {
                early_println!("[virtio-input] Warning: unexpected event size {}", length);
                eventq.free_desc(desc_idx);
                continue;
            }

            // Read the VirtIO event directly from physical address
            // Note: In identity-mapped kernel space, physical == virtual
            let virtio_event =
                unsafe { core::ptr::read_volatile(buffer_addr as *const VirtioInputEvent) };

            // Convert to Scarlet event and push to EventDevice
            self.event_device
                .push_event(virtio_event.type_, virtio_event.code, virtio_event.value);

            // Re-setup the descriptor for reuse
            eventq.desc[desc_idx].len = VirtioInputEvent::size() as u32;
            eventq.desc[desc_idx].flags = DescriptorFlag::Write as u16;

            // Re-add to available ring
            if let Err(e) = eventq.push(desc_idx) {
                early_println!("[virtio-input] Failed to re-add buffer: {:?}", e);
                eventq.free_desc(desc_idx);
            }
        }

        // Notify device that we've added more buffers
        self.write32_register(Register::QueueNotify, 0);
    }

    /// Enable interrupts for this device
    pub fn enable_interrupts(&self, interrupt_id: u32) -> Result<(), &'static str> {
        // Store the interrupt ID
        *self.interrupt_id.lock() = Some(interrupt_id);

        // Check current ISR status and clear any pending interrupts
        let isr = self.read32_register(Register::InterruptStatus);
        if isr != 0 {
            self.write32_register(Register::InterruptAck, isr);
            // Process any pending events
            self.process_events();
        }

        // Enable interrupt in PLIC for CPU 0
        crate::interrupt::InterruptManager::with_manager(|mgr| {
            mgr.enable_external_interrupt(interrupt_id, 0)
        })
        .map_err(|_| "Failed to enable interrupt in PLIC")?;

        Ok(())
    }

    /// Get the EventDevice for this input device
    pub fn get_event_device(&self) -> Arc<EventDevice> {
        self.event_device.clone()
    }
}

// Implement MemoryMappingOps for VirtioInputDevice
impl crate::object::capability::memory_mapping::MemoryMappingOps for VirtioInputDevice {
    fn get_mapping_info(
        &self,
        _offset: usize,
        _length: usize,
    ) -> Result<(usize, usize, bool), &'static str> {
        Err("Memory mapping not supported")
    }
}

// Implement Device for VirtioInputDevice
impl crate::device::Device for VirtioInputDevice {
    fn device_type(&self) -> crate::device::DeviceType {
        crate::device::DeviceType::Char
    }

    fn name(&self) -> &'static str {
        "virtio-input"
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
        self
    }
}

// Implement Selectable for VirtioInputDevice
impl crate::object::capability::selectable::Selectable for VirtioInputDevice {
    fn wait_until_ready(
        &self,
        _interest: crate::object::capability::selectable::ReadyInterest,
        _trapframe: &mut crate::arch::Trapframe,
        _timeout_ticks: Option<u64>,
    ) -> crate::object::capability::selectable::SelectWaitOutcome {
        crate::object::capability::selectable::SelectWaitOutcome::Ready
    }
}

// Implement ControlOps for VirtioInputDevice
impl crate::object::capability::control::ControlOps for VirtioInputDevice {}

impl VirtioDevice for VirtioInputDevice {
    fn get_base_addr(&self) -> usize {
        self.base_addr
    }

    fn get_device_info(&self) -> (u32, u32) {
        let device_id = self.read32_register(Register::DeviceId);
        let vendor_id = self.read32_register(Register::VendorId);
        (device_id, vendor_id)
    }

    fn get_virtqueue_count(&self) -> usize {
        2 // Event queue and status queue
    }

    fn get_virtqueue_size(&self, queue_idx: usize) -> usize {
        match queue_idx {
            0 => self.eventq.lock().get_queue_size(),
            1 => self.statusq.lock().get_queue_size(),
            _ => 0,
        }
    }

    fn get_queue_desc_addr(&self, queue_idx: usize) -> Option<u64> {
        match queue_idx {
            0 => Some(self.eventq.lock().get_raw_ptr() as u64),
            1 => Some(self.statusq.lock().get_raw_ptr() as u64),
            _ => None,
        }
    }

    fn get_queue_driver_addr(&self, queue_idx: usize) -> Option<u64> {
        match queue_idx {
            0 => Some(self.eventq.lock().avail.flags as *const _ as u64),
            1 => Some(self.statusq.lock().avail.flags as *const _ as u64),
            _ => None,
        }
    }

    fn get_queue_device_addr(&self, queue_idx: usize) -> Option<u64> {
        match queue_idx {
            0 => Some(self.eventq.lock().used.flags as *const _ as u64),
            1 => Some(self.statusq.lock().used.flags as *const _ as u64),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::device::input::{event_types::*, key_codes::*};

    use super::*;

    #[test_case]
    fn test_virtio_input_event_size() {
        // VirtioInputEvent should be 8 bytes (2+2+4)
        assert_eq!(VirtioInputEvent::size(), 8);
    }

    #[test_case]
    fn test_event_conversion() {
        let virtio_event = VirtioInputEvent {
            type_: EV_KEY,
            code: KEY_A,
            value: 1,
        };

        assert_eq!(virtio_event.type_, EV_KEY);
        assert_eq!(virtio_event.code, KEY_A);
        assert_eq!(virtio_event.value, 1);
    }
}

// Implement InterruptCapableDevice for VirtioInputDevice
impl crate::device::events::InterruptCapableDevice for VirtioInputDevice {
    fn handle_interrupt(&self) -> crate::interrupt::InterruptResult<()> {
        // Read ISR status to acknowledge interrupt
        let isr_status = self.read32_register(Register::InterruptStatus);
        if isr_status == 0 {
            return Ok(());
        }

        // Acknowledge the interrupt
        self.write32_register(Register::InterruptAck, isr_status);

        // Process pending events
        self.process_events();

        Ok(())
    }

    fn interrupt_id(&self) -> Option<crate::interrupt::InterruptId> {
        None
    }
}
