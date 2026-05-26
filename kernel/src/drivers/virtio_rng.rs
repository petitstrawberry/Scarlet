//! # VirtIO RNG (Random Number Generator) Device Driver
//!
//! This module provides a driver for VirtIO entropy source devices (virtio-rng),
//! acting as an entropy source for the kernel's random number generation subsystem.
//!
//! The driver integrates with the kernel's RNG manager to provide cryptographically
//! secure random numbers from the host's entropy source.
//!
//! ## Features
//!
//! - VirtIO RNG device driver with virtqueue management
//! - Entropy source integration with kernel RNG subsystem
//! - Internal buffering for efficient random number requests
//!
//! ## Implementation Details
//!
//! The driver uses a single virtqueue (requestq) for receiving random data
//! from the host. Random bytes are fetched in batches and buffered internally
//! to minimize virtqueue operations when requests are made.

use crate::vm::addr::virt_to_phys;
use alloc::{boxed::Box, collections::VecDeque, vec};
use spin::{Mutex, RwLock};

use crate::drivers::virtio::{
    device::VirtioDevice,
    queue::{DescriptorFlag, VirtQueue},
};
use crate::environment::PAGE_SIZE;
use crate::mem::page::ContiguousPages;
use crate::random::EntropySource;

// Default buffer size for random data
const RNG_BUFFER_SIZE: usize = 256;

/// VirtIO RNG Device
///
/// This device provides access to a hardware random number generator through
/// the VirtIO interface, acting as an entropy source for the kernel RNG.
pub struct VirtioRngDevice {
    /// Base memory address for MMIO access
    base_addr: usize,
    /// VirtIO queue for random number requests
    virtqueues: Mutex<[VirtQueue<'static>; 1]>,
    /// Internal buffer for random data
    buffer: Mutex<VecDeque<u8>>,
    /// Negotiated features
    features: RwLock<u64>,
    /// Device initialization status
    initialized: RwLock<bool>,
}

impl VirtioRngDevice {
    /// Create a new VirtIO RNG device
    ///
    /// # Arguments
    ///
    /// * `base_addr` - Base memory address for the device's MMIO region
    ///
    /// # Returns
    ///
    /// A new VirtioRngDevice instance
    pub fn new(base_addr: usize) -> Self {
        let mut device = Self {
            base_addr,
            virtqueues: Mutex::new([VirtQueue::new(8)]), // Small queue is sufficient for RNG
            buffer: Mutex::new(VecDeque::with_capacity(RNG_BUFFER_SIZE)),
            features: RwLock::new(0),
            initialized: RwLock::new(false),
        };

        // Initialize the device
        let negotiated_features = match device.init() {
            Ok(features) => {
                *device.initialized.write() = true;
                features
            }
            Err(e) => {
                crate::println!("[VirtIO RNG] Failed to initialize: {}", e);
                0
            }
        };

        // Store negotiated features
        *device.features.write() = negotiated_features;

        crate::println!(
            "[VirtIO RNG] Device initialized with features: 0x{:x}",
            negotiated_features
        );

        device
    }

    /// Fill the internal buffer with random data from the device
    ///
    /// This method requests random data from the VirtIO RNG device and stores it
    /// in the internal buffer for later reads.
    ///
    /// # Returns
    ///
    /// The number of bytes added to the buffer, or an error message
    fn fill_buffer(&self) -> Result<usize, &'static str> {
        let mut virtqueues = self.virtqueues.lock();
        let queue = &mut virtqueues[0];

        // Allocate buffer from PMM for DMA
        let buffer_alloc =
            ContiguousPages::new(1).ok_or("Failed to allocate RNG buffer from PMM")?;
        let data_ptr = buffer_alloc.as_ptr() as *mut u8;

        // Allocate descriptor for the data buffer (device writable)
        let desc_idx = queue.alloc_desc().ok_or("No available descriptors")?;

        // Set up the descriptor
        queue.desc[desc_idx].addr = buffer_alloc.as_paddr() as u64;
        queue.desc[desc_idx].len = RNG_BUFFER_SIZE as u32;
        queue.desc[desc_idx].flags = DescriptorFlag::Write as u16;
        queue.desc[desc_idx].next = 0;

