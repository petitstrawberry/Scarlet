use core::any::Any;

use crate::{device::{char::CharDevice, manager::DeviceManager, Device, DeviceType}};

/// Character device for xv6 console that bridges to TTY
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

impl CharDevice for ConsoleDevice {
    fn read_byte(&self) -> Option<u8> {
        // Bridge to TTY device instead of direct serial access
        let device_manager = DeviceManager::get_manager();
        if let Some(tty_device) = device_manager.get_device_by_name("tty0") {
            return tty_device.as_char_device().and_then(|char_device| char_device.read_byte());
        }
        
        // Fallback: return None if TTY is not available
        None
    }

    fn write_byte(&self, byte: u8) -> Result<(), &'static str> {
        // Bridge to TTY device instead of direct serial access
        let device_manager = DeviceManager::get_manager();
        if let Some(tty_device) = device_manager.get_device_by_name("tty0") {
            if let Some(char_device) = tty_device.as_char_device() {
                return char_device.write_byte(byte);
            }
        }
        
        Err("TTY device not available")
    }

    fn can_read(&self) -> bool {
        // Check TTY availability and read capability
        let device_manager = DeviceManager::get_manager();
        if let Some(tty_device) = device_manager.get_device_by_name("tty0") {
            if let Some(char_device) = tty_device.as_char_device() {
                return char_device.can_read();
            }
        }
        false
    }

    fn can_write(&self) -> bool {
        // Check TTY availability and write capability
        let device_manager = DeviceManager::get_manager();
        if let Some(tty_device) = device_manager.get_device_by_name("tty0") {
            if let Some(char_device) = tty_device.as_char_device() {
                return char_device.can_write();
            }
        }
        false
    }
}

