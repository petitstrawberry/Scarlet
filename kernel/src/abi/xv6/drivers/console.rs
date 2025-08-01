use core::any::Any;

use crate::{device::{char::CharDevice, manager::DeviceManager, Device, DeviceType}, object::capability::{ControlOps, MemoryMappingOps}};

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

impl ControlOps for ConsoleDevice {
    // Console devices don't support control operations by default
    fn control(&self, _command: u32, _arg: usize) -> Result<i32, &'static str> {
        Err("Control operations not supported")
    }
}

impl MemoryMappingOps for ConsoleDevice {
    fn get_mapping_info(&self, _offset: usize, _length: usize) 
                       -> Result<(usize, usize, bool), &'static str> {
        Err("Memory mapping not supported by console device")
    }
    
    fn on_mapped(&self, _vaddr: usize, _paddr: usize, _length: usize, _offset: usize) {
        // Console devices don't support memory mapping
    }
    
    fn on_unmapped(&self, _vaddr: usize, _length: usize) {
        // Console devices don't support memory mapping
    }
    
    fn supports_mmap(&self) -> bool {
        false
    }
}

