//! Read-only thermal-zone and applied cooling-state snapshots through `/dev/thermal`.

use alloc::{
    string::{String, ToString},
    sync::Arc,
};
use core::{any::Any, fmt::Write};

use crate::{
    device::{Device, DeviceType, char::CharDevice, manager::DeviceManager, thermal},
    object::capability::{
        ControlOps, MemoryMappingOps,
        selectable::{SelectWaitOutcome, Selectable},
    },
    sync::IrqSpinLock,
};

struct ThermalDevice {
    snapshot: IrqSpinLock<String>,
}

impl ThermalDevice {
    fn render() -> String {
        let mut output = String::new();
        for zone in thermal::snapshots() {
            let temperature = zone
                .hottest_mc
                .map(|value| value.to_string())
                .unwrap_or_else(|| String::from("unavailable"));
            let _ = writeln!(
                output,
                "zone={} temperature_mc={} sample_count={} failed_samples={}",
                zone.name, temperature, zone.sample_count, zone.failed_samples,
            );
            for cooler in zone.coolers {
                let _ = writeln!(
                    output,
                    "cooler={} applied_state={} max_state={}",
                    cooler.name, cooler.applied_state, cooler.max_state,
                );
            }
        }
        if output.is_empty() {
            output.push_str("No thermal zones registered\n");
        }
        output
    }
}

impl Device for ThermalDevice {
    fn device_type(&self) -> DeviceType {
        DeviceType::Char
    }
    fn name(&self) -> &'static str {
        "thermal"
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

impl CharDevice for ThermalDevice {
    fn read_byte(&self) -> Option<u8> {
        let mut byte = [0];
        (self.read_at(0, &mut byte).ok()? == 1).then_some(byte[0])
    }
    fn read_at(&self, position: u64, buffer: &mut [u8]) -> Result<usize, &'static str> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let position =
            usize::try_from(position).map_err(|_| "thermal: read offset out of range")?;
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
        Err("thermal: read-only device")
    }
    fn write(&self, _buffer: &[u8]) -> Result<usize, &'static str> {
        Err("thermal: read-only device")
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

impl ControlOps for ThermalDevice {
    fn control(&self, _command: u32, _arg: usize) -> Result<i32, &'static str> {
        Err("thermal: controls are not supported")
    }
}
impl MemoryMappingOps for ThermalDevice {
    fn get_mapping_info(
        &self,
        _offset: usize,
        _length: usize,
    ) -> Result<crate::object::capability::MemoryMappingInfo, &'static str> {
        Err("thermal: memory mapping unsupported")
    }
    fn supports_mmap(&self) -> bool {
        false
    }
}
impl Selectable for ThermalDevice {
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
        String::from("thermal"),
        Arc::new(ThermalDevice {
            snapshot: IrqSpinLock::new(String::new()),
        }),
    );
}
crate::driver_initcall!(register);
