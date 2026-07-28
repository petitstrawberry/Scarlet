//! GPU control library for Scarlet OS.
//!
//! This library opens a GPU control connection, creates GPU buffer and timeline
//! child handles, and exposes fixed-width ABI mirrors for generic controls.

#![no_std]

extern crate scarlet_std as std;

use std::{
    fs::File,
    handle::{Handle, HandleError, HandleResult},
};

/// GPU control command constants.
pub mod commands {
    /// Query generic GPU and backend information.
    pub const GPU_QUERY_INFO: u32 = 0x4750;
    /// Create a kernel-owned GPU buffer child handle.
    pub const GPU_CREATE_BUFFER: u32 = 0x4751;
    /// Query a GPU buffer child handle.
    pub const GPU_BUFFER_QUERY_INFO: u32 = 0x4752;
    /// Create a GPU timeline child handle.
    pub const GPU_CREATE_TIMELINE: u32 = 0x4753;
    /// Query a GPU timeline child handle.
    pub const GPU_TIMELINE_QUERY: u32 = 0x4754;
    /// Advance a GPU timeline child handle.
    pub const GPU_TIMELINE_SIGNAL: u32 = 0x4755;
    /// Permanently fail a GPU timeline child handle.
    pub const GPU_TIMELINE_FAIL: u32 = 0x4756;
    /// Create a fixed-target GPU timeline point child handle.
    pub const GPU_TIMELINE_CREATE_POINT: u32 = 0x4757;
}

/// ABI version accepted by [`GpuQueryInfo`].
pub const GPU_ABI_VERSION: u32 = 1;
/// Query completed successfully.
pub const GPU_RESULT_SUCCESS: u32 = 0;
/// The request used an unsupported ABI version.
pub const GPU_RESULT_INVALID_ABI: u32 = 1;
/// The request contained invalid flags, reserved fields, or sizes.
pub const GPU_RESULT_INVALID_ARGUMENT: u32 = 2;
/// The kernel could not allocate the requested object or handle.
pub const GPU_RESULT_OUT_OF_RESOURCES: u32 = 3;
/// The operation is invalid for the object's current state.
pub const GPU_RESULT_INVALID_STATE: u32 = 4;

/// Create buffer flag permitting CPU memory mappings.
pub const GPU_BUFFER_FLAG_CPU_VISIBLE: u32 = 1 << 0;
/// All currently defined GPU buffer creation flags.
pub const GPU_BUFFER_FLAGS_VALID: u32 = GPU_BUFFER_FLAG_CPU_VISIBLE;

/// The GPU backend is not available for control.
pub const GPU_DEVICE_STATE_UNAVAILABLE: u32 = 0;
/// The GPU backend is available for its advertised operations.
pub const GPU_DEVICE_STATE_READY: u32 = 1;
/// The GPU backend was lost after it became available.
pub const GPU_DEVICE_STATE_LOST: u32 = 2;

/// No generic execution support is available.
pub const GPU_EXECUTION_SUPPORT_NONE: u32 = 0;
/// Generic address-space operations are available.
pub const GPU_EXECUTION_SUPPORT_ADDRESS_SPACE: u32 = 1 << 0;
/// Generic memory operations are available.
pub const GPU_EXECUTION_SUPPORT_MEMORY: u32 = 1 << 1;
/// Generic queue operations are available.
pub const GPU_EXECUTION_SUPPORT_QUEUE: u32 = 1 << 2;
/// Generic timeline operations are available.
pub const GPU_EXECUTION_SUPPORT_TIMELINE: u32 = 1 << 3;
/// Generic presentation operations are available.
pub const GPU_EXECUTION_SUPPORT_PRESENTATION: u32 = 1 << 4;

/// Fixed byte capacity of an opaque backend or dialect identifier.
pub const GPU_BACKEND_ID_BYTES: usize = 32;
/// Fixed byte capacity of opaque backend-defined information.
pub const GPU_BACKEND_INFO_BYTES: usize = 64;

