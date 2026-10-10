//! Opt-in cumulative network timing diagnostics at `/dev/net_profile`.
//! Writes accept only `0` or `1`, optionally followed by one newline.

use alloc::{string::String, sync::Arc};
use core::{any::Any, fmt::Write};

use crate::{
    device::{Device, DeviceType, char::CharDevice, manager::DeviceManager},
    network::profile,
    object::capability::{
        ControlOps, MemoryMappingOps,
        selectable::{ReadyInterest, SelectWaitOutcome, Selectable},
    },
    sync::IrqSpinLock,
};

struct NetworkProfileDevice {
    snapshot: IrqSpinLock<String>,
}

impl NetworkProfileDevice {
    fn render() -> String {
        // Read counters and format before taking the device's snapshot lock.
        let counters = profile::snapshot();
        let enabled = profile::enabled();
        let mut output = String::new();
        let _ = writeln!(
            output,
            "time_ns={} enabled={} sample_every={}",
            crate::timer::get_time_ns(),
            u8::from(enabled),
            profile::SAMPLE_EVERY
        );
        let _ = writeln!(
            output,
            "cumulative=1 clock=monotonic_ns timing=wall_includes_preemption snapshot=relaxed_noncoherent"
        );
        let _ = writeln!(
            output,
            "timings_are_sampled=1 in_flight_may_finish_after_disable=1 overlapping_stages_do_not_sum=1"
        );
        let _ = writeln!(
            output,
            "STAGE CALLS BYTES CAPACITY TIMED_CALLS TIMED_BYTES TIMED_CAPACITY TIMED_NS"
        );
        for (name, counter) in profile::STAGE_NAMES.iter().zip(counters) {
            let _ = writeln!(
                output,
                "{} {} {} {} {} {} {} {}",
                name,
                counter.calls,
                counter.bytes,
                counter.capacity,
                counter.timed_calls,
                counter.timed_bytes,
                counter.timed_capacity,
                counter.timed_ns
            );
        }
        output
    }
}

impl Device for NetworkProfileDevice {
    fn device_type(&self) -> DeviceType {
        DeviceType::Char
    }
    fn name(&self) -> &'static str {
        "net_profile"
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

impl CharDevice for NetworkProfileDevice {
    fn read_byte(&self) -> Option<u8> {
        let mut byte = [0];
        (self.read_at(0, &mut byte).ok()? == 1).then_some(byte[0])
    }
    fn read_at(&self, position: u64, buffer: &mut [u8]) -> Result<usize, &'static str> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let position =
            usize::try_from(position).map_err(|_| "net_profile: read offset out of range")?;
        if position == 0 || self.snapshot.lock().is_empty() {
            // Counter reads and formatting must precede the snapshot lock.
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
        Err("net_profile: write a complete 0 or 1 command")
    }
    fn write(&self, buffer: &[u8]) -> Result<usize, &'static str> {
        profile::command(buffer)?;
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

impl ControlOps for NetworkProfileDevice {
    fn control(&self, _command: u32, _arg: usize) -> Result<i32, &'static str> {
        Err("net_profile: control unsupported")
    }
}
impl MemoryMappingOps for NetworkProfileDevice {
    fn get_mapping_info(
        &self,
        _offset: usize,
        _length: usize,
    ) -> Result<crate::object::capability::MemoryMappingInfo, &'static str> {
        Err("net_profile: memory mapping unsupported")
    }
    fn supports_mmap(&self) -> bool {
        false
    }
}
impl Selectable for NetworkProfileDevice {
    fn wait_until_ready(
        &self,
        _interest: ReadyInterest,
        _trapframe: &mut crate::arch::Trapframe,
        _timeout_ticks: Option<u64>,
        _min_wait_ticks: u64,
    ) -> SelectWaitOutcome {
        SelectWaitOutcome::Ready
    }
}

fn register() {
    DeviceManager::get_manager().register_device_with_name(
        String::from("net_profile"),
        Arc::new(NetworkProfileDevice {
            snapshot: IrqSpinLock::new(String::new()),
        }),
    );
}
crate::driver_initcall!(register);
