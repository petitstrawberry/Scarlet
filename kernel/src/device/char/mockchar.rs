use core::any::Any;
use alloc::vec::Vec;
use spin::Mutex;

use super::{CharDevice, super::{Device, DeviceType}};

/// Mock character device for testing
pub struct MockCharDevice {
    name: &'static str,
    read_buffer: Vec<u8>,
    write_buffer: Mutex<Vec<u8>>,
    read_index: Mutex<usize>,
}

impl MockCharDevice {
    pub fn new(name: &'static str) -> Self {
        Self {
            name,
            read_buffer: Vec::new(),
            write_buffer: Mutex::new(Vec::new()),
            read_index: Mutex::new(0),
        }
    }

    /// Set the data that will be returned by read operations
    pub fn set_read_data(&mut self, data: Vec<u8>) {
        self.read_buffer = data;
        *self.read_index.lock() = 0;
    }

    /// Get the data that was written to the device
    pub fn get_written_data(&self) -> Vec<u8> {
        self.write_buffer.lock().clone()
    }

    /// Clear the written data buffer
    pub fn clear_written_data(&self) {
        self.write_buffer.lock().clear();
    }

    /// Reset the read index to start reading from the beginning
    pub fn reset_read_index(&self) {
        *self.read_index.lock() = 0;
    }
}

impl Device for MockCharDevice {
    fn device_type(&self) -> DeviceType {
        DeviceType::Char
    }

    fn name(&self) -> &'static str {
        self.name
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

impl CharDevice for MockCharDevice {
    fn read_byte(&self) -> Option<u8> {
        let mut read_index = self.read_index.lock();
        if *read_index < self.read_buffer.len() {
            let byte = self.read_buffer[*read_index];
            *read_index += 1;
            Some(byte)
        } else {
            None
        }
    }

    fn write_byte(&self, byte: u8) -> Result<(), &'static str> {
        self.write_buffer.lock().push(byte);
        Ok(())
    }

    fn can_read(&self) -> bool {
        *self.read_index.lock() < self.read_buffer.len()
    }

    fn can_write(&self) -> bool {
        true // Mock device can always write
    }
}