/// Fixed-width request and response for [`commands::GPU_QUERY_INFO`].
///
/// Initialize with [`GpuQueryInfo::new`] and inspect `result` after the
/// operation. The explicit result is retained even when the control syscall
/// itself succeeds.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GpuQueryInfo {
    /// ABI version supplied by userspace and echoed by the kernel.
    pub abi_version: u32,
    /// Explicit `GPU_RESULT_*` result code.
    pub result: u32,
    /// Stable `GPU_DEVICE_STATE_*` value.
    pub device_state: u32,
    /// Bitset of `GPU_EXECUTION_SUPPORT_*` values.
    pub execution_support: u32,
    /// Maximum generic opaque command size, or zero when no command operation exists.
    pub max_opaque_command_size: u32,
    /// Reserved for ABI-compatible future use. Must be zero.
    pub reserved: u32,
    /// Backend-defined negotiated feature bits with backend-specific meaning.
    pub backend_feature_bits: u64,
    /// Number of meaningful bytes in `backend_id`.
    pub backend_id_len: u32,
    /// Number of meaningful bytes in `backend_info`.
    pub backend_info_len: u32,
    /// Opaque backend or dialect identifier, not NUL-terminated.
    pub backend_id: [u8; GPU_BACKEND_ID_BYTES],
    /// Opaque backend-defined information bytes.
    pub backend_info: [u8; GPU_BACKEND_INFO_BYTES],
}

impl GpuQueryInfo {
    /// Create a query request for the current ABI version.
    ///
    /// # Returns
    ///
    /// A zeroed request structure with `abi_version` set.
    pub const fn new() -> Self {
        Self {
            abi_version: GPU_ABI_VERSION,
            result: GPU_RESULT_SUCCESS,
            device_state: GPU_DEVICE_STATE_UNAVAILABLE,
            execution_support: GPU_EXECUTION_SUPPORT_NONE,
            max_opaque_command_size: 0,
            reserved: 0,
            backend_feature_bits: 0,
            backend_id_len: 0,
            backend_info_len: 0,
            backend_id: [0; GPU_BACKEND_ID_BYTES],
            backend_info: [0; GPU_BACKEND_INFO_BYTES],
        }
    }

    /// Return the meaningful opaque backend identifier bytes.
    ///
    /// # Returns
    ///
    /// A bounded slice of `backend_id`.
    pub fn backend_id_bytes(&self) -> &[u8] {
        &self.backend_id[..(self.backend_id_len as usize).min(GPU_BACKEND_ID_BYTES)]
    }

    /// Return the meaningful opaque backend information bytes.
    ///
    /// # Returns
    ///
    /// A bounded slice of `backend_info`.
    pub fn backend_info_bytes(&self) -> &[u8] {
        &self.backend_info[..(self.backend_info_len as usize).min(GPU_BACKEND_INFO_BYTES)]
    }
}

impl Default for GpuQueryInfo {
    fn default() -> Self {
        Self::new()
    }
}

/// Fixed-width request and response for [`commands::GPU_CREATE_BUFFER`].
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GpuCreateBuffer {
    /// ABI version supplied by userspace and echoed by the kernel.
    pub abi_version: u32,
    /// Explicit `GPU_RESULT_*` result code.
    pub result: u32,
    /// `GPU_BUFFER_FLAG_*` creation flags.
    pub flags: u32,
    /// Reserved for ABI-compatible future use. Must be zero.
    pub reserved: u32,
    /// Requested logical buffer size in bytes.
    pub size_bytes: u64,
    /// Newly created child handle on success, otherwise zero.
    pub buffer_handle: u32,
    /// Whether the resulting buffer exposes CPU mapping capability.
    pub cpu_visible: u32,
    /// Page-rounded allocation size backing the child object.
    pub allocated_size: u64,
}

