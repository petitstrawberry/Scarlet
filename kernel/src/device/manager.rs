//! Device Manager module.
//! 
//! 

extern crate alloc;
use alloc::vec::Vec;
use alloc::boxed::Box;
use spin::mutex::Mutex;
use spin::MutexGuard;

use crate::early_println;
use crate::early_print;
use crate::traits::serial::Serial;

use super::DeviceInfo;

pub struct BasicDeviceManager {
    /* Basic I/O */
    serials: Vec<Box<dyn Serial>>,
}

impl BasicDeviceManager {
    pub fn new() -> Self {
        BasicDeviceManager {
            serials: Vec::new(),
        }
    }

    pub fn register_serial(&mut self, serial: Box<dyn Serial>) {
        self.serials.push(serial);
        early_println!("Registered serial device");
    }

    pub fn register_serials(&mut self, serias: Vec<Box<dyn Serial>>) {
        let len = serias.len();
        for serial in serias {
            self.serials.push(serial);
        }
        early_println!("Registered serial devices: {}", len);
    }

    pub fn borrow_serial(&self, idx: usize) -> Option<&Box<dyn Serial>> {
        self.serials.get(idx)
    }

    pub fn borrow_mut_serial(&mut self, idx: usize) -> Option<&mut Box<dyn Serial>> {
        self.serials.get_mut(idx)
    }

    pub fn borrow_serials(&self) -> &Vec<Box<dyn Serial>> {
        &self.serials
    }

    pub fn borrow_mut_serials(&mut self) -> &mut Vec<Box<dyn Serial>> {
        &mut self.serials
    }
}

pub struct DeviceManager {
    /* Manager for basic devices */
    pub basic: BasicDeviceManager,
    /* Other devices */
    devices: Vec<Box<dyn DeviceInfo>>,
}

impl DeviceManager {
    const fn new() -> Self {
        DeviceManager {
            basic: BasicDeviceManager {
                serials: Vec::new(),
            },
            devices: Vec::new(),
        }
    }

    #[allow(static_mut_refs)]
    pub fn locked() -> MutexGuard<'static, DeviceManager> {
        static mut MANAGER: Mutex<DeviceManager> = Mutex::new(DeviceManager::new());
        unsafe { MANAGER.lock() }
    }

    pub fn register_device(&mut self, device: Box<dyn DeviceInfo>) {
        self.devices.push(device);
    }

    pub fn borrow_devices(&self) -> &Vec<Box<dyn DeviceInfo>> {
        &self.devices
    }

    pub fn borrow_mut_devices(&mut self) -> &mut Vec<Box<dyn DeviceInfo>> {
        &mut self.devices
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
        let device = Box::new(PlatformDeviceInfo::new("test", 0, &["test,device"]));
        let mut manager = DeviceManager::locked();
        manager.register_device(device);
        let len = manager.devices.len();
        let registered_device = &manager.devices[len -1];
        assert_eq!(registered_device.name(), "test");
    }
}
