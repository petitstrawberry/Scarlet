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
/// Control command that queries one backend-defined execution dialect.
pub const GPU_QUERY_DIALECT: u32 = 0x4758;
/// Control command that creates a GPU execution context child handle.
pub const GPU_CREATE_CONTEXT: u32 = 0x4759;
/// Control command that queries a GPU execution context child handle.
pub const GPU_CONTEXT_QUERY: u32 = 0x475a;
/// Control command that creates a GPU execution queue child handle.
pub const GPU_CREATE_QUEUE: u32 = 0x475b;
/// Control command that queries a GPU execution queue child handle.
pub const GPU_QUEUE_QUERY: u32 = 0x475c;
/// Control command that synchronously submits opaque commands to a GPU queue.
pub const GPU_QUEUE_SUBMIT: u32 = 0x475d;
/// Control command that creates a backend-owned GPU image child handle.
pub const GPU_CREATE_IMAGE: u32 = 0x475e;
/// Control command that queries a GPU image child handle.
pub const GPU_IMAGE_QUERY_INFO: u32 = 0x475f;
/// Control command that attaches a GPU image to an execution context.
pub const GPU_CONTEXT_ATTACH_IMAGE: u32 = 0x4760;

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
/// The requested backend operation is not available.
pub const GPU_RESULT_UNSUPPORTED: u32 = 5;

/// Create buffer flag permitting CPU memory mappings.
pub const GPU_BUFFER_FLAG_CPU_VISIBLE: u32 = 1 << 0;
/// All currently defined GPU buffer creation flags.
pub const GPU_BUFFER_FLAGS_VALID: u32 = GPU_BUFFER_FLAG_CPU_VISIBLE;

/// Fixed byte capacity of a backend or dialect identifier.
pub const GPU_BACKEND_ID_BYTES: usize = 32;
/// Fixed byte capacity of backend-defined opaque information.
pub const GPU_BACKEND_INFO_BYTES: usize = 64;
/// Fixed byte capacity of opaque backend-defined dialect information.
pub const GPU_DIALECT_INFO_BYTES: usize = 256;
/// Maximum command stream length accepted by the generic queue ABI.
pub const GPU_MAX_OPAQUE_COMMAND_SIZE: u32 = 64 * 1024;

/// Submit flag requesting a timeline update after successful backend completion.
pub const GPU_QUEUE_SUBMIT_FLAG_SIGNAL_TIMELINE: u32 = 1 << 0;
/// All currently defined GPU queue submission flags.
pub const GPU_QUEUE_SUBMIT_FLAGS_VALID: u32 = GPU_QUEUE_SUBMIT_FLAG_SIGNAL_TIMELINE;

/// Generic BGRA8 normalized unsigned image format.
pub const GPU_IMAGE_FORMAT_BGRA8_UNORM: u32 = 1;
/// Image usage permitting the image to be bound as a render target.
pub const GPU_IMAGE_USAGE_RENDER_TARGET: u32 = 1 << 0;
/// Image usage permitting the image to be selected for display scanout.
pub const GPU_IMAGE_USAGE_PRESENTABLE: u32 = 1 << 1;
/// All currently defined GPU image usage flags.
pub const GPU_IMAGE_USAGE_VALID: u32 = GPU_IMAGE_USAGE_RENDER_TARGET | GPU_IMAGE_USAGE_PRESENTABLE;

/// Fixed-width request and response for [`GPU_CREATE_IMAGE`].
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GpuCreateImage {
    /// ABI version supplied by userspace and echoed by the kernel.
    pub abi_version: u32,
    /// Explicit `GPU_RESULT_*` result code.
    pub result: u32,
    /// Generic `GPU_IMAGE_FORMAT_*` image format.
    pub format: u32,
    /// `GPU_IMAGE_USAGE_*` image usage flags.
    pub usage: u32,
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// Newly created child handle on success, otherwise zero.
    pub image_handle: u32,
    /// Reserved for ABI-compatible future use. Must be zero.
    pub reserved: u32,
    /// Opaque backend command resource token for the image.
    pub command_resource_token: u64,
    /// Backing allocation size in bytes.
    pub allocation_size: u64,
}

impl GpuCreateImage {
    /// Create an image request for the current ABI version.
    ///
    /// # Arguments
    ///
    /// * `width` - Requested non-zero image width in pixels.
    /// * `height` - Requested non-zero image height in pixels.
    ///
    /// # Returns
    ///
    /// A BGRA8 render-target and presentable image request.
    pub const fn new(width: u32, height: u32) -> Self {
        Self {
            abi_version: GPU_ABI_VERSION,
            result: GPU_RESULT_SUCCESS,
            format: GPU_IMAGE_FORMAT_BGRA8_UNORM,
            usage: GPU_IMAGE_USAGE_VALID,
            width,
            height,
            image_handle: 0,
            reserved: 0,
            command_resource_token: 0,
            allocation_size: 0,
        }
    }

