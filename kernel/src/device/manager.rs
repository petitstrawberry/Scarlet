//! Device Manager module.
//! 
//! 

extern crate alloc;

use alloc::vec::Vec;
use alloc::boxed::Box;
use spin::mutex::Mutex;

use crate::device::platform::PlatformDeviceInfo;
use crate::println;
use crate::print;

use crate::traits::serial::Serial;

use super::fdt::FdtManager;
use super::DeviceDriver;
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
        println!("Registered serial device");
    }

    pub fn register_serials(&mut self, serias: Vec<Box<dyn Serial>>) {
        let len = serias.len();
        for serial in serias {
            self.serials.push(serial);
        }
        println!("Registered serial devices: {}", len);
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

static mut MANAGER: DeviceManager = DeviceManager::new();
pub struct DeviceManager {
    /* Manager for basic devices */
    pub basic: BasicDeviceManager,
    /* Other devices */
    devices: Mutex<Vec<Box<dyn DeviceInfo>>>,
    /* Device drivers */
    drivers: Mutex<Vec<Box<dyn DeviceDriver>>>,
}

impl DeviceManager {
    const fn new() -> Self {
        DeviceManager {
            basic: BasicDeviceManager {
                serials: Vec::new(),
            },
            devices: Mutex::new(Vec::new()),
            drivers: Mutex::new(Vec::new()),
        }
    }

    #[allow(static_mut_refs)]
    pub fn get_manager() -> &'static DeviceManager {
        unsafe { &MANAGER }
    }

    #[allow(static_mut_refs)]
    pub fn get_mut_manager() -> &'static mut DeviceManager {
        unsafe { &mut MANAGER }
    }

    pub fn register_device(&mut self, device: Box<dyn DeviceInfo>) {
        self.devices.lock().push(device);
    }

    pub fn borrow_devices(&self) -> &Mutex<Vec<Box<dyn DeviceInfo>>> {
        &self.devices
    }

    pub fn borrow_mut_devices(&mut self) -> &mut Mutex<Vec<Box<dyn DeviceInfo>>> {
        &mut self.devices
    }
    pub fn borrow_drivers(&self) -> &Mutex<Vec<Box<dyn DeviceDriver>>> {
        &self.drivers
    }
    
    pub fn borrow_mut_drivers(&mut self) -> &mut Mutex<Vec<Box<dyn DeviceDriver>>> {
        &mut self.drivers
    }

    pub fn populate_devices(&mut self) {
        let fdt_manager = FdtManager::get_mut_manager();
        let fdt = fdt_manager.get_fdt();
        if fdt.is_none() {
            println!("FDT not initialized");
            return;
        }
        let fdt = fdt.unwrap();
        println!("Populating devices from FDT...");

        let soc = fdt.find_node("/soc");
        if soc.is_none() {
            println!("No /soc node found");
            return;
        }

        let soc = soc.unwrap();
        let mut idx = 0;
        for child in soc.children() {
            // println!("Found child node: {}", child.name);
            let compatible = child.compatible();
            if compatible.is_none() {
                continue;
            }
            let compatible = compatible.unwrap().all().collect::<Vec<_>>();

            for driver in self.drivers.lock().iter() {
                if driver.match_table().iter().any(|&c| compatible.contains(&c)) {
                    println!("Found matching driver for {}", driver.name());
                    let device = Box::new(PlatformDeviceInfo::new(
                        child.name,
                        idx,
                        compatible.clone(),
                    ));
                    if let Err(e) = driver.probe(&*device) {
                        println!("Failed to probe device {}: {}", device.name(), e);
                    } else {
                        idx += 1;
                    }
                }
            }
        }
    }

    pub fn register_driver(&mut self, driver: Box<dyn DeviceDriver>) {
        self.drivers.lock().push(driver);
    }
}

pub fn register_serial(serial: Box<dyn Serial>) {
    let manager = DeviceManager::get_mut_manager();
    manager.basic.register_serial(serial);
}

#[cfg(test)]
mod tests {
    use alloc::vec;
    use super::*;
    use crate::device::platform::*;

    #[test_case]
    fn test_device_manager() {
        let device = Box::new(PlatformDeviceInfo::new("test", 0, vec!["test,device"]));
        let manager = DeviceManager::get_mut_manager();
        manager.register_device(device);
        let len = manager.devices.lock().len();
        let registered_device = &manager.devices.lock()[len -1];
        assert_eq!(registered_device.name(), "test");
    }

    #[test_case]
    fn test_populate_driver() {
        static mut TEST_RESULT: bool = false;
        fn probe_fn(_device: &dyn DeviceInfo) -> Result<(), &'static str> {      
            unsafe {
                TEST_RESULT = true;
            }  
            Ok(())
        }

        let driver = Box::new(PlatformDeviceDriver::new(
            "test",
            Vec::new(),
            probe_fn,
            |_device| Ok(()),
            vec!["sifive,test0"]
        ));
        DeviceManager::get_mut_manager().register_driver(driver);

        DeviceManager::get_mut_manager().populate_devices();
        let result = unsafe { TEST_RESULT };
        assert_eq!(result, true);
    }
}
