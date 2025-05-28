use core::any::Any;

use alloc::{boxed::Box, vec::Vec};

use super::Device;

extern crate alloc;

/// Character device interface
/// 
/// This trait defines the interface for character devices.
/// It provides methods for querying device information and handling character I/O operations.
pub trait CharDevice: Device {
    /// Read a single byte from the device
    /// 
    /// # Returns
    /// 
    /// The byte read from the device, or None if no data is available
    fn read_byte(&mut self) -> Option<u8>;
    
    /// Write a single byte to the device
    /// 
    /// # Arguments
    /// 
    /// * `byte` - The byte to write to the device
    /// 
    /// # Returns
    /// 
    /// Result indicating success or failure
    fn write_byte(&mut self, byte: u8) -> Result<(), &'static str>;
    
    /// Read multiple bytes from the device
    /// 
    /// # Arguments
    /// 
    /// * `buffer` - The buffer to read data into
    /// 
    /// # Returns
    /// 
    /// The number of bytes actually read
    fn read(&mut self, buffer: &mut [u8]) -> usize {
        let mut bytes_read = 0;
        for i in 0..buffer.len() {
            if let Some(byte) = self.read_byte() {
                buffer[i] = byte;
                bytes_read += 1;
            } else {
                break;
            }
        }
        bytes_read
    }
    
    /// Write multiple bytes to the device
    /// 
    /// # Arguments
    /// 
    /// * `buffer` - The buffer containing data to write
    /// 
    /// # Returns
    /// 
    /// Result containing the number of bytes written or an error
    fn write(&mut self, buffer: &[u8]) -> Result<usize, &'static str> {
        let mut bytes_written = 0;
        for &byte in buffer {
            self.write_byte(byte)?;
            bytes_written += 1;
        }
        Ok(bytes_written)
    }
    
    /// Check if the device is ready for reading
    fn can_read(&self) -> bool;
    
    /// Check if the device is ready for writing
    fn can_write(&self) -> bool;
}

/// A generic implementation of a character device
pub struct GenericCharDevice {
    id: usize,
    device_name: &'static str,
    read_fn: fn() -> Option<u8>,
    write_fn: fn(u8) -> Result<(), &'static str>,
    can_read_fn: fn() -> bool,
    can_write_fn: fn() -> bool,
}

impl GenericCharDevice {
    pub fn new(
        id: usize, 
        device_name: &'static str, 
        read_fn: fn() -> Option<u8>,
        write_fn: fn(u8) -> Result<(), &'static str>,
        can_read_fn: fn() -> bool,
        can_write_fn: fn() -> bool,
    ) -> Self {
        Self { 
            id, 
            device_name, 
            read_fn, 
            write_fn,
            can_read_fn,
            can_write_fn,
        }
    }
}

impl Device for GenericCharDevice {
    fn device_type(&self) -> super::DeviceType {
        super::DeviceType::Char
    }

    fn name(&self) -> &'static str {
        self.device_name
    }

    fn id(&self) -> usize {
        self.id
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
    
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    
    fn as_char_device(&mut self) -> Option<&mut dyn CharDevice> {
        Some(self)
    }
}

impl CharDevice for GenericCharDevice {
    fn read_byte(&mut self) -> Option<u8> {
        (self.read_fn)()
    }

    fn write_byte(&mut self, byte: u8) -> Result<(), &'static str> {
        (self.write_fn)(byte)
    }

    fn can_read(&self) -> bool {
        (self.can_read_fn)()
    }

    fn can_write(&self) -> bool {
        (self.can_write_fn)()
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
pub mod mockchar;