    /// Clear response fields while preserving request fields.
    pub(crate) fn clear_response(&mut self) {
        self.result = GPU_RESULT_SUCCESS;
        self.image_handle = 0;
        self.command_resource_token = 0;
        self.allocation_size = 0;
    }
}

/// Fixed-width response for [`GPU_IMAGE_QUERY_INFO`].
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GpuImageInfo {
    /// ABI version supplied by userspace and echoed by the kernel.
    pub abi_version: u32,
    /// Explicit `GPU_RESULT_*` result code.
    pub result: u32,
    /// Generic `GPU_IMAGE_FORMAT_*` image format.
    pub format: u32,
    /// `GPU_IMAGE_USAGE_*` image usage flags.
    pub usage: u32,
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// Opaque backend command resource token for the image.
    pub command_resource_token: u64,
    /// Backing allocation size in bytes.
    pub allocation_size: u64,
}

impl GpuImageInfo {
    /// Create a zeroed image query for the current ABI version.
    ///
    /// # Returns
    ///
    /// A zeroed image query structure.
    pub const fn new() -> Self {
        Self {
            abi_version: GPU_ABI_VERSION,
            result: GPU_RESULT_SUCCESS,
            format: 0,
            usage: 0,
            width: 0,
            height: 0,
            command_resource_token: 0,
            allocation_size: 0,
        }
    }

    /// Clear response fields while preserving the ABI version.
    pub(crate) fn clear_response(&mut self) {
        self.result = GPU_RESULT_SUCCESS;
        self.format = 0;
        self.usage = 0;
        self.width = 0;
        self.height = 0;
        self.command_resource_token = 0;
        self.allocation_size = 0;
    }
}

impl Default for GpuImageInfo {
    fn default() -> Self {
        Self::new()
    }
}

/// Fixed-width request and response for [`GPU_CONTEXT_ATTACH_IMAGE`].
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GpuContextAttachImage {
    /// ABI version supplied by userspace and echoed by the kernel.
    pub abi_version: u32,
    /// Explicit `GPU_RESULT_*` result code.
    pub result: u32,
    /// Existing GPU image child handle to attach.
    pub image_handle: u32,
    /// Reserved attachment flags. Must be zero.
    pub flags: u32,
    /// Opaque command resource token authorized for this context on success.
    pub command_resource_token: u64,
    /// Reserved for ABI-compatible future use. Must be zero.
    pub reserved: u64,
}

impl GpuContextAttachImage {
    /// Create an image attachment request for the current ABI version.
    ///
    /// # Arguments
    ///
    /// * `image_handle` - Existing image capability handle.
    ///
    /// # Returns
    ///
    /// A zeroed image attachment request.
    pub const fn new(image_handle: u32) -> Self {
        Self {
            abi_version: GPU_ABI_VERSION,
            result: GPU_RESULT_SUCCESS,
            image_handle,
            flags: 0,
            command_resource_token: 0,
            reserved: 0,
        }
    }

    /// Clear response fields while preserving request fields.
    pub(crate) fn clear_response(&mut self) {
        self.result = GPU_RESULT_SUCCESS;
        self.command_resource_token = 0;
    }
}

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

/// Fixed-width request and response for [`GPU_QUERY_DIALECT`].
///
/// A dialect token is opaque, backend-defined query data. It is not a
/// capability and does not grant authority over a GPU object.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GpuQueryDialect {
    /// ABI version supplied by userspace and echoed by the kernel.
    pub abi_version: u32,
    /// Explicit `GPU_RESULT_*` result code.
    pub result: u32,
    /// Reserved query flags. Must be zero.
    pub flags: u32,
    /// Reserved for ABI-compatible future use. Must be zero.
    pub reserved: u32,
    /// Backend-defined dialect index to query.
    pub dialect_index: u32,
    /// Reserved for ABI-compatible future use. Must be zero.
    pub reserved2: u32,
    /// Opaque backend token describing the queried dialect.
    pub dialect_token: u64,
    /// Number of meaningful bytes in `dialect_info`.
    pub dialect_info_len: u32,
    /// Reserved for ABI-compatible future use. Always zero.
    pub reserved3: u32,
    /// Opaque backend-defined dialect information bytes.
    pub dialect_info: [u8; GPU_DIALECT_INFO_BYTES],
}

