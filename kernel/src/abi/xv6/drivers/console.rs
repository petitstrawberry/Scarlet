use core::any::Any;

use crate::{device::{char::CharDevice, manager::DeviceManager, Device, DeviceType}};

/// Character device for xv6 console
pub struct ConsoleDevice {
    id: usize,
    name: &'static str,
}

impl ConsoleDevice {
    pub fn new(id: usize, name: &'static str) -> Self {
        Self {
            id,
            name,
        }
    }
}

impl Device for ConsoleDevice {
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
    
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    
    fn as_char_device(&mut self) -> Option<&mut dyn CharDevice> {
        Some(self)
    }
}

impl CharDevice for ConsoleDevice {
    fn read_byte(&mut self) -> Option<u8> {
        let serial = DeviceManager::get_mut_manager().basic.borrow_mut_serial(0)?;
        let mut c = serial.get();

        while c.is_none() {
            // Wait for input
            // This is a blocking read, in a real implementation you might want to handle this differently
            c = serial.get();
        }
        let c = c.unwrap();
        if c == '\r' {
            serial.put('\n').ok(); // Convert carriage return to newline
        }
        serial.put(c).ok(); // Echo back the character
        Some(c as u8)
    }

    fn write_byte(&mut self, byte: u8) -> Result<(), &'static str> {
        let serial = DeviceManager::get_mut_manager().basic.borrow_mut_serial(0)
            .ok_or("Failed to borrow serial device")?;
        serial.put(byte as char)
            .map_err(|_| "Failed to write byte to console")
    }

    fn can_read(&self) -> bool {
        true
    }

    fn can_write(&self) -> bool {
        true // Mock device can always write
    }
}

