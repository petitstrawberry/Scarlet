extern crate alloc;

mod dtb;

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;

use crate::device::{IrqSink, MmioDevice};

pub use dtb::DtbGenerator;

pub struct CpuInfo {
    pub compatible: String,
    pub isa: Option<String>,
    pub mmu_type: Option<String>,
    pub enable_method: Option<String>,
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
    pub initrd_base: Option<u64>,
    pub initrd_size: Option<u64>,
}

impl MachineConfig {
    #[cfg(target_arch = "riscv64")]
    pub fn qemu_virt() -> Self {
        Self {
            compatible: String::from("riscv-virtio"),
            model: String::from("Scarlet QEMU Virtual Machine"),
            address_cells: 2,
            size_cells: 2,
            memory_base: 0x80000000,
            memory_size: 256 * 1024 * 1024,
            timebase_frequency: 10000000,
            bootargs: String::from("console=ttyS0"),
            stdout_path: Some(String::from("/soc/serial@10000000")),
            num_vcpus: 1,
            cpus: vec![CpuInfo {
                compatible: String::from("riscv"),
                isa: Some(String::from("rv64imafdc")),
                mmu_type: Some(String::from("riscv,sv48")),
                enable_method: None,
            }],
            initrd_base: None,
            initrd_size: None,
        }
    }

    #[cfg(target_arch = "aarch64")]
    pub fn qemu_virt() -> Self {
        Self::qemu_virt_aarch64()
    }

    pub fn qemu_virt_aarch64() -> Self {
        Self {
            compatible: String::from("arm,virt"),
            model: String::from("Scarlet QEMU Virtual Machine"),
            address_cells: 2,
            size_cells: 2,
            memory_base: 0x4000_0000,
            memory_size: 256 * 1024 * 1024,
            timebase_frequency: 10_000_000,
            bootargs: String::from("console=ttyAMA0"),
            stdout_path: Some(String::from("/soc/serial@9000000")),
            num_vcpus: 1,
            cpus: vec![CpuInfo {
                compatible: String::from("arm,arm-v8"),
                isa: None,
                mmu_type: None,
                enable_method: Some(String::from("psci")),
            }],
            initrd_base: None,
            initrd_size: None,
        }
    }

    pub fn set_initrd(&mut self, base: u64, size: u64) {
        self.initrd_base = Some(base);
        self.initrd_size = Some(size);
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

    pub fn config_mut(&mut self) -> &mut MachineConfig {
        &mut self.config
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