impl GpuQueryDialect {
    /// Create a zeroed dialect query for the current ABI version.
    ///
    /// # Arguments
    ///
    /// * `dialect_index` - Backend-defined dialect index to query.
    ///
    /// # Returns
    ///
    /// A zeroed dialect query structure.
    pub const fn new(dialect_index: u32) -> Self {
        Self {
            abi_version: GPU_ABI_VERSION,
            result: GPU_RESULT_SUCCESS,
            flags: 0,
            reserved: 0,
            dialect_index,
            reserved2: 0,
            dialect_token: 0,
            dialect_info_len: 0,
            reserved3: 0,
            dialect_info: [0; GPU_DIALECT_INFO_BYTES],
        }
    }

    /// Clear response fields while preserving request fields.
    pub(crate) fn clear_response(&mut self) {
        self.result = GPU_RESULT_SUCCESS;
        self.dialect_token = 0;
        self.dialect_info_len = 0;
        self.reserved3 = 0;
        self.dialect_info = [0; GPU_DIALECT_INFO_BYTES];
    }
}

impl Default for GpuQueryDialect {
    fn default() -> Self {
        Self::new(0)
    }
}

/// Fixed-width request and response for [`GPU_CREATE_CONTEXT`].
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GpuCreateContext {
    /// ABI version supplied by userspace and echoed by the kernel.
    pub abi_version: u32,
    /// Explicit `GPU_RESULT_*` result code.
    pub result: u32,
    /// Reserved context creation flags. Must be zero.
    pub flags: u32,
    /// Reserved for ABI-compatible future use. Must be zero.
    pub reserved: u32,
    /// Requested backend-defined dialect index.
    pub dialect_index: u32,
    /// Newly created context child handle on success, otherwise zero.
    pub context_handle: u32,
    /// Opaque token previously reported for the requested dialect.
    pub requested_dialect_token: u64,
    /// Effective backend-defined dialect index selected for the context.
    pub effective_dialect_index: u32,
    /// Reserved for ABI-compatible future use. Always zero.
    pub reserved2: u32,
    /// Opaque token for the dialect actually selected by the backend.
    pub effective_dialect_token: u64,
}

impl GpuCreateContext {
    /// Create a zeroed context request for the current ABI version.
    ///
    /// # Arguments
    ///
    /// * `dialect_index` - Requested backend-defined dialect index.
    /// * `requested_dialect_token` - Opaque dialect data returned by a prior query.
    ///
    /// # Returns
    ///
    /// A zeroed context request structure.
    pub const fn new(dialect_index: u32, requested_dialect_token: u64) -> Self {
        Self {
            abi_version: GPU_ABI_VERSION,
            result: GPU_RESULT_SUCCESS,
            flags: 0,
            reserved: 0,
            dialect_index,
            context_handle: 0,
            requested_dialect_token,
            effective_dialect_index: 0,
            reserved2: 0,
            effective_dialect_token: 0,
        }
    }

    /// Clear response fields while preserving request fields.
    pub(crate) fn clear_response(&mut self) {
        self.result = GPU_RESULT_SUCCESS;
        self.context_handle = 0;
        self.effective_dialect_index = 0;
        self.reserved2 = 0;
        self.effective_dialect_token = 0;
    }
}

/// Fixed-width response for [`GPU_CONTEXT_QUERY`].
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GpuContextInfo {
    /// ABI version supplied by userspace and echoed by the kernel.
    pub abi_version: u32,
    /// Explicit `GPU_RESULT_*` result code.
    pub result: u32,
    /// Effective backend-defined dialect index selected for this context.
    pub effective_dialect_index: u32,
    /// Reserved for ABI-compatible future use. Must be zero.
    pub reserved: u32,
    /// Opaque token for the dialect actually selected by the backend.
    pub effective_dialect_token: u64,
    /// Reserved for ABI-compatible future use. Always zero.
    pub reserved2: u64,
}

impl GpuContextInfo {
    /// Create a zeroed context query for the current ABI version.
    ///
    /// # Returns
    ///
    /// A zeroed context query structure.
    pub const fn new() -> Self {
        Self {
            abi_version: GPU_ABI_VERSION,
            result: GPU_RESULT_SUCCESS,
            effective_dialect_index: 0,
            reserved: 0,
            effective_dialect_token: 0,
            reserved2: 0,
        }
    }

    /// Clear response fields while preserving the ABI version.
    pub(crate) fn clear_response(&mut self) {
        self.result = GPU_RESULT_SUCCESS;
        self.effective_dialect_index = 0;
        self.reserved = 0;
        self.effective_dialect_token = 0;
        self.reserved2 = 0;
    }
}

impl Default for GpuContextInfo {
    fn default() -> Self {
        Self::new()
    }
}

