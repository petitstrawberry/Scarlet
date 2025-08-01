use core::any::Any;

use super::Device;
use crate::object::capability::{ControlOps, MemoryMappingOps};

extern crate alloc;

/// Seek operations for character device positioning
#[derive(Debug, Clone, Copy)]
pub enum SeekFrom {
    /// Seek from the beginning of the device
    Start(u64),
    /// Seek relative to the current position
    Current(i64),
    /// Seek relative to the end of the device
    End(i64),
}

/// Character device interface
/// 
/// This trait defines the interface for character devices.
/// It provides methods for querying device information and handling character I/O operations.
/// Uses internal mutability for thread-safe shared access.
pub trait CharDevice: Device {
    /// Read a single byte from the device
    /// 
    /// For blocking devices (like TTY), this method will block until data is available.
    /// For non-blocking devices, this returns None if no data is available.
    /// 
    /// # Returns
    /// 
    /// The byte read from the device, or None if no data is available
    fn read_byte(&self) -> Option<u8>;
    
    /// Write a single byte to the device
    /// 
    /// # Arguments
    /// 
    /// * `byte` - The byte to write to the device
    /// 
    /// # Returns
    /// 
    /// Result indicating success or failure
    fn write_byte(&self, byte: u8) -> Result<(), &'static str>;
    
    /// Read multiple bytes from the device
    /// 
    /// # Arguments
    /// 
    /// * `buffer` - The buffer to read data into
    /// 
    /// # Returns
    /// 
    /// The number of bytes actually read
    fn read(&self, buffer: &mut [u8]) -> usize {
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
    fn write(&self, buffer: &[u8]) -> Result<usize, &'static str> {
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
    
    /// Read data from a specific position in the device
    /// 
    /// Default implementation falls back to sequential read for stream devices.
    /// Devices that support random access (like framebuffer, memory devices) should override this.
    /// 
    /// # Arguments
    /// 
    /// * `position` - Byte offset to read from
    /// * `buffer` - Buffer to read data into
    /// 
    /// # Returns
    /// 
    /// Result containing the number of bytes read or an error
    fn read_at(&self, _position: u64, buffer: &mut [u8]) -> Result<usize, &'static str> {
        // Default: use sequential read for stream devices
        Ok(self.read(buffer))
    }
    
    /// Write data to a specific position in the device
    /// 
    /// Default implementation falls back to sequential write for stream devices.
    /// Devices that support random access (like framebuffer, memory devices) should override this.
    /// 
    /// # Arguments
    /// 
    /// * `position` - Byte offset to write to
    /// * `buffer` - Buffer containing data to write
    /// 
    /// # Returns
    /// 
    /// Result containing the number of bytes written or an error
    fn write_at(&self, _position: u64, buffer: &[u8]) -> Result<usize, &'static str> {
        // Default: use sequential write for stream devices
        self.write(buffer)
    }
    
    /// Check if this device supports seek operations
    /// 
    /// Default implementation returns false for stream devices.
    /// Devices that support seeking should override this.
    /// 
    /// # Returns
    /// 
    /// True if the device supports seek operations
    fn can_seek(&self) -> bool {
        false
    }
}

/// A generic implementation of a character device
pub struct GenericCharDevice {
    device_name: &'static str,
    read_fn: fn() -> Option<u8>,
    write_fn: fn(u8) -> Result<(), &'static str>,
    can_read_fn: fn() -> bool,
    can_write_fn: fn() -> bool,
}

impl GenericCharDevice {
    pub fn new(
        device_name: &'static str, 
        read_fn: fn() -> Option<u8>,
        write_fn: fn(u8) -> Result<(), &'static str>,
        can_read_fn: fn() -> bool,
        can_write_fn: fn() -> bool,
    ) -> Self {
        Self { 
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

impl CharDevice for GenericCharDevice {
    fn read_byte(&self) -> Option<u8> {
        (self.read_fn)()
    }

    fn write_byte(&self, byte: u8) -> Result<(), &'static str> {
        (self.write_fn)(byte)
    }

    fn can_read(&self) -> bool {
        (self.can_read_fn)()
    }

    fn can_write(&self) -> bool {
        (self.can_write_fn)()
    }
}

impl ControlOps for GenericCharDevice {
    // Generic character devices don't support control operations by default
    fn control(&self, _command: u32, _arg: usize) -> Result<i32, &'static str> {
        Err("Control operations not supported")
    }
}

impl MemoryMappingOps for GenericCharDevice {
    fn get_mapping_info(&self, _offset: usize, _length: usize) 
                       -> Result<(usize, usize, bool), &'static str> {
        Err("Memory mapping not supported by this character device")
    }
    
    fn on_mapped(&self, _vaddr: usize, _paddr: usize, _length: usize, _offset: usize) {
        // Generic character devices don't support memory mapping
    }
    
    fn on_unmapped(&self, _vaddr: usize, _length: usize) {
        // Generic character devices don't support memory mapping
    }
    
    fn supports_mmap(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
pub mod mockchar;

pub mod tty;