impl GpuCreateBuffer {
    /// Create a buffer request for the current ABI version.
    pub const fn new(size_bytes: u64, flags: u32) -> Self {
        Self {
            abi_version: GPU_ABI_VERSION,
            result: GPU_RESULT_SUCCESS,
            flags,
            reserved: 0,
            size_bytes,
            buffer_handle: 0,
            cpu_visible: 0,
            allocated_size: 0,
        }
    }
}

/// Fixed-width response for [`commands::GPU_BUFFER_QUERY_INFO`].
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GpuBufferInfo {
    /// ABI version supplied by userspace and echoed by the kernel.
    pub abi_version: u32,
    /// Explicit `GPU_RESULT_*` result code.
    pub result: u32,
    /// Creation flags retained by the buffer.
    pub flags: u32,
    /// Whether the buffer exposes CPU mapping capability.
    pub cpu_visible: u32,
    /// Page-rounded buffer size in bytes.
    pub size_bytes: u64,
    /// Reserved for ABI-compatible future use. Always zero.
    pub reserved: u64,
}

impl GpuBufferInfo {
    /// Create a zeroed buffer query for the current ABI version.
    pub const fn new() -> Self {
        Self {
            abi_version: GPU_ABI_VERSION,
            result: GPU_RESULT_SUCCESS,
            flags: 0,
            cpu_visible: 0,
            size_bytes: 0,
            reserved: 0,
        }
    }
}

impl Default for GpuBufferInfo {
    fn default() -> Self {
        Self::new()
    }
}

/// Fixed-width request and response for [`commands::GPU_CREATE_TIMELINE`].
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GpuCreateTimeline {
    /// ABI version supplied by userspace and echoed by the kernel.
    pub abi_version: u32,
    /// Explicit `GPU_RESULT_*` result code.
    pub result: u32,
    /// Reserved creation flags. Must be zero in this phase.
    pub flags: u32,
    /// Reserved for ABI-compatible future use. Must be zero.
    pub reserved: u32,
    /// Initial completed timeline value.
    pub initial_value: u64,
    /// Newly created child handle on success, otherwise zero.
    pub timeline_handle: u32,
    /// Whether the timeline is failed at creation. Always zero.
    pub failed: u32,
    /// Current completed timeline value after creation.
    pub current_value: u64,
}

impl GpuCreateTimeline {
    /// Create a timeline request for the current ABI version.
    pub const fn new(initial_value: u64) -> Self {
        Self {
            abi_version: GPU_ABI_VERSION,
            result: GPU_RESULT_SUCCESS,
            flags: 0,
            reserved: 0,
            initial_value,
            timeline_handle: 0,
            failed: 0,
            current_value: 0,
        }
    }
}

/// Fixed-width response for [`commands::GPU_TIMELINE_QUERY`].
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GpuTimelineInfo {
    /// ABI version supplied by userspace and echoed by the kernel.
    pub abi_version: u32,
    /// Explicit `GPU_RESULT_*` result code.
    pub result: u32,
    /// Non-zero when the timeline is permanently failed.
    pub failed: u32,
    /// Reserved for ABI-compatible future use. Always zero.
    pub reserved: u32,
    /// Current completed timeline value.
    pub current_value: u64,
    /// Reserved for ABI-compatible future use. Always zero.
    pub reserved2: u64,
}

impl GpuTimelineInfo {
    /// Create a zeroed timeline query for the current ABI version.
    pub const fn new() -> Self {
        Self {
            abi_version: GPU_ABI_VERSION,
            result: GPU_RESULT_SUCCESS,
            failed: 0,
            reserved: 0,
            current_value: 0,
            reserved2: 0,
        }
    }
}

impl Default for GpuTimelineInfo {
    fn default() -> Self {
        Self::new()
    }
}

