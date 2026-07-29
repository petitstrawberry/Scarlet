//! Registered GPU control device object.

use alloc::sync::Arc;
use core::any::Any;

use super::{GpuBackend, GpuConnection};
use crate::device::{Device, DeviceType, char::CharDevice};
use crate::object::capability::selectable::{ReadyInterest, ReadySet, SelectWaitOutcome};
use crate::object::capability::{ControlOps, MemoryMappingInfo, MemoryMappingOps, Selectable};

/// Registered `/dev/gpuN` control device.
///
/// Every open creates an independent [`GpuConnection`]. This object does not
/// itself expose a GPU child handle or a memory mapping capability.
pub struct GpuControlDevice {
    backend: Arc<dyn GpuBackend>,
}

impl GpuControlDevice {
    /// Create a registered GPU control device for a backend.
    ///
    /// # Arguments
    ///
    /// * `backend` - Backend that supplies stable GPU information.
    ///
    /// # Returns
    ///
    /// A GPU control device ready for registration in devfs.
    pub fn new(backend: Arc<dyn GpuBackend>) -> Self {
        Self { backend }
    }
}

impl Device for GpuControlDevice {
    fn open(self: Arc<Self>) -> Result<Arc<dyn Device>, &'static str> {
        Ok(Arc::new(GpuConnection::new(Arc::clone(&self.backend))))
    }

    fn device_type(&self) -> DeviceType {
        DeviceType::Char
    }

    fn name(&self) -> &'static str {
        "gpu"
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

impl CharDevice for GpuControlDevice {
    fn read_byte(&self) -> Option<u8> {
        None
    }

    fn write_byte(&self, _byte: u8) -> Result<(), &'static str> {
        Err("GPU control devices do not support byte writes")
    }

    fn write(&self, _buffer: &[u8]) -> Result<usize, &'static str> {
        Err("GPU control devices do not support stream writes")
    }

    fn can_read(&self) -> bool {
        false
    }

    fn can_write(&self) -> bool {
        false
    }
}

impl ControlOps for GpuControlDevice {
    fn control(&self, _command: u32, _arg: usize) -> Result<i32, &'static str> {
        Err("GPU control operations require an opened GPU connection")
    }
}

impl MemoryMappingOps for GpuControlDevice {
    fn get_mapping_info(
        &self,
        _offset: usize,
        _length: usize,
    ) -> Result<MemoryMappingInfo, &'static str> {
        Err("GPU control connections do not support memory mapping")
    }

    fn on_mapped(&self, _vaddr: usize, _paddr: usize, _length: usize, _offset: usize) {}

    fn on_unmapped(&self, _vaddr: usize, _length: usize) {}

    fn supports_mmap(&self) -> bool {
        false
    }
}

impl Selectable for GpuControlDevice {
    fn current_ready(&self, _interest: ReadyInterest) -> ReadySet {
        ReadySet::none()
    }

    fn wait_until_ready(
        &self,
        _interest: ReadyInterest,
        _trapframe: &mut crate::arch::Trapframe,
        _timeout_ticks: Option<u64>,
        _min_wait_ticks: u64,
    ) -> SelectWaitOutcome {
        SelectWaitOutcome::TimedOut
    }
}
