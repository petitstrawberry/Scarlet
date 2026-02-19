extern crate alloc;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::any::Any;

pub trait MmioDevice {
    fn base(&self) -> u64;
    fn size(&self) -> u64;
    fn read(&mut self, offset: u64, size: u8) -> u64;
    fn write(&mut self, offset: u64, size: u8, data: u64);
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

pub struct FdtNodeInfo {
    pub name: String,
    pub compatible: String,
    pub reg: Vec<(u64, u64)>,
    pub interrupts: Vec<u32>,
    pub interrupt_parent: Option<u32>,
    pub extra: Vec<(String, FdtValue)>,
}

pub enum FdtValue {
    String(String),
    U32(u32),
    U64(u64),
    ArrayU32(Vec<u32>),
    ArrayU64(Vec<u64>),
    Empty,
}

pub trait DeviceFdt {
    fn fdt_node(&self) -> Option<FdtNodeInfo>;
}

pub struct DeviceEmulator {
    devices: Vec<Box<dyn MmioDevice>>,
}

impl DeviceEmulator {
    pub fn new() -> Self {
        Self {
            devices: Vec::new(),
        }
    }

    pub fn register<D: MmioDevice + 'static>(&mut self, device: D) {
        self.devices.push(Box::new(device));
    }

    fn find_device(&self, addr: u64) -> Option<(usize, u64)> {
        for (i, dev) in self.devices.iter().enumerate() {
            let base = dev.base();
            let end = base + dev.size();
            if addr >= base && addr < end {
                return Some((i, addr - base));
            }
        }
        None
    }

    pub fn handle_mmio_read(&mut self, addr: u64, size: u8) -> u64 {
        if let Some((idx, offset)) = self.find_device(addr) {
            self.devices[idx].read(offset, size)
        } else {
            0
        }
    }

    pub fn handle_mmio_write(&mut self, addr: u64, size: u8, data: u64) {
        if let Some((idx, offset)) = self.find_device(addr) {
            self.devices[idx].write(offset, size, data);
        }
    }

    pub fn devices(&self) -> &[Box<dyn MmioDevice>] {
        &self.devices
    }
}

impl Default for DeviceEmulator {
    fn default() -> Self {
        Self::new()
    }
}
