//! Temporary CPU topology information character device.
//!
//! Exposes scheduler-visible CPU class and capacity through `/dev/cpuinfo`.
//! This is a diagnostic interface, not a stable ABI. It should be replaced by
//! a proper kernel introspection interface once the scheduler API settles.

extern crate alloc;

use alloc::{string::String, sync::Arc};
use core::{any::Any, fmt::Write};

use crate::{
    device::{Device, DeviceType, char::CharDevice, manager::DeviceManager},
    driver_initcall,
    environment::MAX_NUM_CPUS,
    object::capability::{
        ControlOps, MemoryMappingOps,
        selectable::{SelectWaitOutcome, Selectable},
    },
    sched::scheduler::{cpu_topology, is_cpu_online},
    task::SCHED_UTIL_SCALE,
};

/// Read-only CPU topology character device.
pub struct CpuInfoDevice;

impl CpuInfoDevice {
    /// Create a CPU information device.
    ///
    /// # Returns
    ///
    /// A new CPU information character device.
    pub fn new() -> Self {
        Self
    }

    fn render() -> String {
        let mut output = String::new();

        for cpu_id in 0..MAX_NUM_CPUS {
            if !is_cpu_online(cpu_id) {
                continue;
            }

            let Some(topology) = cpu_topology(cpu_id) else {
                continue;
            };

            let _ = writeln!(output, "processor\t: {}", topology.cpu_id);
            let _ = writeln!(output, "online\t\t: yes");
            let _ = writeln!(output, "core class\t: {}", topology.core_class.as_str());
            let _ = writeln!(output, "cpu capacity\t: {}", topology.capacity);
            let _ = writeln!(output, "util scale\t: {}", SCHED_UTIL_SCALE);
            let _ = writeln!(output);
        }

        output
    }
}

impl Device for CpuInfoDevice {
    fn device_type(&self) -> DeviceType {
        DeviceType::Char
    }

    fn name(&self) -> &'static str {
        "cpuinfo"
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn as_char_device(&self) -> Option<&dyn CharDevice> {
        Some(self)
    }
}

impl CharDevice for CpuInfoDevice {
    fn read_byte(&self) -> Option<u8> {
        let mut byte = [0u8; 1];
        match self.read_at(0, &mut byte) {
            Ok(1) => Some(byte[0]),
            _ => None,
        }
    }

    fn read_at(&self, position: u64, buffer: &mut [u8]) -> Result<usize, &'static str> {
        let content = Self::render();
        let bytes = content.as_bytes();
        let position = usize::try_from(position).map_err(|_| "Read position out of range")?;

        if position >= bytes.len() || buffer.is_empty() {
            return Ok(0);
        }

        let bytes_to_read = core::cmp::min(buffer.len(), bytes.len() - position);
        buffer[..bytes_to_read].copy_from_slice(&bytes[position..position + bytes_to_read]);
        Ok(bytes_to_read)
    }

    fn write_byte(&self, _byte: u8) -> Result<(), &'static str> {
        Err("Write not supported for cpuinfo device")
    }

    fn write(&self, _buffer: &[u8]) -> Result<usize, &'static str> {
        Err("Write not supported for cpuinfo device")
    }

    fn can_read(&self) -> bool {
        true
    }

    fn can_write(&self) -> bool {
        false
    }

    fn can_seek(&self) -> bool {
        true
    }
}

impl ControlOps for CpuInfoDevice {
    fn control(&self, _command: u32, _arg: usize) -> Result<i32, &'static str> {
        Err("Control operations not supported for cpuinfo device")
    }
}

impl MemoryMappingOps for CpuInfoDevice {
    fn get_mapping_info(
        &self,
        _offset: usize,
        _length: usize,
    ) -> Result<crate::object::capability::MemoryMappingInfo, &'static str> {
        Err("Memory mapping not supported for cpuinfo device")
    }

    fn supports_mmap(&self) -> bool {
        false
    }
}

impl Selectable for CpuInfoDevice {
    fn wait_until_ready(
        &self,
        _interest: crate::object::capability::selectable::ReadyInterest,
        _trapframe: &mut crate::arch::Trapframe,
        _timeout_ticks: Option<u64>,
        _min_wait_ticks: u64,
    ) -> SelectWaitOutcome {
        SelectWaitOutcome::Ready
    }
}

fn register_cpuinfo_device() {
    let dm = DeviceManager::get_manager();
    let dev: Arc<dyn Device> = Arc::new(CpuInfoDevice::new());
    let _id = dm.register_device_with_name(String::from("cpuinfo"), dev);
}

driver_initcall!(register_cpuinfo_device);
