//! Device Manager module.
//! 
//! 

extern crate alloc;
use alloc::vec::Vec;
use alloc::boxed::Box;
use spin::mutex::Mutex;
use spin::MutexGuard;

use crate::traits::serial::Serial;

use super::Device;

pub struct BasicDeviceManager {
    /* Basic I/O */
    pub serial: Vec<Box<dyn Serial>>,
}

impl BasicDeviceManager {
    pub fn new() -> Self {
        BasicDeviceManager {
            serial: Vec::new(),
        }
    }

    pub fn register_serial(&mut self, serial: Box<dyn Serial>) {
        self.serial.push(serial);
    }
}

pub struct DeviceManager {
    /* Manager for basic devices */
    pub basic: BasicDeviceManager,
    /* Other devices */
    pub devices: Vec<Box<dyn Device>>,
}

impl DeviceManager {
    const fn new() -> Self {
        DeviceManager {
            basic: BasicDeviceManager {
                serial: Vec::new(),
            },
            devices: Vec::new(),
        }
    }

    #[allow(static_mut_refs)]
    pub fn locked() -> MutexGuard<'static, DeviceManager> {
        static mut MANAGER: Mutex<DeviceManager> = Mutex::new(DeviceManager::new());
        unsafe { MANAGER.lock() }
    }

    pub fn register_device(&mut self, device: Box<dyn Device>) {
        self.devices.push(device);
    }
}

pub fn register_serial(serial: Box<dyn Serial>) {
    let mut manager = DeviceManager::locked();
    manager.basic.register_serial(serial);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::platform::*;

    #[test_case]
    fn test_device_manager() {
        let device = Box::new(PlatformDevice::new("test", 0));
        let mut manager = DeviceManager::locked();
        manager.register_device(device);
        let len = manager.devices.len();
        let registered_device = &manager.devices[len -1];
        assert_eq!(registered_device.name(), "test");
    }
}