/// Fixed-width request and response for [`GPU_CREATE_QUEUE`].
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GpuCreateQueue {
    /// ABI version supplied by userspace and echoed by the kernel.
    pub abi_version: u32,
    /// Explicit `GPU_RESULT_*` result code.
    pub result: u32,
    /// Reserved queue creation flags. Must be zero.
    pub flags: u32,
    /// Reserved for ABI-compatible future use. Must be zero.
    pub reserved: u32,
    /// Newly created queue child handle on success, otherwise zero.
    pub queue_handle: u32,
    /// Maximum opaque command size accepted by the queue.
    pub max_opaque_command_size: u32,
    /// Reserved for ABI-compatible future use. Always zero.
    pub reserved2: u64,
}

impl GpuCreateQueue {
    /// Create a zeroed queue request for the current ABI version.
    ///
    /// # Returns
    ///
    /// A zeroed queue request structure.
    pub const fn new() -> Self {
        Self {
            abi_version: GPU_ABI_VERSION,
            result: GPU_RESULT_SUCCESS,
            flags: 0,
            reserved: 0,
            queue_handle: 0,
            max_opaque_command_size: 0,
            reserved2: 0,
        }
    }

    /// Clear response fields while preserving request fields.
    pub(crate) fn clear_response(&mut self) {
        self.result = GPU_RESULT_SUCCESS;
        self.queue_handle = 0;
        self.max_opaque_command_size = 0;
        self.reserved2 = 0;
    }
}

/// Fixed-width response for [`GPU_QUEUE_QUERY`].
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GpuQueueInfo {
    /// ABI version supplied by userspace and echoed by the kernel.
    pub abi_version: u32,
    /// Explicit `GPU_RESULT_*` result code.
    pub result: u32,
    /// Maximum opaque command size accepted by this queue.
    pub max_opaque_command_size: u32,
    /// Reserved for ABI-compatible future use. Must be zero.
    pub reserved: u32,
    /// Reserved for ABI-compatible future use. Always zero.
    pub reserved2: u64,
}

impl GpuQueueInfo {
    /// Create a zeroed queue query for the current ABI version.
    ///
    /// # Returns
    ///
    /// A zeroed queue query structure.
    pub const fn new() -> Self {
        Self {
            abi_version: GPU_ABI_VERSION,
            result: GPU_RESULT_SUCCESS,
            max_opaque_command_size: 0,
            reserved: 0,
            reserved2: 0,
        }
    }

    /// Clear response fields while preserving the ABI version.
    pub(crate) fn clear_response(&mut self) {
        self.result = GPU_RESULT_SUCCESS;
        self.max_opaque_command_size = 0;
        self.reserved = 0;
        self.reserved2 = 0;
    }
}

impl Default for GpuQueueInfo {
    fn default() -> Self {
        Self::new()
    }
}

/// Fixed-width request and response for [`GPU_QUEUE_SUBMIT`].
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GpuQueueSubmit {
    /// ABI version supplied by userspace and echoed by the kernel.
    pub abi_version: u32,
    /// Explicit `GPU_RESULT_*` result code.
    pub result: u32,
    /// `GPU_QUEUE_SUBMIT_FLAG_*` submission flags.
    pub flags: u32,
    /// Reserved for ABI-compatible future use. Must be zero.
    pub reserved: u32,
    /// Userspace address of opaque command bytes.
    pub command_ptr: u64,
    /// Number of opaque command bytes to copy and submit.
    pub command_size: u32,
    /// Existing GPU timeline handle used when timeline signalling is requested.
    pub signal_timeline_handle: u32,
    /// Value to signal after successful fenced backend completion.
    pub signal_value: u64,
    /// Completed value after the submission result is determined.
    pub completed_value: u64,
    /// Non-zero when the requested timeline is permanently failed.
    pub timeline_failed: u32,
    /// Reserved for ABI-compatible future use. Must be zero.
    pub reserved2: u32,
}

impl GpuQueueSubmit {
    /// Create a queue submission request for the current ABI version.
    ///
    /// # Arguments
    ///
    /// * `command_ptr` - Userspace address of opaque command bytes.
    /// * `command_size` - Number of bytes to copy and submit.
    ///
    /// # Returns
    ///
    /// A zeroed queue submission request structure.
    pub const fn new(command_ptr: u64, command_size: u32) -> Self {
        Self {
            abi_version: GPU_ABI_VERSION,
            result: GPU_RESULT_SUCCESS,
            flags: 0,
            reserved: 0,
            command_ptr,
            command_size,
            signal_timeline_handle: 0,
            signal_value: 0,
            completed_value: 0,
            timeline_failed: 0,
            reserved2: 0,
        }
    }

    /// Clear response fields while preserving request fields.
    pub(crate) fn clear_response(&mut self) {
        self.result = GPU_RESULT_SUCCESS;
        self.completed_value = 0;
        self.timeline_failed = 0;
        self.reserved2 = 0;
    }
}
