extern crate alloc;

use alloc::sync::Arc;
use alloc::vec::Vec;
use vm_fdt::{Error, FdtWriter};

use super::MachineConfig;
use crate::device::{DeviceFdt, MmioDevice};

pub struct DtbGenerator<'a> {
    config: &'a MachineConfig,
    devices: &'a [Arc<dyn MmioDevice>],
}

impl<'a> DtbGenerator<'a> {
    pub fn new(config: &'a MachineConfig, devices: &'a [Arc<dyn MmioDevice>]) -> Self {
        Self { config, devices }
    }

    pub fn generate(&self) -> Result<Vec<u8>, Error> {
        let mut fdt = FdtWriter::new()?;

        let root = fdt.begin_node("")?;
        fdt.property_string("compatible", &self.config.compatible)?;
        fdt.property_string("model", &self.config.model)?;
        fdt.property_u32("#address-cells", self.config.address_cells)?;
        fdt.property_u32("#size-cells", self.config.size_cells)?;

        self.add_chosen_node(&mut fdt)?;
        self.add_memory_node(&mut fdt)?;
        self.add_cpus_node(&mut fdt)?;
        self.add_soc_node(&mut fdt)?;

        fdt.end_node(root)?;
        fdt.finish()
    }

    fn add_chosen_node(&self, fdt: &mut FdtWriter) -> Result<(), Error> {
        let chosen = fdt.begin_node("chosen")?;
        fdt.property_string("bootargs", &self.config.bootargs)?;
        if let Some(ref stdout) = self.config.stdout_path {
            fdt.property_string("stdout-path", stdout)?;
        }
        fdt.end_node(chosen)?;
        Ok(())
    }

    fn add_memory_node(&self, fdt: &mut FdtWriter) -> Result<(), Error> {
        let mem_name = alloc::format!("memory@{:x}", self.config.memory_base);
        let memory = fdt.begin_node(&mem_name)?;
        fdt.property_string("device_type", "memory")?;
        fdt.property_array_u64("reg", &[self.config.memory_base, self.config.memory_size])?;
        fdt.end_node(memory)?;
        Ok(())
    }

    fn add_cpus_node(&self, fdt: &mut FdtWriter) -> Result<(), Error> {
        let cpus = fdt.begin_node("cpus")?;
        fdt.property_u32("#address-cells", 1)?;
        fdt.property_u32("#size-cells", 0)?;
        fdt.property_u32("timebase-frequency", self.config.timebase_frequency)?;

        for (i, cpu_info) in self.config.cpus.iter().enumerate() {
            let cpu_name = alloc::format!("cpu@{:x}", i);
            let cpu = fdt.begin_node(&cpu_name)?;
            fdt.property_string("device_type", "cpu")?;
            fdt.property_string("compatible", &cpu_info.compatible)?;
            fdt.property_u32("reg", i as u32)?;
            fdt.property_string("riscv,isa", &cpu_info.isa)?;
            fdt.property_string("mmu-type", &cpu_info.mmu_type)?;

            let intc = fdt.begin_node("interrupt-controller")?;
            fdt.property_string("compatible", "riscv,cpu-intc")?;
            fdt.property_u32("#interrupt-cells", 1)?;
            fdt.property_null("interrupt-controller")?;
            fdt.end_node(intc)?;

            fdt.end_node(cpu)?;
        }

        fdt.end_node(cpus)?;
        Ok(())
    }

    fn add_soc_node(&self, fdt: &mut FdtWriter) -> Result<(), Error> {
        let soc = fdt.begin_node("soc")?;
        fdt.property_u32("#address-cells", 2)?;
        fdt.property_u32("#size-cells", 2)?;
        fdt.property_string("compatible", "simple-bus")?;
        fdt.property_array_u32("ranges", &[])?;

        for device in self.devices {
            if let Some(any_ref) = device
                .as_any()
                .downcast_ref::<crate::devices::uart::Ns16550a>()
            {
                if let Some(node_info) =
                    <crate::devices::uart::Ns16550a as DeviceFdt>::fdt_node(any_ref)
                {
                    self.add_device_node(fdt, &node_info)?;
                }
            } else if let Some(any_ref) = device
                .as_any()
                .downcast_ref::<crate::devices::plic::PlicDevice>()
                && let Some(node_info) =
                    <crate::devices::plic::PlicDevice as DeviceFdt>::fdt_node(any_ref)
            {
                self.add_device_node(fdt, &node_info)?;
            }
        }

        fdt.end_node(soc)?;
        Ok(())
    }

    fn add_device_node(
        &self,
        fdt: &mut FdtWriter,
        info: &crate::device::FdtNodeInfo,
    ) -> Result<(), Error> {
        let node = fdt.begin_node(&info.name)?;

        fdt.property_string("compatible", &info.compatible)?;

        if !info.reg.is_empty() {
            let mut reg = alloc::vec::Vec::new();
            for (addr, size) in &info.reg {
                reg.push(*addr);
                reg.push(*size);
            }
            fdt.property_array_u64("reg", &reg)?;
        }

        if !info.interrupts.is_empty() {
            fdt.property_array_u32("interrupts", &info.interrupts)?;
        }

        if let Some(parent) = info.interrupt_parent {
            fdt.property_u32("interrupt-parent", parent)?;
        }

        for (name, value) in &info.extra {
            match value {
                crate::device::FdtValue::String(s) => fdt.property_string(name, s)?,
                crate::device::FdtValue::U32(v) => fdt.property_u32(name, *v)?,
                crate::device::FdtValue::U64(v) => fdt.property_u64(name, *v)?,
                crate::device::FdtValue::ArrayU32(arr) => fdt.property_array_u32(name, arr)?,
                crate::device::FdtValue::ArrayU64(arr) => fdt.property_array_u64(name, arr)?,
                crate::device::FdtValue::Empty => fdt.property_null(name)?,
            }
        }

        fdt.end_node(node)?;
        Ok(())
    }
}
