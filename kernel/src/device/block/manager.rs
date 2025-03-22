use alloc::vec::Vec;

use super::BlockDevice;

extern crate alloc;

pub struct BlockDeviceManager {
    devices: Vec<BlockDevice>,
}

static mut INSTANCE: BlockDeviceManager = BlockDeviceManager::new();

impl BlockDeviceManager {

    const fn new() -> Self {
        Self {
            devices: Vec::new(),
        }
    }

    #[allow(static_mut_refs)]
    pub fn get_mut_manager() -> &'static mut Self {
        unsafe { &mut INSTANCE }
    }

    #[allow(static_mut_refs)]
    pub fn get_manager() -> &'static Self {
        unsafe { &INSTANCE }
    }

    pub fn register_device(&mut self, device: BlockDevice) {
        self.devices.push(device);
    }
    pub fn get_device(&self, id: usize) -> Option<&BlockDevice> {
        self.devices.get(id)
    }

    pub fn get_mut_device(&mut self, id: usize) -> Option<&mut BlockDevice> {
        self.devices.get_mut(id)
    }

    pub fn get_devices_count(&self) -> usize {
        self.devices.len()
    }
}