extern crate alloc;

mod dtb;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use scarlet_std::println;

use crate::device::{IrqSink, MmioDevice};

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

pub struct VcpuIrqSink {
    vcpu_handle: u32,
}

impl VcpuIrqSink {
    pub fn new(vcpu_handle: u32) -> Self {
        Self { vcpu_handle }
    }
}

impl IrqSink for VcpuIrqSink {
    fn set_level(&self, level: bool) {
        use scarlet_std::syscall::{Syscall, syscall3};
        const VCPU_CTL_INJECT_INTERRUPT: u32 = 0x04;
        const VCPU_CTL_CLEAR_INTERRUPT: u32 = 0x05;
        const IRQ_TYPE_EXTERNAL: usize = 2;

        // print!("[VcpuIrqSink] set_level({}) handle={}\n", level, self.vcpu_handle);
        if level {
            let _ = syscall3(
                Syscall::HandleControl,
                self.vcpu_handle as usize,
                VCPU_CTL_INJECT_INTERRUPT as usize,
                IRQ_TYPE_EXTERNAL,
            );
        } else {
            let _ = syscall3(
                Syscall::HandleControl,
                self.vcpu_handle as usize,
                VCPU_CTL_CLEAR_INTERRUPT as usize,
                IRQ_TYPE_EXTERNAL,
            );
        }
    }
}

pub struct Machine {
    config: MachineConfig,
    devices: Vec<Arc<dyn MmioDevice>>,
    vcpu_handle: Option<u32>,
}

// SAFETY: Machine is safe to send/share because:
// - config and vcpu_handle are plain data
// - devices contains Arc<dyn MmioDevice> where MmioDevice: Send + Sync
unsafe impl Send for Machine {}
unsafe impl Sync for Machine {}

impl Machine {
    pub fn new(config: MachineConfig) -> Self {
        Self {
            devices: Vec::new(),
            config,
            vcpu_handle: None,
        }
    }

    pub fn set_vcpu_handle(&mut self, handle: u32) {
        self.vcpu_handle = Some(handle);
    }

    pub fn register<D: MmioDevice + 'static>(&mut self, device: Arc<D>) {
        self.devices.push(device);
    }

    pub fn devices(&self) -> &[Arc<dyn MmioDevice>] {
        &self.devices
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

    pub fn handle_mmio_read(&self, addr: u64, size: u8) -> u64 {
        if let Some((idx, offset)) = self.find_device(addr) {
            self.devices[idx].read(offset, size)
        } else {
            0
        }
    }

    pub fn handle_mmio_write(&self, addr: u64, size: u8, data: u64) {
        if let Some((idx, offset)) = self.find_device(addr) {
            self.devices[idx].write(offset, size, data);
        }
    }
}
