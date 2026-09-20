//! Read-only IRQ delivery counters at `/dev/interrupts`.
//!
//! Like `cpuinfo`, this is a diagnostic text interface, not a stable ABI.
//! Counters are kernel-wide totals; compare snapshots using `time_ns` to
//! distinguish normal traffic from repeated delivery of an uncleared source.

use alloc::{string::String, sync::Arc};
use core::{any::Any, fmt::Write};

use crate::{
    device::{Device, DeviceType, char::CharDevice, manager::DeviceManager},
    interrupt::InterruptManager,
    object::capability::{
        ControlOps, MemoryMappingOps,
        selectable::{ReadyInterest, SelectWaitOutcome, Selectable},
    },
    sync::IrqSpinLock,
};

struct InterruptsDevice {
    snapshot: IrqSpinLock<String>,
}

impl InterruptsDevice {
    fn render() -> String {
        let entries = InterruptManager::global().statistics();
        let mut output = String::new();
        let _ = writeln!(output, "time_ns={}", crate::time::current_time_ns());
        let (total_pages, free_pages) = crate::mem::pmm::stats();
        let _ = writeln!(
            output,
            "pmm_total_bytes={} pmm_free_bytes={} kernel_heap_initial_bytes={}",
            total_pages * crate::environment::PAGE_SIZE,
            free_pages * crate::environment::PAGE_SIZE,
            crate::environment::KERNEL_HEAP_SIZE,
        );
        let _ = writeln!(output, "IRQ HWIRQ DELIVERIES");
        for entry in entries {
            let _ = writeln!(
                output,
                "{} {} {}",
                entry.interrupt_id, entry.hardware_id, entry.deliveries
            );
        }
        output
    }
}

impl Device for InterruptsDevice {
    fn device_type(&self) -> DeviceType {
        DeviceType::Char
    }
    fn name(&self) -> &'static str {
        "interrupts"
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

impl CharDevice for InterruptsDevice {
    fn read_byte(&self) -> Option<u8> {
        let mut byte = [0];
        (self.read_at(0, &mut byte).ok()? == 1).then_some(byte[0])
    }
    fn read_at(&self, position: u64, buffer: &mut [u8]) -> Result<usize, &'static str> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let position =
            usize::try_from(position).map_err(|_| "interrupts: read offset out of range")?;
        if position == 0 || self.snapshot.lock().is_empty() {
            // Registry access and formatting must precede the snapshot lock.
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
        Err("interrupts: read only")
    }
    fn write(&self, _buffer: &[u8]) -> Result<usize, &'static str> {
        Err("interrupts: read only")
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

impl ControlOps for InterruptsDevice {
    fn control(&self, _command: u32, _arg: usize) -> Result<i32, &'static str> {
        Err("interrupts: control unsupported")
    }
}
impl MemoryMappingOps for InterruptsDevice {
    fn get_mapping_info(
        &self,
        _offset: usize,
        _length: usize,
    ) -> Result<crate::object::capability::MemoryMappingInfo, &'static str> {
        Err("interrupts: memory mapping unsupported")
    }
    fn supports_mmap(&self) -> bool {
        false
    }
}
impl Selectable for InterruptsDevice {
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
        String::from("interrupts"),
        Arc::new(InterruptsDevice {
            snapshot: IrqSpinLock::new(String::new()),
        }),
    );
}
crate::driver_initcall!(register);