/// Fixed-width request and response for [`commands::GPU_TIMELINE_SIGNAL`].
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GpuTimelineSignal {
    /// ABI version supplied by userspace and echoed by the kernel.
    pub abi_version: u32,
    /// Explicit `GPU_RESULT_*` result code.
    pub result: u32,
    /// Requested completed timeline value.
    pub value: u64,
    /// Completed timeline value after the request.
    pub current_value: u64,
    /// Non-zero when the timeline is permanently failed.
    pub failed: u32,
    /// Reserved for ABI-compatible future use. Always zero.
    pub reserved: u32,
}

impl GpuTimelineSignal {
    /// Create a timeline signal request for the current ABI version.
    pub const fn new(value: u64) -> Self {
        Self {
            abi_version: GPU_ABI_VERSION,
            result: GPU_RESULT_SUCCESS,
            value,
            current_value: 0,
            failed: 0,
            reserved: 0,
        }
    }
}

/// Fixed-width request and response for [`commands::GPU_TIMELINE_FAIL`].
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GpuTimelineFail {
    /// ABI version supplied by userspace and echoed by the kernel.
    pub abi_version: u32,
    /// Explicit `GPU_RESULT_*` result code.
    pub result: u32,
    /// Completed timeline value after the request.
    pub current_value: u64,
    /// Non-zero when the timeline is permanently failed.
    pub failed: u32,
    /// Reserved for ABI-compatible future use. Must be zero.
    pub reserved: u32,
}

impl GpuTimelineFail {
    /// Create a timeline failure request for the current ABI version.
    pub const fn new() -> Self {
        Self {
            abi_version: GPU_ABI_VERSION,
            result: GPU_RESULT_SUCCESS,
            current_value: 0,
            failed: 0,
            reserved: 0,
        }
    }
}

/// Fixed-width request and response for [`commands::GPU_TIMELINE_CREATE_POINT`].
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GpuTimelineCreatePoint {
    /// ABI version supplied by userspace and echoed by the kernel.
    pub abi_version: u32,
    /// Explicit `GPU_RESULT_*` result code.
    pub result: u32,
    /// Fixed completed-value target for the point.
    pub target_value: u64,
    /// Newly created point child handle on success, otherwise zero.
    pub point_handle: u32,
    /// Non-zero when the parent timeline is permanently failed.
    pub failed: u32,
    /// Current completed value of the parent timeline.
    pub current_value: u64,
}

impl GpuTimelineCreatePoint {
    /// Create a timeline point request for the current ABI version.
    pub const fn new(target_value: u64) -> Self {
        Self {
            abi_version: GPU_ABI_VERSION,
            result: GPU_RESULT_SUCCESS,
            target_value,
            point_handle: 0,
            failed: 0,
            current_value: 0,
        }
    }
}

/// GPU control connection wrapper.
pub struct Gpu {
    file: File,
}

impl Gpu {
    /// Open a GPU control device.
    ///
    /// # Arguments
    ///
    /// * `path` - Device path such as `/dev/gpu0`.
    ///
    /// # Returns
    ///
    /// An independent GPU connection or a handle error.
    pub fn open(path: &str) -> HandleResult<Self> {
        let file = File::open(path).map_err(|_| HandleError::NotFound)?;
        Ok(Self { file })
    }

    /// Query stable device information and opaque backend information.
    ///
    /// # Returns
    ///
    /// Fixed-width query information. Inspect `result` for request-level errors.
    pub fn query_info(&self) -> HandleResult<GpuQueryInfo> {
        let mut info = GpuQueryInfo::new();
        self.file
            .as_handle()
            .control(commands::GPU_QUERY_INFO, &mut info as *mut _ as usize)?;
        Ok(info)
    }

