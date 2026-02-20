extern crate alloc;

mod dtb;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::device::MmioDevice;
use crate::devices::plic::{PlicConfig, PlicDevice};
use crate::devices::uart::Ns16550a;

pub use dtb::DtbGenerator;

pub struct DeviceConfig {
    pub device_type: DeviceType,
    pub base: u64,
    pub irq: Option<u32>,
}

pub enum DeviceType {
    Uart,
    Plic {
        num_sources: usize,
        num_contexts: usize,
    },
}

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
    pub devices: Vec<DeviceConfig>,
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
                mmu_type: String::from("riscv,sv39"),
            }],
            devices: vec![
                DeviceConfig {
                    device_type: DeviceType::Uart,
                    base: 0x10000000,
                    irq: Some(10),
                },
                DeviceConfig {
                    device_type: DeviceType::Plic {
                        num_sources: 128,
                        num_contexts: 2,
                    },
                    base: 0x0C000000,
                    irq: None,
                },
            ],
        }
    }
}

pub struct Machine {
    config: MachineConfig,
    devices: Vec<Box<dyn MmioDevice>>,
    irq_map: Vec<(u32, usize)>,
}

impl Machine {
    pub fn new(config: MachineConfig) -> Self {
        Self {
            devices: Vec::new(),
            irq_map: Vec::new(),
            config,
        }
    }

    pub fn build(&mut self) {
        for (idx, dev_config) in self.config.devices.iter().enumerate() {
            match &dev_config.device_type {
                DeviceType::Uart => {
                    let uart = Box::new(Ns16550a::new(dev_config.base));
                    self.devices.push(uart);
                    if let Some(irq) = dev_config.irq {
                        self.irq_map.push((irq, idx));
                    }
                }
                DeviceType::Plic {
                    num_sources,
                    num_contexts,
                } => {
                    let plic_config = PlicConfig {
                        base: dev_config.base,
                        num_sources: *num_sources,
                        num_contexts: *num_contexts,
                        num_priorities: 7,
                    };
                    let plic = Box::new(PlicDevice::new(plic_config));
                    self.devices.push(plic);
                }
            }
        }
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

    pub fn get_plic(&self) -> Option<&PlicDevice> {
        for dev in &self.devices {
            if let Some(plic) = dev.as_any().downcast_ref::<PlicDevice>() {
                return Some(plic);
            }
        }
        None
    }

    pub fn get_plic_mut(&mut self) -> Option<&mut PlicDevice> {
        for dev in &mut self.devices {
            if let Some(plic) = dev.as_any_mut().downcast_mut::<PlicDevice>() {
                return Some(plic);
            }
        }
        None
    }

    pub fn uart_irq(&self) -> Option<u32> {
        for dev_config in &self.config.devices {
            if matches!(dev_config.device_type, DeviceType::Uart) {
                return dev_config.irq;
            }
        }
        None
    }
}
