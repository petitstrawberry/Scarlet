//! Kernel device-frequency policies through `/dev/devfreq`.
//!
//! `device <name> frequency <kHz>` selects an exact OPP;
//! `device <name> governor <performance|powersave|userspace|simple_ondemand>` changes policy. Hardware
//! serialization and voltage safety remain the provider's responsibility.

use alloc::{string::String, sync::Arc};
use core::{any::Any, fmt::Write};

use crate::{
    device::{
        Device, DeviceType,
        char::CharDevice,
        devfreq::{self, DeviceFrequencyGovernor},
        manager::DeviceManager,
    },
    object::capability::{
        ControlOps, MemoryMappingOps,
        selectable::{SelectWaitOutcome, Selectable},
    },
    sync::IrqSpinLock,
};

struct DeviceFrequencyDevice {
    snapshot: IrqSpinLock<String>,
}

impl DeviceFrequencyDevice {
    fn render() -> String {
        let mut output = String::new();
        for policy in devfreq::snapshots() {
            let _ = writeln!(
                output,
                "device={} governor={} min_khz={} max_khz={} thermal_max_khz={} requested_khz={} target_khz={}",
                policy.name,
                policy.governor.as_str(),
                policy.min_khz,
                policy.max_khz,
                policy.thermal_max_khz,
                policy.requested_khz,
                policy.target_khz,
            );
            match policy.current_khz {
                Some(current) => {
                    let _ = writeln!(output, "current_khz={current}");
                }
                None => output.push_str("current_khz=unavailable\n"),
            }
            match policy.utilization_pct {
                Some(load) => {
                    let _ = writeln!(output, "utilization_pct={load}");
                }
                None => output.push_str("utilization_pct=unavailable\n"),
            }
            let _ = writeln!(
                output,
                "sample_count={} failed_samples={}",
                policy.sample_count, policy.failed_samples
            );
            let _ = writeln!(
                output,
                "upthreshold_pct={} downdifferential_pct={}",
                policy.ondemand.upthreshold_pct, policy.ondemand.downdifferential_pct
            );
            if let Ok(opps) = devfreq::operating_points(policy.name) {
                let _ = write!(output, "available_khz=");
                for (index, opp) in opps.iter().enumerate() {
                    let _ = write!(
                        output,
                        "{}{}",
                        if index == 0 { "" } else { " " },
                        opp.freq_khz
                    );
                }
                let _ = writeln!(output);
            }
        }
        if output.is_empty() {
            output.push_str("No device frequency policies registered\n");
        }
        output
    }

    fn command(bytes: &[u8]) -> Result<(), &'static str> {
        if bytes.len() > 128 {
            return Err("devfreq: command too long");
        }
        let command = core::str::from_utf8(bytes).map_err(|_| "devfreq: invalid UTF-8")?;
        let mut words = command.split_whitespace();
        if words.next() != Some("device") {
            return Err("devfreq: expected device <name> <frequency|governor|thresholds> <value>");
        }
        let name = words.next().ok_or("devfreq: missing device name")?;
        let operation = words.next().ok_or("devfreq: missing operation")?;
        let value = words.next().ok_or("devfreq: missing value")?;
        let differential = if operation == "thresholds" {
            Some(
                words
                    .next()
                    .ok_or("devfreq: missing downdifferential")?
                    .parse::<u32>()
                    .map_err(|_| "devfreq: invalid downdifferential")?,
            )
        } else {
            None
        };
        if words.next().is_some() {
            return Err("devfreq: unexpected argument");
        }
        match operation {
            "frequency" => {
                let frequency = value
                    .parse::<u64>()
                    .map_err(|_| "devfreq: invalid frequency in kHz")?;
                devfreq::set_userspace_frequency(name, frequency)
            }
            "governor" => {
                let governor = match value {
                    "performance" => DeviceFrequencyGovernor::Performance,
                    "powersave" => DeviceFrequencyGovernor::Powersave,
                    "userspace" => DeviceFrequencyGovernor::Userspace,
                    "simple_ondemand" => DeviceFrequencyGovernor::SimpleOndemand,
                    _ => return Err("devfreq: unknown governor"),
                };
                devfreq::set_governor(name, governor)
            }
            "thresholds" => devfreq::configure_simple_ondemand(
                name,
                devfreq::SimpleOndemandConfig {
                    upthreshold_pct: value
                        .parse::<u32>()
                        .map_err(|_| "devfreq: invalid upthreshold")?,
                    downdifferential_pct: differential.unwrap(),
                },
            ),
            _ => Err("devfreq: unknown operation"),
        }
    }
}

impl Device for DeviceFrequencyDevice {
    fn device_type(&self) -> DeviceType {
        DeviceType::Char
    }
    fn name(&self) -> &'static str {
        "devfreq"
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

impl CharDevice for DeviceFrequencyDevice {
    fn read_byte(&self) -> Option<u8> {
        let mut byte = [0];
        (self.read_at(0, &mut byte).ok()? == 1).then_some(byte[0])
    }
    fn read_at(&self, position: u64, buffer: &mut [u8]) -> Result<usize, &'static str> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let position =
            usize::try_from(position).map_err(|_| "devfreq: read offset out of range")?;
        if position == 0 || self.snapshot.lock().is_empty() {
            *self.snapshot.lock() = Self::render();
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
        Err("devfreq: write a complete command")
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

impl ControlOps for DeviceFrequencyDevice {
    fn control(&self, _command: u32, _arg: usize) -> Result<i32, &'static str> {
        Err("devfreq: use policy commands")
    }
}
impl MemoryMappingOps for DeviceFrequencyDevice {
    fn get_mapping_info(
        &self,
        _offset: usize,
        _length: usize,
    ) -> Result<crate::object::capability::MemoryMappingInfo, &'static str> {
        Err("devfreq: memory mapping unsupported")
    }
    fn supports_mmap(&self) -> bool {
        false
    }
}
impl Selectable for DeviceFrequencyDevice {
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
        String::from("devfreq"),
        Arc::new(DeviceFrequencyDevice {
            snapshot: IrqSpinLock::new(String::new()),
        }),
    );
}
crate::driver_initcall!(register);
