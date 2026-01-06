//! # Kernel Random Number Generator
//!
//! This module provides a kernel-level random number generation facility.
//! It abstracts away the underlying entropy sources and provides a unified
//! interface for obtaining random numbers throughout the kernel.
//!
//! ## Architecture
//!
//! The RNG subsystem consists of:
//! - A central RNG manager that coordinates entropy sources
//! - Multiple entropy sources (e.g., virtio-rng, hardware RNG, timer jitter)
//! - A buffered output that can be accessed via CharDevice interface
//!
//! ## Usage
//!
//! ```rust
//! use crate::random::RandomManager;
//!
//! // Get random bytes
//! let mut buffer = [0u8; 32];
//! RandomManager::get_random_bytes(&mut buffer);
//! ```

use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;
use spin::{Mutex, Once};

use crate::device::char::CharDevice;
use crate::device::{Device, DeviceType};
use crate::object::capability::{ControlOps, MemoryMappingOps, Selectable};

/// Size of the internal random pool buffer
const RANDOM_POOL_SIZE: usize = 4096;

/// Trait for entropy sources that can provide random data
pub trait EntropySource: Send + Sync {
    /// Get a name for this entropy source
    fn name(&self) -> &'static str;

    /// Read random bytes from this source
    ///
    /// # Arguments
    ///
    /// * `buffer` - Buffer to fill with random data
    ///
    /// # Returns
    ///
    /// Number of bytes actually read
    fn read_entropy(&self, buffer: &mut [u8]) -> usize;

    /// Check if this source is available and ready
    fn is_available(&self) -> bool;
}

/// Random number generator manager
///
/// This is the central component that manages entropy sources and provides
/// random numbers to the rest of the kernel.
pub struct RandomManager {
    /// Registered entropy sources
    sources: Mutex<Vec<Arc<dyn EntropySource>>>,
    /// Internal random pool buffer
    pool: Mutex<VecDeque<u8>>,
}

impl RandomManager {
    /// Create a new RandomManager
    fn new() -> Self {
        Self {
            sources: Mutex::new(Vec::new()),
            pool: Mutex::new(VecDeque::with_capacity(RANDOM_POOL_SIZE)),
        }
    }

    /// Get the global RandomManager instance
    fn instance() -> &'static RandomManager {
        static INSTANCE: Once<RandomManager> = Once::new();
        INSTANCE.call_once(|| RandomManager::new())
    }

    /// Register an entropy source
    ///
    /// # Arguments
    ///
    /// * `source` - The entropy source to register
    pub fn register_entropy_source(source: Arc<dyn EntropySource>) {
        let manager = Self::instance();
        let mut sources = manager.sources.lock();
        crate::early_println!("[Random] Registering entropy source: {}", source.name());
        sources.push(source);
    }

    /// Fill the internal pool with entropy from available sources
    fn fill_pool(&self) -> Result<usize, &'static str> {
        let sources = self.sources.lock();
        
        if sources.is_empty() {
            return Err("No entropy sources available");
        }

        let mut total_bytes = 0;
        let mut temp_buffer = [0u8; 256];

        // Try each source until we get some data
        for source in sources.iter() {
            if !source.is_available() {
                continue;
            }

            let bytes_read = source.read_entropy(&mut temp_buffer);
            if bytes_read > 0 {
                let mut pool = self.pool.lock();
                let available_space = RANDOM_POOL_SIZE.saturating_sub(pool.len());
                let bytes_to_add = bytes_read.min(available_space);
                
                if bytes_to_add < bytes_read {
                    crate::early_println!(
                        "[Random] Pool full, discarding {} entropy bytes",
                        bytes_read - bytes_to_add
                    );
                }
                
                for i in 0..bytes_to_add {
                    pool.push_back(temp_buffer[i]);
                }
                total_bytes += bytes_to_add;
                break; // Got data from one source, that's enough for now
            }
        }

        if total_bytes == 0 {
            Err("Failed to read from any entropy source")
        } else {
            Ok(total_bytes)
        }
    }

    /// Get random bytes from the pool
    ///
    /// # Arguments
    ///
    /// * `buffer` - Buffer to fill with random data
    ///
    /// # Returns
    ///
    /// Number of bytes actually written
    pub fn get_random_bytes(buffer: &mut [u8]) -> usize {
        let manager = Self::instance();
        let mut bytes_read = 0;

        for i in 0..buffer.len() {
            // Try to get byte from pool
            let mut pool = manager.pool.lock();
            if let Some(byte) = pool.pop_front() {
                buffer[i] = byte;
                bytes_read += 1;
            } else {
                // Pool is empty, try to refill while holding the lock
                drop(pool);
                if manager.fill_pool().is_err() {
                    // Can't get more entropy
                    return bytes_read;
                }
                // Try again after refill
                pool = manager.pool.lock();
                if let Some(byte) = pool.pop_front() {
                    buffer[i] = byte;
                    bytes_read += 1;
                } else {
                    // Still empty, give up
                    drop(pool);
                    return bytes_read;
                }
            }
        }

        bytes_read
    }

    /// Read a single random byte
    pub fn get_random_byte() -> Option<u8> {
        let mut buffer = [0u8; 1];
        if Self::get_random_bytes(&mut buffer) == 1 {
            Some(buffer[0])
        } else {
            None
        }
    }
}

/// Character device interface for /dev/random
///
/// This provides the /dev/random device that userspace can read from.
pub struct RandomCharDevice;

impl RandomCharDevice {
    pub fn new() -> Self {
        Self
    }
}

impl Device for RandomCharDevice {
    fn device_type(&self) -> DeviceType {
        DeviceType::Char
    }

    fn name(&self) -> &'static str {
        "random"
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn as_char_device(&self) -> Option<&dyn CharDevice> {
        Some(self)
    }
}

impl CharDevice for RandomCharDevice {
    fn read_byte(&self) -> Option<u8> {
        RandomManager::get_random_byte()
    }

    fn write_byte(&self, _byte: u8) -> Result<(), &'static str> {
        Err("Write not supported for random device")
    }

    fn can_read(&self) -> bool {
        // Check if we have any entropy sources available
        let manager = RandomManager::instance();
        let sources = manager.sources.lock();
        !sources.is_empty() && sources.iter().any(|s| s.is_available())
    }

    fn can_write(&self) -> bool {
        false
    }
}

impl ControlOps for RandomCharDevice {
    fn control(&self, _command: u32, _arg: usize) -> Result<i32, &'static str> {
        Err("Control operations not supported for random device")
    }
}

impl MemoryMappingOps for RandomCharDevice {
    fn get_mapping_info(
        &self,
        _offset: usize,
        _length: usize,
    ) -> Result<(usize, usize, bool), &'static str> {
        Err("Memory mapping not supported for random device")
    }
}

impl Selectable for RandomCharDevice {}
