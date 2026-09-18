//! Common CPU frequency policy control through `/dev/cpufreq`.
//!
//! Reads list policies and hardware snapshots. Each write is one complete
//! command: `cpu <logical-id> frequency <kHz>` or
//! `cpu <logical-id> governor <performance|powersave|userspace|schedutil>`.

use alloc::{string::String, sync::Arc};
use core::{any::Any, fmt::Write};

use crate::{
    device::{
        Device, DeviceType,
        char::CharDevice,
        cpufreq::{self, CpuFrequencyGovernor, CpuFrequencyOpp, MAX_CPUFREQ_OPPS},
        manager::DeviceManager,
    },
    environment::MAX_NUM_CPUS,
    object::capability::{
        ControlOps, MemoryMappingOps,
        selectable::{SelectWaitOutcome, Selectable},
    },
    sync::IrqSpinLock,
};

struct CpuFrequencyDevice {
    snapshot: IrqSpinLock<String>,
}

impl CpuFrequencyDevice {
    fn render() -> String {
        let mut output = String::new();
        let mut seen = [0u32; MAX_NUM_CPUS];
        let mut count = 0;
        for cpu_id in 0..MAX_NUM_CPUS {
            if !crate::sched::scheduler::is_cpu_online(cpu_id) {
                continue;
            }
            let Some(policy) = cpufreq::cpu_frequency_policy_info(cpu_id) else {
                continue;
            };
            if seen[..count].contains(&policy.domain) {
                continue;
            }
            seen[count] = policy.domain;
            count += 1;
            let _ = writeln!(
                output,
                "cpu={} domain=0x{:x} cpus=0x{:x} governor={} min_khz={} max_khz={} target_khz={}",
                cpu_id,
                policy.domain,
                policy.cpus_mask,
                policy.governor.as_str(),
                policy.min_freq_khz,
                policy.max_freq_khz,
                policy.target_freq_khz
            );
            if let Some(info) = cpufreq::cpu_frequency_info(cpu_id) {
                if let Some(current) = info.current_freq_khz {
                    let _ = writeln!(output, "current_khz={current}");
                }
                let _ = writeln!(output, "raw_status=0x{:08x}", info.raw_status);
            }
            let mut opps = [CpuFrequencyOpp {
                pstate: 0,
                freq_khz: 0,
            }; MAX_CPUFREQ_OPPS];
            let opp_count = cpufreq::operating_points(policy.domain, &mut opps);
            let _ = write!(output, "available_khz=");
            for (index, opp) in opps[..opp_count].iter().enumerate() {
                let _ = write!(
                    output,
                    "{}{}",
                    if index == 0 { "" } else { " " },
                    opp.freq_khz
                );
            }
            let _ = writeln!(output);
        }
        if output.is_empty() {
            output.push_str("No CPU frequency policies registered\n");
        }
        output
    }

    fn command(buffer: &[u8]) -> Result<(), &'static str> {
        if buffer.len() > 128 {
            return Err("cpufreq: command too long");
        }
        let command =
            core::str::from_utf8(buffer).map_err(|_| "cpufreq: invalid command encoding")?;
        let mut words = command.split_whitespace();
        if words.next() != Some("cpu") {
            return Err("cpufreq: expected cpu <id> <frequency|governor> <value>");
        }
        let cpu_id = words
            .next()
            .and_then(|word| word.parse::<usize>().ok())
            .ok_or("cpufreq: invalid CPU ID")?;
        let operation = words.next().ok_or("cpufreq: missing operation")?;
        let value = words.next().ok_or("cpufreq: missing value")?;
        if words.next().is_some() {
            return Err("cpufreq: unexpected argument");
        }
        if cpu_id >= MAX_NUM_CPUS || !crate::sched::scheduler::is_cpu_online(cpu_id) {
            return Err("cpufreq: CPU is offline");
        }
        let policy =
            cpufreq::cpu_frequency_policy_info(cpu_id).ok_or("cpufreq: no policy for CPU")?;
        match operation {
            "frequency" => {
                let frequency = value
                    .parse::<u64>()
                    .map_err(|_| "cpufreq: invalid frequency in kHz")?;
                if !(policy.min_freq_khz..=policy.max_freq_khz).contains(&frequency) {
                    return Err("cpufreq: frequency outside policy limits");
                }
                cpufreq::set_domain_userspace_frequency(policy.domain, frequency)?;
            }
            "governor" => {
                let governor = match value {
                    "performance" => CpuFrequencyGovernor::Performance,
                    "powersave" => CpuFrequencyGovernor::Powersave,
                    "userspace" => CpuFrequencyGovernor::Userspace,
                    "schedutil" => CpuFrequencyGovernor::Schedutil,
                    _ => return Err("cpufreq: unknown governor"),
                };
                cpufreq::set_domain_governor(policy.domain, governor)?;
            }
            _ => return Err("cpufreq: unknown operation"),
        }
        Ok(())
    }
}

impl Device for CpuFrequencyDevice {
    fn device_type(&self) -> DeviceType {
        DeviceType::Char
    }
    fn name(&self) -> &'static str {
        "cpufreq"
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

impl CharDevice for CpuFrequencyDevice {
    fn read_byte(&self) -> Option<u8> {
        let mut byte = [0];
        (self.read_at(0, &mut byte).ok()? == 1).then_some(byte[0])
    }
    fn read_at(&self, position: u64, buffer: &mut [u8]) -> Result<usize, &'static str> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let position =
            usize::try_from(position).map_err(|_| "cpufreq: read position out of range")?;
        if position == 0 || self.snapshot.lock().is_empty() {
            let content = Self::render();
            *self.snapshot.lock() = content;
        }
        let snapshot = self.snapshot.lock();
        if position >= snapshot.len() {
            return Ok(0);
        }
        let count = buffer.len().min(snapshot.len() - position);
        buffer[..count].copy_from_slice(&snapshot.as_bytes()[position..position + count]);
        Ok(count)
    }
    fn write_byte(&self, _byte: u8) -> Result<(), &'static str> {
        Err("cpufreq: write a complete command")
    }
    fn write(&self, buffer: &[u8]) -> Result<usize, &'static str> {
        Self::command(buffer)?;
        self.snapshot.lock().clear();
        Ok(buffer.len())
    }
    fn can_read(&self) -> bool {
        true
    }
    fn can_write(&self) -> bool {
        true
    }
    fn can_seek(&self) -> bool {
        true
    }
}

impl ControlOps for CpuFrequencyDevice {
    fn control(&self, _command: u32, _arg: usize) -> Result<i32, &'static str> {
        Err("cpufreq: use policy commands")
    }
}
impl MemoryMappingOps for CpuFrequencyDevice {
    fn get_mapping_info(
        &self,
        _offset: usize,
        _length: usize,
    ) -> Result<crate::object::capability::MemoryMappingInfo, &'static str> {
        Err("cpufreq: memory mapping unsupported")
    }
    fn supports_mmap(&self) -> bool {
        false
    }
}
impl Selectable for CpuFrequencyDevice {
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

fn register() {
    DeviceManager::get_manager().register_device_with_name(
        String::from("cpufreq"),
        Arc::new(CpuFrequencyDevice {
            snapshot: IrqSpinLock::new(String::new()),
        }),
    );
}
crate::driver_initcall!(register);
