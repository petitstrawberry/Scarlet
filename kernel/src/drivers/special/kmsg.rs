extern crate alloc;

use alloc::{string::String, sync::Arc};
use core::any::Any;

use crate::{
    device::{self, Device, DeviceType, char::CharDevice, manager::DeviceManager},
    driver_initcall,
    object::capability::{
        ControlOps, MemoryMappingOps,
        selectable::{ReadyInterest, ReadySet, SelectWaitOutcome, Selectable},
    },
    task::mytask,
};

pub struct KmsgDevice;

impl Selectable for KmsgDevice {
    fn current_ready(&self, interest: ReadyInterest) -> ReadySet {
        let mut set = ReadySet::none();
        if interest.read {
            set.read = !crate::log::is_empty();
        }
        set
    }

    fn wait_until_ready(
        &self,
        interest: ReadyInterest,
        trapframe: &mut crate::arch::Trapframe,
        _timeout_ticks: Option<u64>,
        _min_wait_ticks: u64,
    ) -> SelectWaitOutcome {
        if interest.read && crate::log::is_empty() {
            if let Some(task) = mytask() {
                crate::log::READER_WAKER.wait(task.get_id(), trapframe);
            }
        }
        SelectWaitOutcome::Ready
    }
}

impl Device for KmsgDevice {
    fn device_type(&self) -> DeviceType {
        DeviceType::Char
    }

    fn name(&self) -> &'static str {
        "kmsg"
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

impl CharDevice for KmsgDevice {
    fn read_byte(&self) -> Option<u8> {
        while crate::log::is_empty() {
            if let Some(task) = mytask() {
                crate::log::READER_WAKER.wait(task.get_id(), task.get_trapframe());
            } else {
                return None;
            }
        }
        let mut buf = [0u8; 1];
        if crate::log::read_at(crate::log::tail(), &mut buf) > 0 {
            Some(buf[0])
        } else {
            None
        }
    }

    fn write_byte(&self, _byte: u8) -> Result<(), &'static str> {
        Err("kmsg is read-only")
    }

    fn can_read(&self) -> bool {
        !crate::log::is_empty()
    }

    fn can_write(&self) -> bool {
        false
    }

    fn read_at(&self, position: u64, buffer: &mut [u8]) -> Result<usize, &'static str> {
        let pos = position as usize;
        while pos >= crate::log::head() {
            if let Some(task) = mytask() {
                crate::log::READER_WAKER.wait(task.get_id(), task.get_trapframe());
            } else {
                return Ok(0);
            }
        }
        Ok(crate::log::read_at(pos, buffer))
    }
}

impl ControlOps for KmsgDevice {
    fn control(&self, _command: u32, _arg: usize) -> Result<i32, &'static str> {
        Err("Control operations not supported")
    }
}

impl MemoryMappingOps for KmsgDevice {
    fn get_mapping_info(
        &self,
        _offset: usize,
        _length: usize,
    ) -> Result<(usize, usize, bool), &'static str> {
        Err("Memory mapping not supported by kmsg device")
    }

    fn supports_mmap(&self) -> bool {
        false
    }
}

fn register_kmsg_device() {
    let dm = DeviceManager::get_manager();
    let dev: Arc<dyn Device> = Arc::new(KmsgDevice);
    let _id = dm.register_device_with_name(String::from("kmsg"), dev);
}

driver_initcall!(register_kmsg_device);
