//! Temporary CPU topology information character device.
//!
//! Exposes scheduler-visible CPU class and capacity through `/dev/cpuinfo`.
//! This is a diagnostic interface, not a stable ABI. It should be replaced by
//! a proper kernel introspection interface once the scheduler API settles.

extern crate alloc;

use alloc::{string::String, sync::Arc};
use core::{any::Any, fmt::Write};
use crate::sync::Mutex;

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
pub struct CpuInfoDevice {
    snapshot: Mutex<String>,
}

impl CpuInfoDevice {
    /// Create a CPU information device.
    ///
    /// # Returns
    ///
    /// A new CPU information character device.
    pub fn new() -> Self {
        Self {
            snapshot: Mutex::new(String::new()),
        }
    }

    fn copy_from_snapshot(snapshot: &str, position: usize, buffer: &mut [u8]) -> usize {
        let bytes = snapshot.as_bytes();

        if position >= bytes.len() || buffer.is_empty() {
            return 0;
        }

        let bytes_to_read = core::cmp::min(buffer.len(), bytes.len() - position);
        buffer[..bytes_to_read].copy_from_slice(&bytes[position..position + bytes_to_read]);
        bytes_to_read
    }

    fn render() -> String {
        let mut output = String::new();
        let migration_stats = crate::sched::scheduler::scheduler_migration_stats();

        let _ = writeln!(output, "scheduler migrations\t: {}", migration_stats.total);
        let _ = writeln!(
            output,
            "scheduler promotions\t: {}",
            migration_stats.promotions
        );
        let _ = writeln!(
            output,
            "scheduler demotions\t: {}",
            migration_stats.demotions
        );
        let _ = writeln!(
            output,
            "scheduler cooldown skips\t: {}",
            migration_stats.cooldown_skips
        );
        let _ = writeln!(
            output,
            "scheduler work steals\t: {}",
            migration_stats.work_steals
        );
        let _ = writeln!(output);

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
            if let Some(domain_id) = topology.domain_id {
                let _ = writeln!(output, "topology domain\t: 0x{:x}", domain_id);
                let _ = writeln!(output, "domain cpus\t: 0x{:x}", topology.domain_cpus_mask);
            } else {
                let _ = writeln!(output, "topology domain\t: none");
                let _ = writeln!(output, "domain cpus\t: 0x0");
            }
            let _ = writeln!(output, "util scale\t: {}", SCHED_UTIL_SCALE);
            if let Some(util) = crate::sched::scheduler::cpu_util_snapshot(cpu_id) {
                let _ = writeln!(output, "util avg\t: {}", util.util_avg);
                let _ = writeln!(output, "util min\t: {}", util.util_min);
                let _ = writeln!(output, "runnable\t: {}", util.runnable_tasks);
            }
            if let Some(policy) = crate::device::cpufreq::cpu_frequency_policy_info(cpu_id) {
                let _ = writeln!(output, "cpufreq gov\t: {}", policy.governor.as_str());
                let _ = writeln!(output, "policy cpus\t: 0x{:x}", policy.cpus_mask);
                let _ = writeln!(output, "policy min kHz\t: {}", policy.min_freq_khz);
                let _ = writeln!(output, "policy max kHz\t: {}", policy.max_freq_khz);
                let _ = writeln!(output, "policy target kHz\t: {}", policy.target_freq_khz);
                let _ = writeln!(output, "policy util\t: {}", policy.last_util);
            }
            if let Some(freq) = crate::device::cpufreq::cpu_frequency_info(cpu_id) {
                let _ = writeln!(output, "perf domain\t: 0x{:x}", freq.performance_domain);
                let _ = writeln!(output, "dvfs status\t: 0x{:08x}", freq.raw_status);
                if let Some(pstate) = freq.current_pstate {
                    let _ = writeln!(output, "cur pstate\t: {}", pstate);
                }
                if let Some(pstate) = freq.target_pstate {
                    let _ = writeln!(output, "target pstate\t: {}", pstate);
                }
                if let Some(freq_khz) = freq.current_freq_khz {
                    let _ = writeln!(output, "cur freq kHz\t: {}", freq_khz);
                }
                if let Some(freq_khz) = freq.target_freq_khz {
                    let _ = writeln!(output, "target freq kHz\t: {}", freq_khz);
                }
                if let Some(freq_khz) = freq.max_freq_khz {
                    let _ = writeln!(output, "max freq kHz\t: {}", freq_khz);
                }
            }
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
        let position = usize::try_from(position).map_err(|_| "Read position out of range")?;

        if buffer.is_empty() {
            return Ok(0);
        }

        if position == 0 {
            let content = Self::render();
            let mut snapshot = self.snapshot.lock();
            *snapshot = content;
            return Ok(Self::copy_from_snapshot(&snapshot, position, buffer));
        }

        {
            let snapshot = self.snapshot.lock();
            if !snapshot.is_empty() {
                return Ok(Self::copy_from_snapshot(&snapshot, position, buffer));
            }
        }

        let content = Self::render();
        let mut snapshot = self.snapshot.lock();
        if snapshot.is_empty() {
            *snapshot = content;
        }
        Ok(Self::copy_from_snapshot(&snapshot, position, buffer))
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
