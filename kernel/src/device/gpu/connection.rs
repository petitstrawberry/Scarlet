//! Per-open GPU control connection.

use alloc::sync::Arc;
use core::any::Any;

use super::{
    GPU_ABI_VERSION, GPU_BUFFER_FLAGS_VALID, GPU_CREATE_BUFFER, GPU_CREATE_TIMELINE,
    GPU_QUERY_INFO, GPU_RESULT_INVALID_ABI, GPU_RESULT_INVALID_ARGUMENT,
    GPU_RESULT_OUT_OF_RESOURCES, GpuBackend, GpuBuffer, GpuCreateBuffer, GpuCreateTimeline,
    GpuQueryInfo, GpuTimeline,
};
use crate::device::{Device, DeviceType, char::CharDevice};
use crate::library::std::usercopy::{copy_from_user, copy_to_user};
use crate::object::KernelObject;
use crate::object::capability::selectable::{ReadyInterest, ReadySet, SelectWaitOutcome};
use crate::object::capability::{ControlOps, MemoryMappingInfo, MemoryMappingOps, Selectable};
use crate::object::handle::AccessMode;

/// Independent GPU endpoint created for each `/dev/gpuN` open.
pub struct GpuConnection {
    backend: Arc<dyn GpuBackend>,
}

impl GpuConnection {
    /// Create a GPU connection for one device-file open.
    ///
    /// # Arguments
    ///
    /// * `backend` - Backend that services this connection.
    /// # Returns
    ///
    /// A new independent GPU connection.
    pub(crate) fn new(backend: Arc<dyn GpuBackend>) -> Self {
        Self { backend }
    }

    /// Fill a fixed-width query response from this connection's backend.
    pub(crate) fn query_info(&self, query: &mut GpuQueryInfo) {
        let reserved = query.reserved;
        query.clear_response();
        if query.abi_version != GPU_ABI_VERSION {
            query.result = GPU_RESULT_INVALID_ABI;
            return;
        }
        if reserved != 0 {
            query.result = GPU_RESULT_INVALID_ARGUMENT;
            return;
        }

        let info = self.backend.query_info();
        query.device_state = info.device.state as u32;
        query.execution_support = info.device.execution_support;
        query.max_opaque_command_size = info.device.max_opaque_command_size;
        query.backend_feature_bits = info.backend_feature_bits;
        query.backend_id_len = info.backend_id_len;
        query.backend_info_len = info.opaque_info_len;
        query.backend_id = info.backend_id;
        query.backend_info = info.opaque_info;
    }

    fn handle_query_info(&self, arg: usize) -> Result<i32, &'static str> {
        let mut query: GpuQueryInfo = read_user_value(arg)?;
        self.query_info(&mut query);
        write_user_value(arg, &query)?;
        Ok(0)
    }

    fn handle_create_buffer(&self, arg: usize) -> Result<i32, &'static str> {
        let mut request: GpuCreateBuffer = read_user_value(arg)?;
        request.clear_response();
        if request.abi_version != GPU_ABI_VERSION {
            request.result = GPU_RESULT_INVALID_ABI;
            write_user_value(arg, &request)?;
            return Ok(0);
        }
        if request.reserved != 0 || request.flags & !GPU_BUFFER_FLAGS_VALID != 0 {
            request.result = GPU_RESULT_INVALID_ARGUMENT;
            write_user_value(arg, &request)?;
            return Ok(0);
        }
        let size_bytes = match usize::try_from(request.size_bytes) {
            Ok(size) => size,
            Err(_) => {
                request.result = GPU_RESULT_INVALID_ARGUMENT;
                write_user_value(arg, &request)?;
                return Ok(0);
            }
        };
        let buffer = match GpuBuffer::new(Arc::clone(&self.backend), size_bytes, request.flags) {
            Ok(buffer) => buffer,
            Err(_) => {
                request.result = GPU_RESULT_OUT_OF_RESOURCES;
                write_user_value(arg, &request)?;
                return Ok(0);
            }
        };
        let allocated_size = u64::try_from(buffer.allocation_size())
            .map_err(|_| "GPU buffer allocation size does not fit ABI")?;
        let task = crate::task::mytask().ok_or("No current task for GPU buffer creation")?;
        let handle = task.handle_table.insert_with_metadata(
            KernelObject::Gpu(Arc::new(buffer)),
            super::child_handle_metadata(AccessMode::ReadWrite),
        );
        match handle {
            Ok(handle) => {
                request.buffer_handle = handle;
                request.cpu_visible =
                    u32::from(request.flags & super::GPU_BUFFER_FLAG_CPU_VISIBLE != 0);
                request.allocated_size = allocated_size;
                if let Err(error) = write_user_value(arg, &request) {
                    task.handle_table.remove(handle);
                    return Err(error);
                }
                Ok(0)
            }
            Err(_) => {
                request.result = GPU_RESULT_OUT_OF_RESOURCES;
                write_user_value(arg, &request)?;
                Ok(0)
            }
        }
    }

    fn handle_create_timeline(&self, arg: usize) -> Result<i32, &'static str> {
        let mut request: GpuCreateTimeline = read_user_value(arg)?;
        request.clear_response();
        if request.abi_version != GPU_ABI_VERSION {
            request.result = GPU_RESULT_INVALID_ABI;
            write_user_value(arg, &request)?;
            return Ok(0);
        }
        if request.flags != 0 || request.reserved != 0 {
            request.result = GPU_RESULT_INVALID_ARGUMENT;
            write_user_value(arg, &request)?;
            return Ok(0);
        }
        let timeline: Arc<dyn super::GpuObject> = Arc::new(GpuTimeline::new(
            Arc::clone(&self.backend),
            request.initial_value,
        ));
        let task = crate::task::mytask().ok_or("No current task for GPU timeline creation")?;
        let handle = task.handle_table.insert_with_metadata(
            KernelObject::Gpu(timeline),
            super::child_handle_metadata(AccessMode::ReadWrite),
        );
        match handle {
            Ok(handle) => {
                request.timeline_handle = handle;
                request.current_value = request.initial_value;
                if let Err(error) = write_user_value(arg, &request) {
                    task.handle_table.remove(handle);
                    return Err(error);
                }
                Ok(0)
            }
            Err(_) => {
                request.result = GPU_RESULT_OUT_OF_RESOURCES;
                write_user_value(arg, &request)?;
                Ok(0)
            }
        }
    }
}

