//! Low-level userspace-driver GPU transport for Scarlet OS.
//!
//! This crate opens GPU control connections, creates capability-backed child
//! objects, and exposes the fixed-width generic driver ABI. Dialects, resource
//! tokens, and opaque command streams are driver-private transport details;
//! applications should use the higher-level `gpu` facade instead.

#![cfg_attr(not(feature = "std"), no_std)]

mod completion;
pub use completion::{
    GPU_COMPLETION_COMPLETE, GPU_COMPLETION_FAILED, GPU_COMPLETION_FAILURE_ABANDONED,
    GPU_COMPLETION_FAILURE_DEVICE_LOST, GPU_COMPLETION_FAILURE_EXECUTION,
    GPU_COMPLETION_FAILURE_NONE, GPU_COMPLETION_PENDING, GpuCompletion, GpuCompletionInfo,
};

#[cfg(not(feature = "std"))]
extern crate scarlet_std as std;

#[cfg(feature = "std")]
use scarlet_os::handle::{Handle, HandleError, HandleResult};
#[cfg(feature = "std")]
use scarlet_os::ipc::SharedMemory;
#[cfg(not(feature = "std"))]
use std::{
    fs::File,
    handle::{Handle, HandleError, HandleResult},
    ipc::SharedMemory,
};

#[cfg(feature = "std")]
struct File {
    handle: Handle,
}

#[cfg(feature = "std")]
impl File {
    fn open(path: &str) -> Result<Self, HandleError> {
        Handle::open(path, 0).map(|handle| Self { handle })
    }

    fn as_handle(&self) -> &Handle {
        &self.handle
    }
}

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
    /// Query one backend-defined execution dialect.
    pub const GPU_QUERY_DIALECT: u32 = 0x4758;
    /// Create a GPU execution context child handle.
    pub const GPU_CREATE_CONTEXT: u32 = 0x4759;
    /// Query a GPU execution context child handle.
    pub const GPU_CONTEXT_QUERY: u32 = 0x475a;
    /// Create a GPU execution queue child handle.
    pub const GPU_CREATE_QUEUE: u32 = 0x475b;
    /// Query a GPU execution queue child handle.
    pub const GPU_QUEUE_QUERY: u32 = 0x475c;
    /// Synchronously submit opaque commands to a GPU queue.
    pub const GPU_QUEUE_SUBMIT: u32 = 0x475d;
    /// Create a backend-owned GPU image child handle.
    pub const GPU_CREATE_IMAGE: u32 = 0x475e;
    /// Query a GPU image child handle.
    pub const GPU_IMAGE_QUERY_INFO: u32 = 0x475f;
    /// Attach a GPU image to an execution context.
    pub const GPU_CONTEXT_ATTACH_IMAGE: u32 = 0x4760;
    /// Attach a GPU buffer to an execution context.
    pub const GPU_CONTEXT_ATTACH_BUFFER: u32 = 0x4761;
    /// Upload BGRA pixels into an image attached to an execution context.
    pub const GPU_CONTEXT_UPLOAD_IMAGE_BGRA: u32 = 0x4762;
    /// Detach an image from an execution context.
    pub const GPU_CONTEXT_DETACH_IMAGE: u32 = 0x4763;
    /// Create a sampled BGRA image imported from SharedMemory.
    pub const GPU_CREATE_IMPORTED_IMAGE_BGRA: u32 = 0x4764;
    /// Transfer one rectangle from fixed imported image backing.
    pub const GPU_CONTEXT_TRANSFER_IMPORTED_IMAGE_BGRA: u32 = 0x4765;
    /// Query immutable image plane and modifier layout.
    pub const GPU_IMAGE_QUERY_LAYOUT: u32 = 0x4766;
    /// Detach a GPU buffer from an execution context.
    pub const GPU_CONTEXT_DETACH_BUFFER: u32 = 0x4767;
    /// Read one attached BGRA image rectangle into userspace.
    pub const GPU_CONTEXT_READBACK_IMAGE_BGRA: u32 = 0x4768;
    /// Query an authoritative read-only GPU completion handle.
    pub const GPU_COMPLETION_QUERY: u32 = 0x4769;
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
/// The requested backend operation is not available.
pub const GPU_RESULT_UNSUPPORTED: u32 = 5;

/// Create buffer flag permitting CPU memory mappings.
pub const GPU_BUFFER_FLAG_CPU_VISIBLE: u32 = 1 << 0;
/// All currently defined GPU buffer creation flags.
pub const GPU_BUFFER_FLAGS_VALID: u32 = GPU_BUFFER_FLAG_CPU_VISIBLE;

/// Generic BGRA8 normalized unsigned image format.
pub const GPU_IMAGE_FORMAT_BGRA8_UNORM: u32 = 1;
/// Generic 32-bit floating-point depth image format.
pub const GPU_IMAGE_FORMAT_DEPTH32_FLOAT: u32 = 2;
/// Maximum number of image planes returned by the generic layout ABI.
pub const GPU_IMAGE_MAX_PLANES: usize = 4;
/// Modifier value for an uncompressed linear image.
pub const GPU_IMAGE_MODIFIER_LINEAR: u64 = 0;
/// Image usage permitting the image to be bound as a render target.
pub const GPU_IMAGE_USAGE_RENDER_TARGET: u32 = 1 << 0;
/// Image usage permitting the image to be selected for display scanout.
pub const GPU_IMAGE_USAGE_PRESENTABLE: u32 = 1 << 1;
/// Image usage permitting the image to be sampled by GPU commands.
pub const GPU_IMAGE_USAGE_SAMPLED: u32 = 1 << 2;
/// Image usage permitting BGRA pixel transfers into the image.
pub const GPU_IMAGE_USAGE_TRANSFER_DST: u32 = 1 << 3;
/// Image usage permitting binding as a depth-stencil attachment.
pub const GPU_IMAGE_USAGE_DEPTH_STENCIL_ATTACHMENT: u32 = 1 << 4;
/// Image usage permitting BGRA pixel transfers out of the image.
pub const GPU_IMAGE_USAGE_TRANSFER_SRC: u32 = 1 << 5;
/// All currently defined GPU image usage flags.
pub const GPU_IMAGE_USAGE_VALID: u32 = GPU_IMAGE_USAGE_RENDER_TARGET
    | GPU_IMAGE_USAGE_PRESENTABLE
    | GPU_IMAGE_USAGE_SAMPLED
    | GPU_IMAGE_USAGE_TRANSFER_DST
    | GPU_IMAGE_USAGE_DEPTH_STENCIL_ATTACHMENT
    | GPU_IMAGE_USAGE_TRANSFER_SRC;

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
/// Generic image upload operations are available.
pub const GPU_EXECUTION_SUPPORT_IMAGE_UPLOAD: u32 = 1 << 5;
/// Generic depth attachment and depth-test operations are available.
pub const GPU_EXECUTION_SUPPORT_DEPTH: u32 = 1 << 6;
/// Generic synchronous image readback operations are available.
pub const GPU_EXECUTION_SUPPORT_IMAGE_READBACK: u32 = 1 << 7;