        // Add descriptor to available ring
        if let Err(e) = queue.push(desc_idx) {
            queue.free_desc(desc_idx);
            return Err(e);
        }

        // Notify device
        self.notify(0);

        // Wait for device to process the request (polling)
        while queue.is_busy() {
            core::hint::spin_loop();
        }

        // Process completed request
        let completed_desc = match queue.pop() {
            Some(idx) => idx,
            None => {
                queue.free_desc(desc_idx);
                return Err("No response from device");
            }
        };

        if completed_desc != desc_idx {
            queue.free_desc(desc_idx);
            return Err("Invalid descriptor index");
        }

        // Copy data to internal buffer
        let bytes_received = queue.desc[desc_idx].len as usize;
        let mut buffer = self.buffer.lock();
        unsafe {
            for i in 0..bytes_received.min(RNG_BUFFER_SIZE) {
                buffer.push_back(*data_ptr.add(i));
            }
        }

        // Free the descriptor
        queue.free_desc(desc_idx);
        // buffer_alloc is automatically dropped here

        Ok(bytes_received)
    }

    /// Read a byte from the internal buffer, filling it if necessary
    ///
    /// # Returns
    ///
    /// A byte from the buffer, or None if unable to get random data
    fn read_byte_internal(&self) -> Option<u8> {
        let mut buffer = self.buffer.lock();

        // If buffer is empty, try to fill it
        if buffer.is_empty() {
            drop(buffer); // Release lock before filling
            if let Err(e) = self.fill_buffer() {
                crate::println!("[VirtIO RNG] Failed to fill buffer: {}", e);
                return None;
            }
            buffer = self.buffer.lock();
        }

        // Pop a byte from the buffer
        if !buffer.is_empty() {
            buffer.pop_front()
        } else {
            None
        }
    }
}

impl EntropySource for VirtioRngDevice {
    fn name(&self) -> &'static str {
        "virtio-rng"
    }

    fn read_entropy(&self, buffer: &mut [u8]) -> usize {
        let mut bytes_read = 0;

        for i in 0..buffer.len() {
            if let Some(byte) = self.read_byte_internal() {
                buffer[i] = byte;
                bytes_read += 1;
            } else {
                break;
            }
        }

        bytes_read
    }

    fn is_available(&self) -> bool {
        // Check if the device is properly initialized
        *self.initialized.read()
    }
}

impl VirtioDevice for VirtioRngDevice {
    fn get_base_addr(&self) -> usize {
        self.base_addr
    }

    fn get_virtqueue_count(&self) -> usize {
        1 // RNG device has only one queue (requestq)
    }

    fn get_virtqueue_size(&self, queue_idx: usize) -> usize {
        if queue_idx >= 1 {
            panic!("Invalid queue index for VirtIO RNG device: {}", queue_idx);
        }
        let virtqueues = self.virtqueues.lock();
        virtqueues[queue_idx].get_queue_size()
    }

    fn get_queue_desc_addr(&self, queue_idx: usize) -> Option<u64> {
        if queue_idx >= self.get_virtqueue_count() {
            return None;
        }
        let virtqueues = self.virtqueues.lock();
        Some(virt_to_phys(virtqueues[queue_idx].get_raw_ptr() as usize) as u64)
    }

    fn get_queue_driver_addr(&self, queue_idx: usize) -> Option<u64> {
        if queue_idx >= self.get_virtqueue_count() {
            return None;
        }
        let virtqueues = self.virtqueues.lock();
        Some(virt_to_phys(virtqueues[queue_idx].avail.flags as *const _ as usize) as u64)
    }

    fn get_queue_device_addr(&self, queue_idx: usize) -> Option<u64> {
        if queue_idx >= self.get_virtqueue_count() {
            return None;
        }
        let virtqueues = self.virtqueues.lock();
        Some(virt_to_phys(virtqueues[queue_idx].used.flags as *const _ as usize) as u64)
    }

    fn get_supported_features(&self, _device_features: u64) -> u64 {
        // VirtIO RNG doesn't have device-specific features in the base spec
        // Return 0 to indicate no additional features are requested
        0
    }
}
