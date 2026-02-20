extern crate alloc;

mod dtb;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::device::MmioDevice;

pub use dtb::DtbGenerator;

pub struct CpuInfo {
    pub compatible: String,
    pub isa: String,
    pub mmu_type: String,
}

pub struct MachineConfig {
    pub compatible: String,
    pub model: String,
    pub address_cells: u32,
    pub size_cells: u32,
    pub memory_base: u64,
    pub memory_size: u64,
    pub timebase_frequency: u32,
    pub bootargs: String,
    pub stdout_path: Option<String>,
    pub num_vcpus: usize,
    pub cpus: Vec<CpuInfo>,
}

impl MachineConfig {
    pub fn qemu_virt() -> Self {
        Self {
            compatible: String::from("riscv-virtio"),
            model: String::from("Scarlet QEMU Virtual Machine"),
            address_cells: 2,
            size_cells: 2,
            memory_base: 0x80000000,
            memory_size: 128 * 1024 * 1024,
            timebase_frequency: 10000000,
            bootargs: String::from("console=ttyS0"),
            stdout_path: Some(String::from("/soc/serial@10000000")),
            num_vcpus: 1,
            cpus: vec![CpuInfo {
                compatible: String::from("riscv"),
                isa: String::from("rv64imafdc"),
                mmu_type: String::from("riscv,sv48"),
            }],
        }
    }
}

pub struct Machine {
    config: MachineConfig,
    devices: Vec<Box<dyn MmioDevice>>,
}

impl Machine {
    pub fn new(config: MachineConfig) -> Self {
        Self {
            devices: Vec::new(),
            config,
        }
    }

    pub fn register<D: MmioDevice + 'static>(&mut self, device: D) {
        self.devices.push(Box::new(device));
    }

    pub fn devices(&self) -> &[Box<dyn MmioDevice>] {
        &self.devices
    }

    pub fn devices_mut(&mut self) -> &mut [Box<dyn MmioDevice>] {
        &mut self.devices
    }

    pub fn config(&self) -> &MachineConfig {
        &self.config
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
}