/// Fixed byte capacity of an opaque backend or dialect identifier.
pub const GPU_BACKEND_ID_BYTES: usize = 32;
/// Fixed byte capacity of opaque backend-defined information.
pub const GPU_BACKEND_INFO_BYTES: usize = 64;
/// Fixed byte capacity of opaque backend-defined dialect information.
pub const GPU_DIALECT_INFO_BYTES: usize = 256;
/// Maximum command stream length accepted by the generic queue ABI.
/// Absolute ABI bound for one backend-defined queue payload. The effective
/// limit must still be obtained from the selected device and queue.
pub const GPU_MAX_OPAQUE_COMMAND_SIZE: u32 = 2 * 1024 * 1024;
/// Maximum BGRA pixel payload accepted by one image upload request.
pub const GPU_MAX_IMAGE_UPLOAD_SIZE: u32 = 64 * 1024 * 1024;

/// Submit flag requesting a timeline update after successful backend completion.
pub const GPU_QUEUE_SUBMIT_FLAG_SIGNAL_TIMELINE: u32 = 1 << 0;

/// Fixed-width request and response for [`commands::GPU_CREATE_IMAGE`].
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
    /// Create a BGRA8 render-target and presentable image request.
    ///
    /// # Arguments
    ///
    /// * `width` - Requested non-zero image width in pixels.
    /// * `height` - Requested non-zero image height in pixels.
    ///
    /// # Returns
    ///
    /// A zeroed request for the current ABI version.
    pub const fn new(width: u32, height: u32) -> Self {
        Self::new_with_usage(
            width,
            height,
            GPU_IMAGE_USAGE_RENDER_TARGET | GPU_IMAGE_USAGE_PRESENTABLE,
        )
    }

    /// Create a BGRA8 image request with explicit generic usage flags.
    ///
    /// # Arguments
    ///
    /// * `width` - Requested non-zero image width in pixels.
    /// * `height` - Requested non-zero image height in pixels.
    /// * `usage` - Requested `GPU_IMAGE_USAGE_*` flags.
    ///
    /// # Returns
    ///
    /// A zeroed request for the current ABI version.
    pub const fn new_with_usage(width: u32, height: u32, usage: u32) -> Self {
        Self {
            abi_version: GPU_ABI_VERSION,
            result: GPU_RESULT_SUCCESS,
            format: GPU_IMAGE_FORMAT_BGRA8_UNORM,
            usage,
            width,
            height,
            image_handle: 0,
            reserved: 0,
            command_resource_token: 0,
            allocation_size: 0,
        }
    }
}

/// Fixed-width response for [`commands::GPU_IMAGE_QUERY_INFO`].
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
}

impl Default for GpuImageInfo {
    fn default() -> Self {
        Self::new()
    }
}

/// Fixed-width layout of one image plane.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuImagePlaneLayout {
    /// Byte offset of the first plane element in the backing allocation.
    pub offset: u64,
    /// Number of bytes occupied by the plane.
    pub size: u64,
    /// Number of bytes between adjacent block rows.
    pub row_pitch: u32,
    /// Number of bytes between adjacent array layers.
    pub array_pitch: u32,
    /// Width of one stored block in pixels.
    pub block_width: u16,
    /// Height of one stored block in pixels.
    pub block_height: u16,
    /// Number of bytes in one stored block.
    pub bytes_per_block: u16,
    /// Reserved for ABI-compatible future use. Always zero.
    pub reserved: u16,
}

impl GpuImagePlaneLayout {
    /// A zeroed unused plane entry.
    pub const EMPTY: Self = Self {
        offset: 0,
        size: 0,
        row_pitch: 0,
        array_pitch: 0,
        block_width: 0,
        block_height: 0,
        bytes_per_block: 0,
        reserved: 0,
    };
}

/// Fixed-width response for [`commands::GPU_IMAGE_QUERY_LAYOUT`].
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuImageLayout {
    /// ABI version supplied by userspace and echoed by the kernel.
    pub abi_version: u32,
    /// Explicit `GPU_RESULT_*` result code.
    pub result: u32,
    /// Backend-neutral or backend-specific immutable memory modifier.
    pub modifier: u64,
    /// Exact bytes required by all initialized image planes.
    pub total_size: u64,
    /// Required physical backing alignment in bytes.
    pub alignment: u64,
    /// Number of initialized entries in `planes`.
    pub plane_count: u32,
    /// Reserved for ABI-compatible future use. Always zero.
    pub reserved: u32,
    /// Fixed-capacity plane layout table.
    pub planes: [GpuImagePlaneLayout; GPU_IMAGE_MAX_PLANES],
}

impl GpuImageLayout {
    /// Create a zeroed image layout query for the current ABI version.
    pub const fn new() -> Self {
        Self {
            abi_version: GPU_ABI_VERSION,
            result: GPU_RESULT_SUCCESS,
            modifier: 0,
            total_size: 0,
            alignment: 0,
            plane_count: 0,
            reserved: 0,
            planes: [GpuImagePlaneLayout::EMPTY; GPU_IMAGE_MAX_PLANES],
        }
    }
}

impl Default for GpuImageLayout {
    fn default() -> Self {
        Self::new()
    }
}

/// Fixed-width request and response for [`commands::GPU_CONTEXT_ATTACH_IMAGE`].
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

/// Fixed-width request and response for [`commands::GPU_CONTEXT_ATTACH_BUFFER`].
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GpuContextAttachBuffer {
    /// ABI version supplied by userspace and echoed by the kernel.
    pub abi_version: u32,
    /// Explicit `GPU_RESULT_*` result code.
    pub result: u32,
    /// Existing GPU buffer child handle to attach.
    pub buffer_handle: u32,
    /// Reserved attachment flags. Must be zero.
    pub flags: u32,
    /// Opaque command resource token authorized for this context on success.
    pub command_resource_token: u64,
    /// Reserved for ABI-compatible future use. Must be zero.
    pub reserved: u64,
}

impl GpuContextAttachBuffer {
    /// Create a buffer attachment request for the current ABI version.
    ///
    /// # Arguments
    ///
    /// * `buffer_handle` - Existing buffer capability handle.
    ///
    /// # Returns
    ///
    /// A zeroed buffer attachment request.
    pub const fn new(buffer_handle: u32) -> Self {
        Self {
            abi_version: GPU_ABI_VERSION,
            result: GPU_RESULT_SUCCESS,
            buffer_handle,
            flags: 0,
            command_resource_token: 0,
            reserved: 0,
        }
    }
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
}