impl Device for GpuConnection {
    fn open(self: Arc<Self>) -> Result<Arc<dyn Device>, &'static str> {
        Ok(self)
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

impl CharDevice for GpuConnection {
    fn read_byte(&self) -> Option<u8> {
        None
    }

    fn write_byte(&self, _byte: u8) -> Result<(), &'static str> {
        Err("GPU connections do not support byte writes")
    }

    fn write(&self, _buffer: &[u8]) -> Result<usize, &'static str> {
        Err("GPU connections do not support stream writes")
    }

    fn can_read(&self) -> bool {
        false
    }

    fn can_write(&self) -> bool {
        false
    }
}

impl ControlOps for GpuConnection {
    fn control(&self, command: u32, arg: usize) -> Result<i32, &'static str> {
        match command {
            GPU_QUERY_INFO => self.handle_query_info(arg),
            GPU_CREATE_BUFFER => self.handle_create_buffer(arg),
            GPU_CREATE_TIMELINE => self.handle_create_timeline(arg),
            _ => Err("Unsupported GPU control command"),
        }
    }

    fn supported_control_commands(&self) -> alloc::vec::Vec<(u32, &'static str)> {
        alloc::vec![
            (GPU_QUERY_INFO, "Query GPU and backend information"),
            (GPU_CREATE_BUFFER, "Create a GPU buffer child handle"),
            (GPU_CREATE_TIMELINE, "Create a GPU timeline child handle"),
        ]
    }
}

impl MemoryMappingOps for GpuConnection {
    fn get_mapping_info(
        &self,
        _offset: usize,
        _length: usize,
    ) -> Result<MemoryMappingInfo, &'static str> {
        Err("GPU connections do not support memory mapping")
    }

    fn on_mapped(&self, _vaddr: usize, _paddr: usize, _length: usize, _offset: usize) {}

    fn on_unmapped(&self, _vaddr: usize, _length: usize) {}

    fn supports_mmap(&self) -> bool {
        false
    }
}

impl Selectable for GpuConnection {
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

pub(super) fn read_user_value<T: Copy>(ptr: usize) -> Result<T, &'static str> {
    if ptr == 0 {
        return Err("GPU query pointer is null");
    }

    let task = crate::task::mytask().ok_or("No current task for GPU query")?;
    let mut value = core::mem::MaybeUninit::<T>::uninit();
    // SAFETY: `bytes` covers exactly the uninitialized storage for `T` and is
    // fully initialized by `copy_from_user` before `assume_init` is called.
    let bytes = unsafe {
        core::slice::from_raw_parts_mut(value.as_mut_ptr() as *mut u8, core::mem::size_of::<T>())
    };
    copy_from_user(&task, ptr, bytes).map_err(|_| "Failed to copy GPU query from user")?;
    // SAFETY: `copy_from_user` filled all bytes of the `T` storage above.
    Ok(unsafe { value.assume_init() })
}

pub(super) fn write_user_value<T: Copy>(ptr: usize, value: &T) -> Result<(), &'static str> {
    if ptr == 0 {
        return Err("GPU query pointer is null");
    }

    let task = crate::task::mytask().ok_or("No current task for GPU query")?;
    // SAFETY: `value` is initialized and remains valid for the synchronous
    // user-copy operation.
    let bytes = unsafe {
        core::slice::from_raw_parts(value as *const T as *const u8, core::mem::size_of::<T>())
    };
    copy_to_user(&task, ptr, bytes).map_err(|_| "Failed to copy GPU query to user")
}
