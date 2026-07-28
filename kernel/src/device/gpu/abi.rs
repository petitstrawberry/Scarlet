//! Fixed-width userspace ABI for GPU control connections.

/// ABI version accepted by [`GpuQueryInfo`].
pub const GPU_ABI_VERSION: u32 = 1;

/// Control command that queries generic GPU and backend information.
pub const GPU_QUERY_INFO: u32 = 0x4750;
/// Control command that creates a kernel-owned GPU buffer child handle.
pub const GPU_CREATE_BUFFER: u32 = 0x4751;
/// Control command that queries a GPU buffer child handle.
pub const GPU_BUFFER_QUERY_INFO: u32 = 0x4752;
/// Control command that creates a GPU timeline child handle.
pub const GPU_CREATE_TIMELINE: u32 = 0x4753;
/// Control command that queries a GPU timeline child handle.
pub const GPU_TIMELINE_QUERY: u32 = 0x4754;
/// Control command that advances a GPU timeline.
pub const GPU_TIMELINE_SIGNAL: u32 = 0x4755;
/// Control command that marks a GPU timeline permanently failed.
pub const GPU_TIMELINE_FAIL: u32 = 0x4756;
/// Control command that creates a fixed-target GPU timeline point child handle.
pub const GPU_TIMELINE_CREATE_POINT: u32 = 0x4757;

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

/// Fixed byte capacity of a backend or dialect identifier.
pub const GPU_BACKEND_ID_BYTES: usize = 32;
/// Fixed byte capacity of backend-defined opaque information.
pub const GPU_BACKEND_INFO_BYTES: usize = 64;

/// Fixed-width request and response for [`GPU_QUERY_INFO`].
///
/// Callers initialize this structure with [`GpuQueryInfo::new`], then pass its
/// address to the control operation. The kernel always reports request-level
/// failure through `result` when it can copy the response back to userspace.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GpuQueryInfo {
    /// ABI version supplied by userspace and echoed by the kernel.
    pub abi_version: u32,
    /// Explicit `GPU_RESULT_*` result code.
    pub result: u32,
    /// Stable numeric device state from [`super::GpuDeviceState`].
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
    /// A zeroed query structure with the supported ABI version set.
    pub const fn new() -> Self {
        Self {
            abi_version: GPU_ABI_VERSION,
            result: GPU_RESULT_SUCCESS,
            device_state: 0,
            execution_support: 0,
            max_opaque_command_size: 0,
            reserved: 0,
            backend_feature_bits: 0,
            backend_id_len: 0,
            backend_info_len: 0,
            backend_id: [0; GPU_BACKEND_ID_BYTES],
            backend_info: [0; GPU_BACKEND_INFO_BYTES],
        }
    }

    /// Clear response fields while preserving the caller-provided ABI version.
    pub(crate) fn clear_response(&mut self) {
        self.result = GPU_RESULT_SUCCESS;
        self.device_state = 0;
        self.execution_support = 0;
        self.max_opaque_command_size = 0;
        self.reserved = 0;
        self.backend_feature_bits = 0;
        self.backend_id_len = 0;
        self.backend_info_len = 0;
        self.backend_id = [0; GPU_BACKEND_ID_BYTES];
        self.backend_info = [0; GPU_BACKEND_INFO_BYTES];
    }
}

impl Default for GpuQueryInfo {
    fn default() -> Self {
        Self::new()
    }
}

/// Fixed-width request and response for [`GPU_CREATE_BUFFER`].
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
    /// Create a zeroed buffer request for the current ABI version.
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

    /// Clear response fields while preserving request fields.
    pub(crate) fn clear_response(&mut self) {
        self.result = GPU_RESULT_SUCCESS;
        self.buffer_handle = 0;
        self.cpu_visible = 0;
        self.allocated_size = 0;
    }
}

/// Fixed-width response for [`GPU_BUFFER_QUERY_INFO`].
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

    /// Clear response fields while preserving the ABI version.
    pub(crate) fn clear_response(&mut self) {
        self.result = GPU_RESULT_SUCCESS;
        self.flags = 0;
        self.cpu_visible = 0;
        self.size_bytes = 0;
        self.reserved = 0;
    }
}

impl Default for GpuBufferInfo {
    fn default() -> Self {
        Self::new()
    }
}

/// Fixed-width request and response for [`GPU_CREATE_TIMELINE`].
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
    /// Create a zeroed timeline request for the current ABI version.
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

    /// Clear response fields while preserving request fields.
    pub(crate) fn clear_response(&mut self) {
        self.result = GPU_RESULT_SUCCESS;
        self.timeline_handle = 0;
        self.failed = 0;
        self.current_value = 0;
    }
}

/// Fixed-width response for [`GPU_TIMELINE_QUERY`].
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

    /// Clear response fields while preserving the ABI version.
    pub(crate) fn clear_response(&mut self) {
        self.result = GPU_RESULT_SUCCESS;
        self.failed = 0;
        self.reserved = 0;
        self.current_value = 0;
        self.reserved2 = 0;
    }
}

impl Default for GpuTimelineInfo {
    fn default() -> Self {
        Self::new()
    }
}

/// Fixed-width request and response for [`GPU_TIMELINE_SIGNAL`].
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

    /// Clear response fields while preserving request fields.
    pub(crate) fn clear_response(&mut self) {
        self.result = GPU_RESULT_SUCCESS;
        self.current_value = 0;
        self.failed = 0;
        self.reserved = 0;
    }
}

/// Fixed-width request and response for [`GPU_TIMELINE_FAIL`].
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

    /// Clear response fields while preserving the ABI version.
    pub(crate) fn clear_response(&mut self) {
        self.result = GPU_RESULT_SUCCESS;
        self.current_value = 0;
        self.failed = 0;
        self.reserved = 0;
    }
}

/// Fixed-width request and response for [`GPU_TIMELINE_CREATE_POINT`].
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

    /// Clear response fields while preserving request fields.
    pub(crate) fn clear_response(&mut self) {
        self.result = GPU_RESULT_SUCCESS;
        self.point_handle = 0;
        self.failed = 0;
        self.current_value = 0;
    }
}