/// Fixed-width request and response for [`commands::GPU_CONTEXT_DETACH_IMAGE`].
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GpuContextDetachImage {
    /// ABI version supplied by userspace and echoed by the kernel.
    pub abi_version: u32,
    /// Explicit `GPU_RESULT_*` result code.
    pub result: u32,
    /// Existing attached GPU image child handle to detach.
    pub image_handle: u32,
    /// Reserved detachment flags. Must be zero.
    pub flags: u32,
    /// Reserved for ABI-compatible future use. Must be zero.
    pub reserved: u64,
}

impl GpuContextDetachImage {
    /// Create an image detachment request for the current ABI version.
    ///
    /// # Arguments
    ///
    /// * `image_handle` - Existing attached image capability handle.
    ///
    /// # Returns
    ///
    /// A zeroed image detachment request.
    pub const fn new(image_handle: u32) -> Self {
        Self {
            abi_version: GPU_ABI_VERSION,
            result: GPU_RESULT_SUCCESS,
            image_handle,
            flags: 0,
            reserved: 0,
        }
    }
}

/// Fixed-width request and response for [`commands::GPU_CONTEXT_DETACH_BUFFER`].
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GpuContextDetachBuffer {
    /// ABI version supplied by userspace and echoed by the kernel.
    pub abi_version: u32,
    /// Explicit `GPU_RESULT_*` result code.
    pub result: u32,
    /// Existing attached GPU buffer child handle to detach.
    pub buffer_handle: u32,
    /// Reserved detachment flags. Must be zero.
    pub flags: u32,
    /// Reserved for ABI-compatible future use. Must be zero.
    pub reserved: u64,
}

impl GpuContextDetachBuffer {
    /// Create a buffer detachment request for the current ABI version.
    ///
    /// # Arguments
    ///
    /// * `buffer_handle` - Existing attached buffer capability handle.
    ///
    /// # Returns
    ///
    /// A zeroed buffer detachment request.
    pub const fn new(buffer_handle: u32) -> Self {
        Self {
            abi_version: GPU_ABI_VERSION,
            result: GPU_RESULT_SUCCESS,
            buffer_handle,
            flags: 0,
            reserved: 0,
        }
    }
}

/// Fixed-width request and response for [`commands::GPU_CREATE_IMPORTED_IMAGE_BGRA`].
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GpuCreateImportedImageBgra {
    /// ABI version supplied by userspace and echoed by the kernel.
    pub abi_version: u32,
    /// Explicit `GPU_RESULT_*` result code.
    pub result: u32,
    /// Existing SharedMemory capability handle.
    pub shm_handle: u32,
    /// Generic `GPU_IMAGE_FORMAT_*` image format.
    pub format: u32,
    /// `GPU_IMAGE_USAGE_*` image usage flags.
    pub usage: u32,
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// Number of bytes between source rows in SharedMemory.
    pub source_stride: u32,
    /// Newly created image child handle on success, otherwise zero.
    pub image_handle: u32,
    /// Reserved for ABI-compatible future use. Must be zero.
    pub reserved: u32,
    /// Byte offset of pixel `(0, 0)` in SharedMemory.
    pub shm_offset: u64,
    /// Opaque backend command resource token for the image.
    pub command_resource_token: u64,
    /// Imported backing allocation size in bytes.
    pub allocation_size: u64,
}

impl GpuCreateImportedImageBgra {
    /// Create an imported BGRA image request for the current ABI version.
    ///
    /// # Arguments
    ///
    /// * `shm_handle` - Existing SharedMemory capability handle.
    /// * `width` - Requested non-zero image width in pixels.
    /// * `height` - Requested non-zero image height in pixels.
    /// * `shm_offset` - Byte offset of pixel `(0, 0)` in SharedMemory.
    /// * `source_stride` - Number of bytes between source rows in SharedMemory.
    ///
    /// # Returns
    ///
    /// A sampled, transfer-destination BGRA image request.
    pub const fn new(
        shm_handle: u32,
        width: u32,
        height: u32,
        shm_offset: u64,
        source_stride: u32,
    ) -> Self {
        Self {
            abi_version: GPU_ABI_VERSION,
            result: GPU_RESULT_SUCCESS,
            shm_handle,
            format: GPU_IMAGE_FORMAT_BGRA8_UNORM,
            usage: GPU_IMAGE_USAGE_SAMPLED | GPU_IMAGE_USAGE_TRANSFER_DST,
            width,
            height,
            source_stride,
            image_handle: 0,
            reserved: 0,
            shm_offset,
            command_resource_token: 0,
            allocation_size: 0,
        }
    }
}

/// Fixed-width request and response for [`commands::GPU_CONTEXT_TRANSFER_IMPORTED_IMAGE_BGRA`].
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GpuContextTransferImportedImageBgra {
    /// ABI version supplied by userspace and echoed by the kernel.
    pub abi_version: u32,
    /// Explicit `GPU_RESULT_*` result code.
    pub result: u32,
    /// Existing imported GPU image child handle attached to this context.
    pub image_handle: u32,
    /// Reserved for ABI-compatible future use. Must be zero.
    pub reserved: u32,
    /// Destination image x coordinate in pixels.
    pub dst_x: u32,
    /// Destination image y coordinate in pixels.
    pub dst_y: u32,
    /// Rectangle width in pixels.
    pub width: u32,
    /// Rectangle height in pixels.
    pub height: u32,
    /// Reserved for ABI-compatible future use. Must be zero.
    pub reserved2: u64,
}

impl GpuContextTransferImportedImageBgra {
    /// Create a pointer-free imported BGRA rectangle transfer request.
    ///
    /// # Arguments
    ///
    /// * `image_handle` - Imported image capability handle attached to the context.
    /// * `rect` - Destination rectangle in the image.
    ///
    /// # Returns
    ///
    /// A request that transfers from the image's fixed SharedMemory backing.
    pub const fn new(image_handle: u32, rect: GpuImageBgraRect) -> Self {
        Self {
            abi_version: GPU_ABI_VERSION,
            result: GPU_RESULT_SUCCESS,
            image_handle,
            reserved: 0,
            dst_x: rect.x,
            dst_y: rect.y,
            width: rect.width,
            height: rect.height,
            reserved2: 0,
        }
    }
}

/// Fixed-width request and response for [`commands::GPU_CONTEXT_READBACK_IMAGE_BGRA`].
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GpuContextReadbackImageBgra {
    /// ABI version supplied by userspace and echoed by the kernel.
    pub abi_version: u32,
    /// Explicit `GPU_RESULT_*` result code.
    pub result: u32,
    /// Existing GPU image child handle attached to this context.
    pub image_handle: u32,
    /// Reserved for ABI-compatible future use. Must be zero.
    pub reserved: u32,
    /// Userspace address of the complete destination BGRA buffer.
    pub destination_ptr: u64,
    /// Length of the complete destination userspace byte range.
    pub destination_length: u64,
    /// Number of bytes between destination rows.
    pub destination_stride: u32,
    /// Source image x coordinate in pixels.
    pub src_x: u32,
    /// Source image y coordinate in pixels.
    pub src_y: u32,
    /// Rectangle width in pixels.
    pub width: u32,
    /// Rectangle height in pixels.
    pub height: u32,
    /// Reserved for ABI-compatible future use. Must be zero.
    pub reserved2: u32,
    /// Reserved for ABI-compatible future use. Must be zero.
    pub reserved3: u64,
}

