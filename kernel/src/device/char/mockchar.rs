use core::any::Any;
use alloc::vec::Vec;

use super::{CharDevice, super::{Device, DeviceType}};

/// Mock character device for testing
pub struct MockCharDevice {
    id: usize,
    name: &'static str,
    read_buffer: Vec<u8>,
    write_buffer: Vec<u8>,
    read_index: usize,
}

impl MockCharDevice {
    pub fn new(id: usize, name: &'static str) -> Self {
        Self {
            id,
            name,
            read_buffer: Vec::new(),
            write_buffer: Vec::new(),
            read_index: 0,
        }
    }

    /// Set the data that will be returned by read operations
    pub fn set_read_data(&mut self, data: Vec<u8>) {
        self.read_buffer = data;
        self.read_index = 0;
    }

    /// Get the data that was written to the device
    pub fn get_written_data(&self) -> &Vec<u8> {
        &self.write_buffer
    }

    /// Clear the written data buffer
    pub fn clear_written_data(&mut self) {
        self.write_buffer.clear();
    }

    /// Reset the read index to start reading from the beginning
    pub fn reset_read_index(&mut self) {
        self.read_index = 0;
    }
}

impl Device for MockCharDevice {
    fn device_type(&self) -> DeviceType {
        DeviceType::Char
    }

    fn name(&self) -> &'static str {
        self.name
    }

    fn id(&self) -> usize {
        self.id
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl CharDevice for MockCharDevice {
    fn read_byte(&mut self) -> Option<u8> {
        if self.read_index < self.read_buffer.len() {
            let byte = self.read_buffer[self.read_index];
            self.read_index += 1;
            Some(byte)
        } else {
            None
        }
    }

    fn write_byte(&mut self, byte: u8) -> Result<(), &'static str> {
        self.write_buffer.push(byte);
        Ok(())
    }

    fn can_read(&self) -> bool {
        self.read_index < self.read_buffer.len()
    }

    fn can_write(&self) -> bool {
        true // Mock device can always write
    }
}