    /// Create a kernel-owned GPU buffer child object.
    ///
    /// # Arguments
    ///
    /// * `size_bytes` - Requested non-zero buffer size.
    /// * `flags` - `GPU_BUFFER_FLAG_*` creation flags.
    ///
    /// # Returns
    ///
    /// An owning buffer wrapper or a handle error.
    pub fn create_buffer(&self, size_bytes: u64, flags: u32) -> HandleResult<GpuBuffer> {
        let mut request = GpuCreateBuffer::new(size_bytes, flags);
        self.file
            .as_handle()
            .control(commands::GPU_CREATE_BUFFER, &mut request as *mut _ as usize)?;
        result_to_handle_error(request.result)?;
        let handle = adopt_child_handle(request.buffer_handle)?;
        Ok(GpuBuffer {
            handle,
            allocated_size: request.allocated_size,
            flags: request.flags,
        })
    }

    /// Create a kernel-owned GPU timeline child object.
    ///
    /// # Arguments
    ///
    /// * `initial_value` - Initial completed timeline value.
    ///
    /// # Returns
    /// An owning timeline wrapper or a handle error.
    pub fn create_timeline(&self, initial_value: u64) -> HandleResult<GpuTimeline> {
        let mut request = GpuCreateTimeline::new(initial_value);
        self.file.as_handle().control(
            commands::GPU_CREATE_TIMELINE,
            &mut request as *mut _ as usize,
        )?;
        result_to_handle_error(request.result)?;
        Ok(GpuTimeline {
            handle: adopt_child_handle(request.timeline_handle)?,
        })
    }

    /// Return the underlying connection handle.
    ///
    /// # Returns
    /// A borrowed RAII handle for advanced control operations.
    pub fn as_handle(&self) -> &Handle {
        self.file.as_handle()
    }
}

/// Owning RAII wrapper for a connection-created GPU buffer child handle.
pub struct GpuBuffer {
    handle: Handle,
    allocated_size: u64,
    flags: u32,
}

impl GpuBuffer {
    /// Query the current immutable buffer details.
    ///
    /// # Returns
    /// Fixed-width buffer information or a handle error.
    pub fn query_info(&self) -> HandleResult<GpuBufferInfo> {
        let mut info = GpuBufferInfo::new();
        self.handle.control(
            commands::GPU_BUFFER_QUERY_INFO,
            &mut info as *mut _ as usize,
        )?;
        result_to_handle_error(info.result)?;
        Ok(info)
    }

    /// Return the page-rounded allocation size reported at creation.
    ///
    /// # Returns
    ///
    /// The buffer's stable backing size in bytes.
    pub const fn allocated_size(&self) -> u64 {
        self.allocated_size
    }

    /// Return the creation flags reported at creation.
    ///
    /// # Returns
    ///
    /// The original `GPU_BUFFER_FLAG_*` bits.
    pub const fn flags(&self) -> u32 {
        self.flags
    }

    /// Return whether this buffer exposes CPU mapping capability.
    ///
    /// # Returns
    ///
    /// `true` when CPU-visible memory was requested at creation.
    pub const fn cpu_visible(&self) -> bool {
        self.flags & GPU_BUFFER_FLAG_CPU_VISIBLE != 0
    }

    /// Return the underlying child handle.
    ///
    /// # Returns
    ///
    /// A borrowed owning-handle wrapper for advanced operations.
    pub fn as_handle(&self) -> &Handle {
        &self.handle
    }

    /// Consume this wrapper and return its owning handle.
    ///
    /// # Returns
    ///
    /// The RAII handle previously owned by this wrapper.
    pub fn into_handle(self) -> Handle {
        self.handle
    }
}

/// Owning RAII wrapper for a connection-created GPU timeline child handle.
pub struct GpuTimeline {
    handle: Handle,
}

impl GpuTimeline {
    /// Query the timeline's completed value and sticky failure state.
    ///
    /// # Returns
    ///
    /// Fixed-width timeline information or a handle error.
    pub fn query(&self) -> HandleResult<GpuTimelineInfo> {
        let mut info = GpuTimelineInfo::new();
        self.handle
            .control(commands::GPU_TIMELINE_QUERY, &mut info as *mut _ as usize)?;
        result_to_handle_error(info.result)?;
        Ok(info)
    }