impl GpuContextReadbackImageBgra {
    /// Create a pointer-based BGRA readback request.
    ///
    /// The source rectangle is written at identical coordinates in the
    /// complete destination buffer.
    ///
    /// # Arguments
    ///
    /// * `image_handle` - Image capability attached to the context.
    /// * `destination` - Complete writable destination buffer.
    /// * `destination_stride` - Bytes between destination rows.
    /// * `rect` - Source image rectangle.
    ///
    /// # Returns
    ///
    /// A request, or an invalid-parameter error when pointer/length conversion fails.
    pub fn new(
        image_handle: u32,
        destination: &mut [u8],
        destination_stride: u32,
        rect: GpuImageBgraRect,
    ) -> HandleResult<Self> {
        Ok(Self {
            abi_version: GPU_ABI_VERSION,
            result: GPU_RESULT_SUCCESS,
            image_handle,
            reserved: 0,
            destination_ptr: u64::try_from(destination.as_mut_ptr() as usize)
                .map_err(|_| HandleError::InvalidParameter)?,
            destination_length: u64::try_from(destination.len())
                .map_err(|_| HandleError::InvalidParameter)?,
            destination_stride,
            src_x: rect.x,
            src_y: rect.y,
            width: rect.width,
            height: rect.height,
            reserved2: 0,
            reserved3: 0,
        })
    }
}

/// Typed destination rectangle for [`GpuContext::upload_image_bgra`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuImageBgraRect {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

impl GpuImageBgraRect {
    /// Create a destination BGRA image rectangle.
    ///
    /// # Arguments
    ///
    /// * `x` - Destination x coordinate in pixels.
    /// * `y` - Destination y coordinate in pixels.
    /// * `width` - Rectangle width in pixels.
    /// * `height` - Rectangle height in pixels.
    ///
    /// # Returns
    ///
    /// A typed rectangle validated by the kernel against the destination image.
    pub const fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

/// Fixed-width request and response for [`commands::GPU_CONTEXT_UPLOAD_IMAGE_BGRA`].
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GpuContextUploadImageBgra {
    /// ABI version supplied by userspace and echoed by the kernel.
    pub abi_version: u32,
    /// Explicit `GPU_RESULT_*` result code.
    pub result: u32,
    /// Existing GPU image child handle attached to the context.
    pub image_handle: u32,
    /// Reserved for ABI-compatible future use. Must be zero.
    pub reserved: u32,
    /// Userspace address of source BGRA pixels.
    pub source_ptr: u64,
    /// Length of the source userspace byte range.
    pub source_length: u64,
    /// Source row stride in bytes.
    pub source_stride: u32,
    /// Destination image x coordinate in pixels.
    pub dst_x: u32,
    /// Destination image y coordinate in pixels.
    pub dst_y: u32,
    /// Rectangle width in pixels.
    pub width: u32,
    /// Rectangle height in pixels.
    pub height: u32,
    /// Reserved for ABI-compatible future use. Must be zero.
    pub reserved2: u64,
}

impl GpuContextUploadImageBgra {
    /// Create a request borrowing one safe BGRA source slice for the control call.
    ///
    /// # Arguments
    ///
    /// * `image_handle` - Image capability handle attached to the context.
    /// * `pixels` - Source BGRA pixel bytes.
    /// * `source_stride` - Source row stride in bytes.
    /// * `rect` - Destination rectangle in the image.
    ///
    /// # Returns
    ///
    /// A fixed-width upload request, or an error when the source slice cannot
    /// contain the requested non-empty strided rectangle.
    pub fn new(
        image_handle: u32,
        pixels: &[u8],
        source_stride: u32,
        rect: GpuImageBgraRect,
    ) -> HandleResult<Self> {
        validate_bgra_upload_pixels(pixels, source_stride, rect)?;
        let source_length =
            u64::try_from(pixels.len()).map_err(|_| HandleError::InvalidParameter)?;
        Ok(Self {
            abi_version: GPU_ABI_VERSION,
            result: GPU_RESULT_SUCCESS,
            image_handle,
            reserved: 0,
            source_ptr: pixels.as_ptr() as usize as u64,
            source_length,
            source_stride,
            dst_x: rect.x,
            dst_y: rect.y,
            width: rect.width,
            height: rect.height,
            reserved2: 0,
        })
    }
}

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
    /// Opaque backend command resource token for the buffer.
    pub command_resource_token: u64,
    /// Page-rounded allocation size backing the child object.
    pub allocation_size: u64,
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
            command_resource_token: 0,
            allocation_size: 0,
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
    /// Opaque backend command resource token for the buffer.
    pub command_resource_token: u64,
    /// Backing allocation size in bytes.
    pub allocation_size: u64,
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
            command_resource_token: 0,
            allocation_size: 0,
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

impl Default for GpuTimelineFail {
    fn default() -> Self {
        Self::new()
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

/// Fixed-width request and response for [`commands::GPU_QUERY_DIALECT`].
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
    /// Reserved for ABI-compatible future use. Must be zero.
    pub reserved3: u32,
    /// Opaque backend-defined dialect information bytes.
    pub dialect_info: [u8; GPU_DIALECT_INFO_BYTES],
}

impl GpuQueryDialect {
    /// Create a dialect query for the current ABI version.
    ///
    /// # Arguments
    ///
    /// * `dialect_index` - Backend-defined dialect index to query.
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

    /// Return the meaningful opaque dialect information bytes.
    ///
    /// # Returns
    ///
    /// A bounded slice of `dialect_info`.
    pub fn dialect_info_bytes(&self) -> &[u8] {
        &self.dialect_info[..(self.dialect_info_len as usize).min(GPU_DIALECT_INFO_BYTES)]
    }
}

/// Fixed-width request and response for [`commands::GPU_CREATE_CONTEXT`].
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
    /// Reserved for ABI-compatible future use. Must be zero.
    pub reserved2: u32,
    /// Opaque token for the dialect actually selected by the backend.
    pub effective_dialect_token: u64,
}

impl GpuCreateContext {
    /// Create a context request for the current ABI version.
    ///
    /// # Arguments
    ///
    /// * `dialect_index` - Requested backend-defined dialect index.
    /// * `requested_dialect_token` - Token returned by a dialect query.
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
}

/// Fixed-width response for [`commands::GPU_CONTEXT_QUERY`].
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
    /// Reserved for ABI-compatible future use. Must be zero.
    pub reserved2: u64,
}

