//! Null character device
//!
//! Behavior:
//! - Read: always returns EOF (0 bytes)
//! - Write: discards all data and reports success
//! - `can_read`/`can_write`: always true
//!
//! It is registered in the `DeviceManager` with the name `"null"`,
//! and is expected to appear as `/dev/null` via DevFS.

extern crate alloc;

use alloc::{string::String, sync::Arc};
use core::any::Any;

use crate::{
    device::{self, Device, DeviceType, char::CharDevice, manager::DeviceManager},
    driver_initcall,
    object::capability::{ControlOps, MemoryMappingOps, selectable::Selectable},
};

pub struct NullDevice;

impl Selectable for NullDevice {
    fn wait_until_ready(
        &self,
        _interest: crate::object::capability::selectable::ReadyInterest,
        _trapframe: &mut crate::arch::Trapframe,
        _timeout_ticks: Option<u64>,
    ) -> crate::object::capability::selectable::SelectWaitOutcome {
        crate::object::capability::selectable::SelectWaitOutcome::Ready
    }
}

impl Device for NullDevice {
    fn device_type(&self) -> DeviceType {
        DeviceType::Char
    }

    fn name(&self) -> &'static str {
        "null"
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn as_char_device(&self) -> Option<&dyn device::char::CharDevice> {
        Some(self)
    }
}

impl CharDevice for NullDevice {
    fn read_byte(&self) -> Option<u8> {
        // Always EOF
        None
    }

    fn write_byte(&self, _byte: u8) -> Result<(), &'static str> {
        // Discard the data
        Ok(())
    }

    fn can_read(&self) -> bool {
        // Always readable (immediate EOF)
        true
    }

    fn can_write(&self) -> bool {
        true
    }
}

impl ControlOps for NullDevice {
    fn control(&self, _command: u32, _arg: usize) -> Result<i32, &'static str> {
        Err("Control operations not supported")
    }
}

impl MemoryMappingOps for NullDevice {
    fn get_mapping_info(
        &self,
        _offset: usize,
        _length: usize,
    ) -> Result<(usize, usize, bool), &'static str> {
        Err("Memory mapping not supported by null device")
    }

    fn supports_mmap(&self) -> bool {
        false
    }
}

fn register_null_device() {
    let dm = DeviceManager::get_manager();
    let dev: Arc<dyn Device> = Arc::new(NullDevice);
    // Register with explicit name: "null"
    let id = dm.register_device_with_name(String::from("null"), dev);
    crate::println!("Null device registered as 'null' with ID: {}", id);
}

driver_initcall!(register_null_device);