    /// Advance this timeline to a non-decreasing completed value.
    ///
    /// # Arguments
    ///
    /// * `value` - New completed value, which may not decrease the timeline.
    ///
    /// # Returns
    /// Fixed-width signal results or a handle error.
    pub fn signal(&self, value: u64) -> HandleResult<GpuTimelineSignal> {
        let mut request = GpuTimelineSignal::new(value);
        self.handle.control(
            commands::GPU_TIMELINE_SIGNAL,
            &mut request as *mut _ as usize,
        )?;
        result_to_handle_error(request.result)?;
        Ok(request)
    }

    /// Mark this timeline permanently failed.
    ///
    /// # Returns
    /// Fixed-width failure results or a handle error.
    pub fn fail(&self) -> HandleResult<GpuTimelineFail> {
        let mut request = GpuTimelineFail::new();
        self.handle
            .control(commands::GPU_TIMELINE_FAIL, &mut request as *mut _ as usize)?;
        result_to_handle_error(request.result)?;
        Ok(request)
    }

    /// Create a fixed-target, selectable timeline point child object.
    ///
    /// # Arguments
    ///
    /// * `target_value` - Completed timeline value that makes the point ready.
    ///
    /// # Returns
    /// An owning selectable point wrapper or a handle error.
    pub fn create_point(&self, target_value: u64) -> HandleResult<GpuTimelinePoint> {
        let mut request = GpuTimelineCreatePoint::new(target_value);
        self.handle.control(
            commands::GPU_TIMELINE_CREATE_POINT,
            &mut request as *mut _ as usize,
        )?;
        result_to_handle_error(request.result)?;
        Ok(GpuTimelinePoint {
            handle: adopt_child_handle(request.point_handle)?,
            target_value,
        })
    }

    /// Return the underlying child handle.
    ///
    /// # Returns
    /// A borrowed owning-handle wrapper for advanced operations.
    pub fn as_handle(&self) -> &Handle {
        &self.handle
    }

    /// Consume this wrapper and return its owning handle.
    ///
    /// # Returns
    /// The RAII handle previously owned by this wrapper.
    pub fn into_handle(self) -> Handle {
        self.handle
    }
}

/// Owning RAII wrapper for a fixed-target GPU timeline point child handle.
pub struct GpuTimelinePoint {
    handle: Handle,
    target_value: u64,
}

impl GpuTimelinePoint {
    /// Return this point's fixed timeline target.
    ///
    /// # Returns
    /// The completed timeline value that makes the point ready.
    pub const fn target_value(&self) -> u64 {
        self.target_value
    }

    /// Return the underlying selectable child handle.
    ///
    /// # Returns
    /// A borrowed owning-handle wrapper for poll/select integration.
    pub fn as_handle(&self) -> &Handle {
        &self.handle
    }

    /// Consume this wrapper and return its owning handle.
    ///
    /// # Returns
    /// The RAII handle previously owned by this wrapper.
    pub fn into_handle(self) -> Handle {
        self.handle
    }
}

fn adopt_child_handle(raw: u32) -> HandleResult<Handle> {
    // SAFETY: A successful GPU create control response contains a newly inserted
    // handle in the caller's handle table. `Handle::from_raw` verifies it with
    // kernel introspection and closes it if that verification fails.
    unsafe { Handle::from_raw(raw as i32) }
}

fn result_to_handle_error(result: u32) -> HandleResult<()> {
    match result {
        GPU_RESULT_SUCCESS => Ok(()),
        GPU_RESULT_INVALID_ABI | GPU_RESULT_INVALID_ARGUMENT => Err(HandleError::InvalidParameter),
        GPU_RESULT_OUT_OF_RESOURCES => Err(HandleError::OutOfResources),
        GPU_RESULT_INVALID_STATE => Err(HandleError::Unsupported),
        _ => Err(HandleError::SystemError(result as i32)),
    }
}