impl GpuContextInfo {
    /// Create a zeroed context query for the current ABI version.
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
}

impl Default for GpuContextInfo {
    fn default() -> Self {
        Self::new()
    }
}

/// Fixed-width request and response for [`commands::GPU_CREATE_QUEUE`].
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
    /// Reserved for ABI-compatible future use. Must be zero.
    pub reserved2: u64,
}

impl GpuCreateQueue {
    /// Create a queue request for the current ABI version.
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
}

impl Default for GpuCreateQueue {
    fn default() -> Self {
        Self::new()
    }
}

/// Fixed-width response for [`commands::GPU_QUEUE_QUERY`].
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
    /// Reserved for ABI-compatible future use. Must be zero.
    pub reserved2: u64,
}

impl GpuQueueInfo {
    /// Create a zeroed queue query for the current ABI version.
    pub const fn new() -> Self {
        Self {
            abi_version: GPU_ABI_VERSION,
            result: GPU_RESULT_SUCCESS,
            max_opaque_command_size: 0,
            reserved: 0,
            reserved2: 0,
        }
    }
}

impl Default for GpuQueueInfo {
    fn default() -> Self {
        Self::new()
    }
}

/// Fixed-width request and response for [`commands::GPU_QUEUE_SUBMIT`].
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
    /// * `commands` - Non-empty backend-defined command byte stream.
    ///
    /// # Returns
    ///
    /// A request that borrows the command slice for the synchronous control call.
    pub fn new(commands: &[u8]) -> HandleResult<Self> {
        let command_size =
            u32::try_from(commands.len()).map_err(|_| HandleError::InvalidParameter)?;
        if command_size == 0 || command_size > GPU_MAX_OPAQUE_COMMAND_SIZE {
            return Err(HandleError::InvalidParameter);
        }
        Ok(Self {
            abi_version: GPU_ABI_VERSION,
            result: GPU_RESULT_SUCCESS,
            flags: 0,
            reserved: 0,
            command_ptr: commands.as_ptr() as usize as u64,
            command_size,
            signal_timeline_handle: 0,
            signal_value: 0,
            completed_value: 0,
            timeline_failed: 0,
            reserved2: 0,
        })
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
            command_resource_token: request.command_resource_token,
            allocation_size: request.allocation_size,
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

    /// Create a BGRA8 render-target and presentable GPU image.
    ///
    /// # Arguments
    ///
    /// * `width` - Requested non-zero image width in pixels.
    /// * `height` - Requested non-zero image height in pixels.
    ///
    /// # Returns
    ///
    /// An owning image capability wrapper or a handle error.
    pub fn create_image(&self, width: u32, height: u32) -> HandleResult<GpuImage> {
        let mut request = GpuCreateImage::new(width, height);
        self.create_image_request(&mut request)
    }

    /// Create a BGRA8 GPU image with explicit generic usage flags.
    ///
    /// # Arguments
    ///
    /// * `width` - Requested non-zero image width in pixels.
    /// * `height` - Requested non-zero image height in pixels.
    /// * `usage` - `GPU_IMAGE_USAGE_*` flags for the image.
    ///
    /// # Returns
    ///
    /// An owning image capability wrapper or a handle error.
    pub fn create_image_with_usage(
        &self,
        width: u32,
        height: u32,
        usage: u32,
    ) -> HandleResult<GpuImage> {
        let mut request = GpuCreateImage::new_with_usage(width, height, usage);
        self.create_image_request(&mut request)
    }

    /// Create a GPU image with an explicit generic format and usage flags.
    ///
    /// # Arguments
    ///
    /// * `format` - A `GPU_IMAGE_FORMAT_*` value.
    /// * `width` - Requested non-zero image width in pixels.
    /// * `height` - Requested non-zero image height in pixels.
    /// * `usage` - `GPU_IMAGE_USAGE_*` flags for the image.
    ///
    /// # Returns
    ///
    /// An owning image capability wrapper or a handle error.
    pub fn create_image_with_format_and_usage(
        &self,
        format: u32,
        width: u32,
        height: u32,
        usage: u32,
    ) -> HandleResult<GpuImage> {
        let mut request = GpuCreateImage::new_with_usage(width, height, usage);
        request.format = format;
        self.create_image_request(&mut request)
    }

    /// Create a sampled BGRA texture image backed by an existing SharedMemory object.
    ///
    /// # Arguments
    ///
    /// * `shared_memory` - SharedMemory object containing the source BGRA pixels.
    /// * `width` - Requested non-zero image width in pixels.
    /// * `height` - Requested non-zero image height in pixels.
    /// * `shm_offset` - Byte offset of pixel `(0, 0)` in SharedMemory.
    /// * `source_stride` - Number of bytes between source rows in SharedMemory.
    ///
    /// # Returns
    ///
    /// An owning image wrapper that keeps the kernel import pinned until its
    /// context is explicitly detached and all image references are released.
    pub fn create_imported_bgra_image(
        &self,
        shared_memory: &SharedMemory,
        width: u32,
        height: u32,
        shm_offset: u64,
        source_stride: u32,
    ) -> HandleResult<GpuImage> {
        let shm_handle =
            u32::try_from(shared_memory.as_raw()).map_err(|_| HandleError::InvalidHandle)?;
        let mut request =
            GpuCreateImportedImageBgra::new(shm_handle, width, height, shm_offset, source_stride);
        self.file.as_handle().control(
            commands::GPU_CREATE_IMPORTED_IMAGE_BGRA,
            &mut request as *mut _ as usize,
        )?;
        result_to_handle_error(request.result)?;
        Ok(GpuImage {
            handle: adopt_child_handle(request.image_handle)?,
            command_resource_token: request.command_resource_token,
        })
    }

    fn create_image_request(&self, request: &mut GpuCreateImage) -> HandleResult<GpuImage> {
        self.file
            .as_handle()
            .control(commands::GPU_CREATE_IMAGE, request as *mut _ as usize)?;
        result_to_handle_error(request.result)?;
        Ok(GpuImage {
            handle: adopt_child_handle(request.image_handle)?,
            command_resource_token: request.command_resource_token,
        })
    }

    /// Query one backend-defined execution dialect.
    ///
    /// # Arguments
    ///
    /// * `dialect_index` - Backend-defined dialect index.
    ///
    /// # Returns
    ///
    /// An opaque dialect descriptor suitable for context creation.
    pub fn query_dialect(&self, dialect_index: u32) -> HandleResult<GpuDialect> {
        let mut request = GpuQueryDialect::new(dialect_index);
        self.file
            .as_handle()
            .control(commands::GPU_QUERY_DIALECT, &mut request as *mut _ as usize)?;
        result_to_handle_error(request.result)?;
        Ok(GpuDialect {
            index: request.dialect_index,
            token: request.dialect_token,
            opaque_info: request.dialect_info,
            opaque_info_len: request.dialect_info_len,
        })
    }

    /// Create an execution context for a queried backend dialect.
    ///
    /// # Arguments
    ///
    /// * `dialect` - Opaque dialect descriptor returned by [`Gpu::query_dialect`].
    ///
    /// # Returns
    ///
    /// An owning execution context wrapper or a handle error.
    pub fn create_context(&self, dialect: &GpuDialect) -> HandleResult<GpuContext> {
        let mut request = GpuCreateContext::new(dialect.index, dialect.token);
        self.file.as_handle().control(
            commands::GPU_CREATE_CONTEXT,
            &mut request as *mut _ as usize,
        )?;
        result_to_handle_error(request.result)?;
        Ok(GpuContext {
            handle: adopt_child_handle(request.context_handle)?,
            effective_dialect_index: request.effective_dialect_index,
            effective_dialect_token: request.effective_dialect_token,
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

/// Opaque backend-defined execution dialect descriptor.
#[derive(Debug, Clone, Copy)]
pub struct GpuDialect {
    index: u32,
    token: u64,
    opaque_info: [u8; GPU_DIALECT_INFO_BYTES],
    opaque_info_len: u32,
}

impl GpuDialect {
    /// Return the backend-defined dialect index.
    ///
    /// # Returns
    ///
    /// The index used to query this dialect.
    pub const fn index(&self) -> u32 {
        self.index
    }

    /// Return the opaque backend dialect token.
    ///
    /// # Returns
    ///
    /// The token used when creating a context.
    pub const fn token(&self) -> u64 {
        self.token
    }

    /// Return the meaningful opaque backend dialect information bytes.
    ///
    /// # Returns
    ///
    /// A bounded slice of backend-defined capability data.
    pub fn opaque_info(&self) -> &[u8] {
        &self.opaque_info[..(self.opaque_info_len as usize).min(GPU_DIALECT_INFO_BYTES)]
    }
}

/// Owning RAII wrapper for a GPU execution context child handle.
pub struct GpuContext {
    handle: Handle,
    effective_dialect_index: u32,
    effective_dialect_token: u64,
}

impl GpuContext {
    /// Query the context's effective execution dialect.
    ///
    /// # Returns
    ///
    /// Current context information or a handle error.
    pub fn query(&self) -> HandleResult<GpuContextInfo> {
        let mut info = GpuContextInfo::new();
        self.handle
            .control(commands::GPU_CONTEXT_QUERY, &mut info as *mut _ as usize)?;
        result_to_handle_error(info.result)?;
        Ok(info)
    }

    /// Create an execution queue owned by this context.
    ///
    /// # Returns
    ///
    /// An owning queue wrapper or a handle error.
    pub fn create_queue(&self) -> HandleResult<GpuQueue> {
        let mut request = GpuCreateQueue::new();
        self.handle
            .control(commands::GPU_CREATE_QUEUE, &mut request as *mut _ as usize)?;
        result_to_handle_error(request.result)?;
        Ok(GpuQueue {
            handle: adopt_child_handle(request.queue_handle)?,
            max_opaque_command_size: request.max_opaque_command_size,
        })
    }

    /// Attach an image so this context's opaque commands may reference it.
    ///
    /// # Arguments
    ///
    /// * `image` - Image capability to attach.
    ///
    /// # Returns
    ///
    /// A non-zero opaque attachment token authorized only for this context. It
    /// is distinct from the image's backend resource identity token.
    pub fn attach_image(&self, image: &GpuImage) -> HandleResult<u64> {
        let image_handle =
            u32::try_from(image.handle.as_raw()).map_err(|_| HandleError::InvalidHandle)?;
        let mut request = GpuContextAttachImage::new(image_handle);
        self.handle.control(
            commands::GPU_CONTEXT_ATTACH_IMAGE,
            &mut request as *mut _ as usize,
        )?;
        result_to_handle_error(request.result)?;
        if request.command_resource_token == 0 {
            return Err(HandleError::SystemError(-1));
        }
        Ok(request.command_resource_token)
    }

    /// Detach an image so the context releases its retained backing reference.
    ///
    /// # Arguments
    ///
    /// * `image` - Image capability previously attached with [`GpuContext::attach_image`].
    ///
    /// # Returns
    ///
    /// Success after the backend detached the image, or a handle error.
    pub fn detach_image(&self, image: &GpuImage) -> HandleResult<()> {
        let image_handle =
            u32::try_from(image.handle.as_raw()).map_err(|_| HandleError::InvalidHandle)?;
        let mut request = GpuContextDetachImage::new(image_handle);
        self.handle.control(
            commands::GPU_CONTEXT_DETACH_IMAGE,
            &mut request as *mut _ as usize,
        )?;
        result_to_handle_error(request.result)
    }

    /// Attach a buffer so this context's opaque commands may reference it.
    ///
    /// # Arguments
    ///
    /// * `buffer` - Buffer capability to attach.
    ///
    /// # Returns
    ///
    /// A non-zero opaque attachment token authorized only for this context. It
    /// is distinct from the buffer's backend resource identity token.
    pub fn attach_buffer(&self, buffer: &GpuBuffer) -> HandleResult<u64> {
        let buffer_handle =
            u32::try_from(buffer.handle.as_raw()).map_err(|_| HandleError::InvalidHandle)?;
        let mut request = GpuContextAttachBuffer::new(buffer_handle);
        self.handle.control(
            commands::GPU_CONTEXT_ATTACH_BUFFER,
            &mut request as *mut _ as usize,
        )?;
        result_to_handle_error(request.result)?;
        if request.command_resource_token == 0 {
            return Err(HandleError::SystemError(-1));
        }
        Ok(request.command_resource_token)
    }

    /// Detach a buffer so the context releases its retained backing reference.
    ///
    /// # Arguments
    ///
    /// * `buffer` - Buffer capability previously attached with [`GpuContext::attach_buffer`].
    ///
    /// # Returns
    ///
    /// Success after the backend detached the buffer, or a handle error.
    pub fn detach_buffer(&self, buffer: &GpuBuffer) -> HandleResult<()> {
        let buffer_handle =
            u32::try_from(buffer.handle.as_raw()).map_err(|_| HandleError::InvalidHandle)?;
        let mut request = GpuContextDetachBuffer::new(buffer_handle);
        self.handle.control(
            commands::GPU_CONTEXT_DETACH_BUFFER,
            &mut request as *mut _ as usize,
        )?;
        result_to_handle_error(request.result)
    }

    /// Upload a strided BGRA source rectangle into an image attached to this context.
    ///
    /// The kernel validates the destination bounds and image usage, copies the
    /// source into kernel-owned backing, and synchronously transfers that backing
    /// to the backend. The source slice is never retained after this call returns.
    ///
    /// # Arguments
    ///
    /// * `image` - Image capability previously attached with [`GpuContext::attach_image`].
    /// * `pixels` - Source BGRA pixel bytes.
    /// * `source_stride` - Source row stride in bytes.
    /// * `rect` - Destination rectangle in the image.
    ///
    /// # Returns
    ///
    /// Nothing after synchronous upload completion, or a handle error.
    pub fn upload_image_bgra(
        &self,
        image: &GpuImage,
        pixels: &[u8],
        source_stride: u32,
        rect: GpuImageBgraRect,
    ) -> HandleResult<()> {
        let image_handle =
            u32::try_from(image.handle.as_raw()).map_err(|_| HandleError::InvalidHandle)?;
        let mut request =
            GpuContextUploadImageBgra::new(image_handle, pixels, source_stride, rect)?;
        self.handle.control(
            commands::GPU_CONTEXT_UPLOAD_IMAGE_BGRA,
            &mut request as *mut _ as usize,
        )?;
        result_to_handle_error(request.result)
    }

    /// Transfer one rectangle from an imported image's fixed SharedMemory backing.
    ///
    /// # Arguments
    ///
    /// * `image` - Imported image capability previously attached to this context.
    /// * `rect` - Destination rectangle in the image.
    ///
    /// # Returns
    ///
    /// Success after synchronous transfer completion, or a handle error. No
    /// userspace pixel pointer is passed to the kernel.
    pub fn transfer_imported_image_bgra(
        &self,
        image: &GpuImage,
        rect: GpuImageBgraRect,
    ) -> HandleResult<()> {
        let image_handle =
            u32::try_from(image.handle.as_raw()).map_err(|_| HandleError::InvalidHandle)?;
        let mut request = GpuContextTransferImportedImageBgra::new(image_handle, rect);
        self.handle.control(
            commands::GPU_CONTEXT_TRANSFER_IMPORTED_IMAGE_BGRA,
            &mut request as *mut _ as usize,
        )?;
        result_to_handle_error(request.result)
    }

    /// Read one attached BGRA image rectangle into a CPU-visible destination.
    ///
    /// The destination describes a complete framebuffer and receives the
    /// rectangle at the same `(x, y)` coordinates used by `rect`. The kernel
    /// waits for GPU-to-backing transfer, invalidates the backing cache range,
    /// and copies only the requested rows before returning.
    ///
    /// # Arguments
    ///
    /// * `image` - Image capability previously attached to this context.
    /// * `destination` - Complete writable BGRA destination buffer.
    /// * `destination_stride` - Bytes between destination rows.
    /// * `rect` - Source image rectangle.
    ///
    /// # Returns
    ///
    /// Success after synchronous readback completion, or a handle error.
    pub fn readback_image_bgra(
        &self,
        image: &GpuImage,
        destination: &mut [u8],
        destination_stride: u32,
        rect: GpuImageBgraRect,
    ) -> HandleResult<()> {
        let image_handle =
            u32::try_from(image.handle.as_raw()).map_err(|_| HandleError::InvalidHandle)?;
        let mut request =
            GpuContextReadbackImageBgra::new(image_handle, destination, destination_stride, rect)?;
        self.handle.control(
            commands::GPU_CONTEXT_READBACK_IMAGE_BGRA,
            &mut request as *mut _ as usize,
        )?;
        result_to_handle_error(request.result)
    }

    /// Return the effective backend-defined dialect index.
    ///
    /// # Returns
    ///
    /// The dialect index selected by the backend.
    pub const fn effective_dialect_index(&self) -> u32 {
        self.effective_dialect_index
    }

    /// Return the effective opaque backend dialect token.
    ///
    /// # Returns
    ///
    /// The dialect token selected by the backend.
    pub const fn effective_dialect_token(&self) -> u64 {
        self.effective_dialect_token
    }

    /// Return the underlying context handle.
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

/// Owning RAII wrapper for a GPU execution queue child handle.
pub struct GpuQueue {
    handle: Handle,
    max_opaque_command_size: u32,
}

impl GpuQueue {
    /// Query this queue's current command limits.
    ///
    /// # Returns
    ///
    /// Current queue information or a handle error.
    pub fn query(&self) -> HandleResult<GpuQueueInfo> {
        let mut info = GpuQueueInfo::new();
        self.handle
            .control(commands::GPU_QUEUE_QUERY, &mut info as *mut _ as usize)?;
        result_to_handle_error(info.result)?;
        Ok(info)
    }

    /// Synchronously submit one opaque backend command stream.
    ///
    /// # Arguments
    ///
    /// * `commands` - Non-empty backend-defined command bytes.
    ///
    /// # Returns
    ///
    /// Submission results after fenced backend completion.
    pub fn submit(&self, commands: &[u8]) -> HandleResult<GpuQueueSubmit> {
        let mut request = self.prepare_submission(commands)?;
        self.control_submission(&mut request)?;
        Ok(request)
    }

    /// Synchronously submit commands and signal a timeline after completion.
    ///
    /// # Arguments
    ///
    /// * `commands` - Non-empty backend-defined command bytes.
    /// * `timeline` - Timeline to update only after successful backend completion.
    /// * `value` - Non-decreasing completed value to signal.
    ///
    /// # Returns
    ///
    /// Submission and timeline state after fenced backend completion.
    pub fn submit_and_signal(
        &self,
        commands: &[u8],
        timeline: &GpuTimeline,
        value: u64,
    ) -> HandleResult<GpuQueueSubmit> {
        let mut request = self.prepare_submission(commands)?;
        request.flags = GPU_QUEUE_SUBMIT_FLAG_SIGNAL_TIMELINE;
        request.signal_timeline_handle =
            u32::try_from(timeline.handle.as_raw()).map_err(|_| HandleError::InvalidHandle)?;
        request.signal_value = value;
        self.control_submission(&mut request)?;
        Ok(request)
    }

    /// Return the queue's creation-time command limit.
    ///
    /// # Returns
    ///
    /// Maximum accepted command stream size in bytes.
    pub const fn max_opaque_command_size(&self) -> u32 {
        self.max_opaque_command_size
    }

    /// Return the underlying queue handle.
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

    fn prepare_submission(&self, commands: &[u8]) -> HandleResult<GpuQueueSubmit> {
        let request = GpuQueueSubmit::new(commands)?;
        if request.command_size > self.max_opaque_command_size {
            return Err(HandleError::InvalidParameter);
        }
        Ok(request)
    }

    fn control_submission(&self, request: &mut GpuQueueSubmit) -> HandleResult<()> {
        self.handle
            .control(commands::GPU_QUEUE_SUBMIT, request as *mut _ as usize)?;
        result_to_handle_error(request.result)
    }
}

/// Owning RAII wrapper for a connection-created GPU buffer child handle.
pub struct GpuBuffer {
    handle: Handle,
    command_resource_token: u64,
    allocation_size: u64,
    flags: u32,
}

/// Owning RAII wrapper for a connection-created GPU image child handle.
pub struct GpuImage {
    handle: Handle,
    command_resource_token: u64,
}

impl GpuImage {
    /// Adopt a transferred GPU image capability handle.
    ///
    /// The handle is queried before it is accepted, which verifies that it is a
    /// live GPU image capability and recovers the backend resource token needed
    /// when attaching it to a context.
    ///
    /// # Arguments
    ///
    /// * `handle` - Owning GPU image capability received from another process.
    ///
    /// # Returns
    ///
    /// An owning image wrapper or a handle error when the transferred object is
    /// not a valid GPU image.
    pub fn from_handle(handle: Handle) -> HandleResult<Self> {
        let mut info = GpuImageInfo::new();
        handle.control(commands::GPU_IMAGE_QUERY_INFO, &mut info as *mut _ as usize)?;
        result_to_handle_error(info.result)?;
        if info.command_resource_token == 0 {
            return Err(HandleError::InvalidHandle);
        }
        Ok(Self {
            handle,
            command_resource_token: info.command_resource_token,
        })
    }

    /// Query the image's immutable format, usage, extent, and allocation details.
    ///
    /// # Returns
    ///
    /// Fixed-width image information or a handle error.
    pub fn query(&self) -> HandleResult<GpuImageInfo> {
        let mut info = GpuImageInfo::new();
        self.handle
            .control(commands::GPU_IMAGE_QUERY_INFO, &mut info as *mut _ as usize)?;
        result_to_handle_error(info.result)?;
        Ok(info)
    }
    /// Query the immutable modifier, allocation, and plane layout.
    ///
    /// # Returns
    ///
    /// The backend-selected image layout fixed at creation time.
    pub fn query_layout(&self) -> HandleResult<GpuImageLayout> {
        let mut layout = GpuImageLayout::new();
        self.handle.control(
            commands::GPU_IMAGE_QUERY_LAYOUT,
            &mut layout as *mut _ as usize,
        )?;
        result_to_handle_error(layout.result)?;
        if layout.reserved != 0
            || layout.plane_count == 0
            || layout.plane_count as usize > GPU_IMAGE_MAX_PLANES
            || layout.planes[..layout.plane_count as usize]
                .iter()
                .any(|plane| plane.reserved != 0)
        {
            return Err(HandleError::InvalidParameter);
        }
        Ok(layout)
    }

    /// Return the opaque backend command resource token.
    ///
    /// This token is not authority. The image capability handle remains required
    /// to attach the image to a context or present it through a display surface.
    ///
    /// # Returns
    ///
    /// The backend-defined opaque command resource token.
    pub const fn token(&self) -> u64 {
        self.command_resource_token
    }

    /// Return the underlying image capability handle.
    ///
    /// # Returns
    ///
    /// A borrowed owning-handle wrapper for display presentation.
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
        self.allocation_size
    }

    /// Return the opaque backend command resource token.
    ///
    /// This token is not authority. The buffer capability handle remains required
    /// to attach the buffer to an execution context.
    ///
    /// # Returns
    ///
    /// The backend-defined opaque command resource token.
    pub const fn token(&self) -> u64 {
        self.command_resource_token
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
        GPU_RESULT_INVALID_STATE | GPU_RESULT_UNSUPPORTED => Err(HandleError::Unsupported),
        _ => Err(HandleError::SystemError(result as i32)),
    }
}

fn validate_bgra_upload_pixels(
    pixels: &[u8],
    source_stride: u32,
    rect: GpuImageBgraRect,
) -> HandleResult<()> {
    if rect.width == 0 || rect.height == 0 {
        return Err(HandleError::InvalidParameter);
    }
    let row_bytes = rect
        .width
        .checked_mul(4)
        .ok_or(HandleError::InvalidParameter)?;
    if source_stride < row_bytes {
        return Err(HandleError::InvalidParameter);
    }
    let height = usize::try_from(rect.height).map_err(|_| HandleError::InvalidParameter)?;
    let row_bytes = usize::try_from(row_bytes).map_err(|_| HandleError::InvalidParameter)?;
    let source_stride =
        usize::try_from(source_stride).map_err(|_| HandleError::InvalidParameter)?;
    let required_source_len = height
        .checked_sub(1)
        .and_then(|rows| rows.checked_mul(source_stride))
        .and_then(|offset| offset.checked_add(row_bytes))
        .ok_or(HandleError::InvalidParameter)?;
    let copy_size = row_bytes
        .checked_mul(height)
        .ok_or(HandleError::InvalidParameter)?;
    if pixels.len() < required_source_len || copy_size > GPU_MAX_IMAGE_UPLOAD_SIZE as usize {
        return Err(HandleError::InvalidParameter);
    }
    Ok(())
}

const _: [(); 296] = [(); core::mem::size_of::<GpuQueryDialect>()];
const _: [(); 48] = [(); core::mem::size_of::<GpuCreateContext>()];
const _: [(); 32] = [(); core::mem::size_of::<GpuContextInfo>()];
const _: [(); 32] = [(); core::mem::size_of::<GpuCreateQueue>()];
const _: [(); 24] = [(); core::mem::size_of::<GpuQueueInfo>()];
const _: [(); 56] = [(); core::mem::size_of::<GpuQueueSubmit>()];
const _: [(); 48] = [(); core::mem::size_of::<GpuCreateImage>()];
const _: [(); 40] = [(); core::mem::size_of::<GpuImageInfo>()];
const _: [(); 32] = [(); core::mem::size_of::<GpuImagePlaneLayout>()];
const _: [(); 168] = [(); core::mem::size_of::<GpuImageLayout>()];
const _: [(); 32] = [(); core::mem::size_of::<GpuContextAttachImage>()];
const _: [(); 24] = [(); core::mem::size_of::<GpuContextDetachImage>()];
const _: [(); 64] = [(); core::mem::size_of::<GpuCreateImportedImageBgra>()];
const _: [(); 48] = [(); core::mem::size_of::<GpuCreateBuffer>()];
const _: [(); 40] = [(); core::mem::size_of::<GpuBufferInfo>()];
const _: [(); 32] = [(); core::mem::size_of::<GpuContextAttachBuffer>()];
const _: [(); 24] = [(); core::mem::size_of::<GpuContextDetachBuffer>()];
const _: [(); 64] = [(); core::mem::size_of::<GpuContextUploadImageBgra>()];
const _: [(); 40] = [(); core::mem::size_of::<GpuContextTransferImportedImageBgra>()];
const _: [(); 64] = [(); core::mem::size_of::<GpuContextReadbackImageBgra>()];
