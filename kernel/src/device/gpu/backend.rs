//! Backend-neutral GPU information model.

use alloc::sync::Arc;

use super::{GPU_BACKEND_ID_BYTES, GPU_BACKEND_INFO_BYTES};
use crate::device::graphics::GpuDisplayResource;

/// The device is not usable for GPU control.
pub const GPU_DEVICE_STATE_UNAVAILABLE: u32 = 0;
/// The device is available for its advertised operations.
pub const GPU_DEVICE_STATE_READY: u32 = 1;
/// The device was lost after it became available.
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

/// Stable state of a GPU device.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuDeviceState {
    /// The backend is unavailable for GPU control.
    Unavailable = GPU_DEVICE_STATE_UNAVAILABLE,
    /// The backend is ready for its advertised operations.
    Ready = GPU_DEVICE_STATE_READY,
    /// The backend has been lost.
    Lost = GPU_DEVICE_STATE_LOST,
}

/// Backend-neutral stable GPU device information.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuDeviceInfo {
    /// Current stable device state.
    pub state: GpuDeviceState,
    /// Truthful generic execution support bits.
    pub execution_support: u32,
    /// Maximum opaque command size for a generic command operation, or zero.
    pub max_opaque_command_size: u32,
}

impl GpuDeviceInfo {
    /// Build stable information for a GPU device.
    ///
    /// # Arguments
    ///
    /// * `state` - Current backend device state.
    /// * `execution_support` - Truthful generic execution support bits.
    /// * `max_opaque_command_size` - Maximum generic opaque command size, or zero.
    ///
    /// # Returns
    ///
    /// Stable GPU device information.
    pub const fn new(
        state: GpuDeviceState,
        execution_support: u32,
        max_opaque_command_size: u32,
    ) -> Self {
        Self {
            state,
            execution_support,
            max_opaque_command_size,
        }
    }
}

/// Backend-provided information exposed through a GPU connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuBackendInfo {
    /// Stable device information shared across all backend implementations.
    pub device: GpuDeviceInfo,
    /// Backend-defined negotiated feature bits.
    pub backend_feature_bits: u64,
    /// Opaque backend or dialect identifier bytes.
    pub backend_id: [u8; GPU_BACKEND_ID_BYTES],
    /// Length of meaningful bytes in `backend_id`.
    pub backend_id_len: u32,
    /// Opaque backend-defined bytes.
    pub opaque_info: [u8; GPU_BACKEND_INFO_BYTES],
    /// Length of meaningful bytes in `opaque_info`.
    pub opaque_info_len: u32,
}

impl GpuBackendInfo {
    /// Build backend information from bounded identifier and opaque data slices.
    ///
    /// # Arguments
    ///
    /// * `device` - Stable generic device information.
    /// * `backend_feature_bits` - Backend-defined negotiated feature bits.
    /// * `backend_id` - Opaque backend or dialect identifier bytes.
    /// * `opaque_info` - Opaque backend-defined information bytes.
    ///
    /// # Returns
    ///
    /// A fixed-capacity backend information record. Input slices are truncated
    /// to their ABI-defined fixed capacities.
    pub fn new(
        device: GpuDeviceInfo,
        backend_feature_bits: u64,
        backend_id: &[u8],
        opaque_info: &[u8],
    ) -> Self {
        let mut result = Self {
            device,
            backend_feature_bits,
            backend_id: [0; GPU_BACKEND_ID_BYTES],
            backend_id_len: 0,
            opaque_info: [0; GPU_BACKEND_INFO_BYTES],
            opaque_info_len: 0,
        };
        let backend_id_len = backend_id.len().min(GPU_BACKEND_ID_BYTES);
        result.backend_id[..backend_id_len].copy_from_slice(&backend_id[..backend_id_len]);
        result.backend_id_len = backend_id_len as u32;
        let opaque_info_len = opaque_info.len().min(GPU_BACKEND_INFO_BYTES);
        result.opaque_info[..opaque_info_len].copy_from_slice(&opaque_info[..opaque_info_len]);
        result.opaque_info_len = opaque_info_len as u32;
        result
    }
}

/// Opaque dialect selection data supplied by a backend query.
///
/// This descriptor is not a capability. It only carries query data used by a
/// backend to choose an execution dialect; GPU object handles remain the only
/// authority for subsequent operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuBackendDialectDescriptor {
    /// Backend-defined index selected by the caller.
    pub index: u32,
    /// Opaque backend token returned by a prior dialect query.
    pub token: u64,
}

impl GpuBackendDialectDescriptor {
    /// Build an opaque dialect selection descriptor.
    ///
    /// # Arguments
    ///
    /// * `index` - Backend-defined dialect index.
    /// * `token` - Opaque token returned from a dialect query.
    ///
    /// # Returns
    ///
    /// A descriptor that a backend must validate before use.
    pub const fn new(index: u32, token: u64) -> Self {
        Self { index, token }
    }
}

/// Bounded backend-defined information for one execution dialect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuBackendDialectInfo {
    /// Backend-defined query index for this dialect.
    pub index: u32,
    /// Opaque backend token for this dialect.
    pub token: u64,
    /// Fixed-capacity opaque dialect information.
    pub opaque_info: [u8; super::GPU_DIALECT_INFO_BYTES],
    /// Number of meaningful bytes in `opaque_info`.
    pub opaque_info_len: u32,
}

impl GpuBackendDialectInfo {
    /// Build bounded opaque information for one execution dialect.
    ///
    /// # Arguments
    ///
    /// * `index` - Backend-defined query index.
    /// * `token` - Opaque backend token for the dialect.
    /// * `opaque_info` - Backend-defined dialect data to expose.
    ///
    /// # Returns
    ///
    /// A fixed-capacity dialect information record. Input data is truncated to
    /// the ABI-defined capacity.
    pub fn new(index: u32, token: u64, opaque_info: &[u8]) -> Self {
        let mut result = Self {
            index,
            token,
            opaque_info: [0; super::GPU_DIALECT_INFO_BYTES],
            opaque_info_len: 0,
        };
        let opaque_info_len = opaque_info.len().min(super::GPU_DIALECT_INFO_BYTES);
        result.opaque_info[..opaque_info_len].copy_from_slice(&opaque_info[..opaque_info_len]);
        result.opaque_info_len = opaque_info_len as u32;
        result
    }

    /// Return the descriptor corresponding to this query result.
    ///
    /// # Returns
    ///
    /// Opaque selection data that a backend must validate again when creating
    /// an execution context.
    pub const fn descriptor(&self) -> GpuBackendDialectDescriptor {
        GpuBackendDialectDescriptor::new(self.index, self.token)
    }
}

/// Effective execution context information reported by a backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuBackendContextInfo {
    /// Effective backend-defined dialect index.
    pub dialect_index: u32,
    /// Opaque backend token for the effective dialect.
    pub dialect_token: u64,
}

impl GpuBackendContextInfo {
    /// Build effective execution context information.
    ///
    /// # Arguments
    ///
    /// * `dialect_index` - Backend-defined effective dialect index.
    /// * `dialect_token` - Opaque token for the effective dialect.
    ///
    /// # Returns
    ///
    /// Effective context information.
    pub const fn new(dialect_index: u32, dialect_token: u64) -> Self {
        Self {
            dialect_index,
            dialect_token,
        }
    }
}

/// Execution queue limits reported by a backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuBackendQueueInfo {
    /// Maximum opaque command byte length accepted by this queue.
    pub max_opaque_command_size: u32,
}

/// Backend-neutral physical backing for a GPU buffer resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuBufferCreateInfo {
    /// Physical address of the stable contiguous backing allocation.
    pub paddr: usize,
    /// Page-rounded allocation size in bytes.
    pub allocation_size: u64,
}

impl GpuBufferCreateInfo {
    /// Build buffer backing information for a backend resource.
    ///
    /// # Arguments
    ///
    /// * `paddr` - Physical address of the contiguous backing allocation.
    /// * `allocation_size` - Non-zero backing allocation size in bytes.
    ///
    /// # Returns
    ///
    /// Backend-neutral buffer backing information.
    pub const fn new(paddr: usize, allocation_size: u64) -> Self {
        Self {
            paddr,
            allocation_size,
        }
    }
}

/// Immutable metadata for a backend-owned GPU buffer resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuBackendBufferInfo {
    /// Opaque backend token used only by opaque command bytes.
    pub command_resource_token: u64,
    /// Page-rounded backing allocation size in bytes.
    pub allocation_size: u64,
}

impl GpuBackendBufferInfo {
    /// Build immutable backend buffer metadata.
    ///
    /// # Arguments
    ///
    /// * `command_resource_token` - Opaque command resource token.
    /// * `allocation_size` - Actual backing allocation size in bytes.
    ///
    /// # Returns
    ///
    /// Backend buffer metadata.
    pub const fn new(command_resource_token: u64, allocation_size: u64) -> Self {
        Self {
            command_resource_token,
            allocation_size,
        }
    }
}

/// Backend buffer retained by a [`crate::device::gpu::GpuBuffer`].
pub trait GpuBackendBuffer: Send + Sync {
    /// Query immutable backend-neutral buffer information.
    ///
    /// # Returns
    ///
    /// An opaque command resource token and backing allocation size.
    fn query_info(&self) -> GpuBackendBufferInfo;

    /// Return an opaque backend identity used only for attachment validation.
    ///
    /// # Returns
    ///
    /// A token that distinguishes backend instances without granting authority.
    fn backend_cookie(&self) -> u64;
}

impl GpuBackendQueueInfo {
    /// Build execution queue limits.
    ///
    /// # Arguments
    ///
    /// * `max_opaque_command_size` - Maximum bounded opaque command size.
    ///
    /// # Returns
    ///
    /// Queue information for an execution backend.
    pub const fn new(max_opaque_command_size: u32) -> Self {
        Self {
            max_opaque_command_size,
        }
    }
}

/// Validated backend-neutral image creation parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuImageCreateInfo {
    /// Generic image format.
    pub format: u32,
    /// Generic image usage flags.
    pub usage: u32,
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
}

/// Backend-neutral physical backing for a GPU image resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuImageBackingInfo {
    /// Physical address of the stable contiguous backing allocation.
    pub paddr: usize,
    /// Page-rounded allocation size in bytes.
    pub allocation_size: u64,
}

impl GpuImageBackingInfo {
    /// Build image backing information for a backend resource.
    ///
    /// # Arguments
    ///
    /// * `paddr` - Physical address of the contiguous image backing.
    /// * `allocation_size` - Non-zero backing allocation size in bytes.
    ///
    /// # Returns
    ///
    /// Backend-neutral image backing information.
    pub const fn new(paddr: usize, allocation_size: u64) -> Self {
        Self {
            paddr,
            allocation_size,
        }
    }
}

/// Validated kernel-backing region for one image upload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuImageUploadInfo {
    /// Byte offset of the first uploaded pixel in the image backing.
    pub backing_offset: u64,
    /// Full image row stride in bytes.
    pub backing_stride: u32,
    /// Full image layer stride in bytes.
    pub backing_layer_stride: u32,
    /// Destination image x coordinate in pixels.
    pub dst_x: u32,
    /// Destination image y coordinate in pixels.
    pub dst_y: u32,
    /// Uploaded rectangle width in pixels.
    pub width: u32,
    /// Uploaded rectangle height in pixels.
    pub height: u32,
}

impl GpuImageUploadInfo {
    /// Build validated backend-neutral image upload information.
    ///
    /// # Arguments
    ///
    /// * `backing_offset` - Byte offset of the rectangle in image backing.
    /// * `backing_stride` - Full image row stride in bytes.
    /// * `backing_layer_stride` - Full image layer stride in bytes.
    /// * `dst_x` - Destination x coordinate in pixels.
    /// * `dst_y` - Destination y coordinate in pixels.
    /// * `width` - Uploaded width in pixels.
    /// * `height` - Uploaded height in pixels.
    ///
    /// # Returns
    ///
    /// Backend upload information that contains no userspace pointer.
    #[allow(clippy::too_many_arguments)]
    pub const fn new(
        backing_offset: u64,
        backing_stride: u32,
        backing_layer_stride: u32,
        dst_x: u32,
        dst_y: u32,
        width: u32,
        height: u32,
    ) -> Self {
        Self {
            backing_offset,
            backing_stride,
            backing_layer_stride,
            dst_x,
            dst_y,
            width,
            height,
        }
    }
}

impl GpuImageCreateInfo {
    /// Build validated image creation parameters.
    ///
    /// # Arguments
    ///
    /// * `format` - Generic image format.
    /// * `usage` - Generic image usage flags.
    /// * `width` - Non-zero image width in pixels.
    /// * `height` - Non-zero image height in pixels.
    ///
    /// # Returns
    ///
    /// Backend-neutral image creation parameters.
    pub const fn new(format: u32, usage: u32, width: u32, height: u32) -> Self {
        Self {
            format,
            usage,
            width,
            height,
        }
    }
}

/// Backend-neutral immutable image information.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuBackendImageInfo {
    /// Generic image format.
    pub format: u32,
    /// Generic image usage flags.
    pub usage: u32,
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// Opaque backend token used only by opaque command bytes.
    pub command_resource_token: u64,
    /// Backing allocation size in bytes.
    pub allocation_size: u64,
}

impl GpuBackendImageInfo {
    /// Build immutable image information.
    ///
    /// # Arguments
    ///
    /// * `create` - Validated generic image parameters.
    /// * `command_resource_token` - Opaque command resource token.
    /// * `allocation_size` - Actual backing allocation size in bytes.
    ///
    /// # Returns
    ///
    /// Backend-neutral immutable image information.
    pub const fn new(
        create: GpuImageCreateInfo,
        command_resource_token: u64,
        allocation_size: u64,
    ) -> Self {
        Self {
            format: create.format,
            usage: create.usage,
            width: create.width,
            height: create.height,
            command_resource_token,
            allocation_size,
        }
    }
}

/// Backend image retained by a [`crate::device::gpu::GpuImage`].
pub trait GpuBackendImage: Send + Sync {
    /// Query immutable backend-neutral image information.
    ///
    /// # Returns
    ///
    /// Generic image metadata and an opaque command resource token.
    fn query_info(&self) -> GpuBackendImageInfo;

    /// Return an opaque identity for the backend that owns this image.
    ///
    /// # Returns
    ///
    /// An internal backend identity used only to validate context attachment.
    fn backend_cookie(&self) -> u64;

    /// Return the display descriptor when this image is presentable.
    ///
    /// # Returns
    ///
    /// An internal display descriptor, or `None` when the image cannot be
    /// presented by the display subsystem.
    fn display_resource(&self) -> Option<GpuDisplayResource>;
}

/// Backend execution context retained by a [`crate::device::gpu::GpuContext`].
pub trait GpuBackendContext: Send + Sync {
    /// Query the effective execution dialect selected for this context.
    ///
    /// # Returns
    ///
    /// Effective backend-neutral context information.
    fn query_info(&self) -> GpuBackendContextInfo;

    /// Create one backend execution queue for this context.
    ///
    /// # Returns
    ///
    /// A backend queue whose commands execute in this context.
    fn create_queue(&self) -> Result<Arc<dyn GpuBackendQueue>, &'static str>;

    /// Attach an image so opaque commands in this context may reference it.
    ///
    /// # Arguments
    ///
    /// * `image` - Backend image retained by the calling kernel capability.
    ///
    /// # Returns
    ///
    /// An opaque command resource token authorized for this context, or an
    /// error when the image does not belong to this backend or cannot attach.
    fn attach_image(&self, _image: &dyn GpuBackendImage) -> Result<u64, &'static str> {
        Err("GPU backend context does not support image attachment")
    }

    /// Detach an image previously attached to this context.
    ///
    /// # Arguments
    ///
    /// * `image` - Backend image to detach.
    ///
    /// # Returns
    ///
    /// Nothing after the image is no longer attached to this context.
    fn detach_image(&self, _image: &dyn GpuBackendImage) -> Result<(), &'static str> {
        Err("GPU backend context does not support image detachment")
    }

    /// Upload an already copied BGRA rectangle into an attached image.
    ///
    /// # Arguments
    ///
    /// * `image` - Backend image retained by the calling kernel capability.
    /// * `upload` - Validated kernel-backing rectangle to transfer.
    ///
    /// # Returns
    ///
    /// Nothing after the backend has synchronously completed the upload, or an
    /// error when image transfer is unavailable.
    fn upload_image_bgra(
        &self,
        _image: &dyn GpuBackendImage,
        _upload: GpuImageUploadInfo,
    ) -> Result<(), &'static str> {
        Err("GPU backend context does not support image uploads")
    }

    /// Transfer a rectangle from fixed imported image backing.
    ///
    /// # Arguments
    ///
    /// * `image` - Backend image retained by the calling kernel capability.
    /// * `transfer` - Validated imported-backing rectangle to transfer.
    ///
    /// # Returns
    ///
    /// Nothing after the backend has synchronously completed the transfer, or an
    /// error when imported image transfer is unavailable.
    fn transfer_imported_image_bgra(
        &self,
        _image: &dyn GpuBackendImage,
        _transfer: GpuImageUploadInfo,
    ) -> Result<(), &'static str> {
        Err("GPU backend context does not support imported image transfers")
    }

    /// Attach a buffer so opaque commands in this context may reference it.
    ///
    /// # Arguments
    ///
    /// * `buffer` - Backend buffer retained by the calling kernel capability.
    ///
    /// # Returns
    ///
    /// An opaque command resource token authorized for this context.
    fn attach_buffer(&self, _buffer: &dyn GpuBackendBuffer) -> Result<u64, &'static str> {
        Err("GPU backend context does not support buffer attachment")
    }

    /// Detach a buffer previously attached to this context.
    ///
    /// # Arguments
    ///
    /// * `buffer` - Backend buffer to detach.
    ///
    /// # Returns
    ///
    /// Nothing after the buffer is no longer attached to this context.
    fn detach_buffer(&self, _buffer: &dyn GpuBackendBuffer) -> Result<(), &'static str> {
        Err("GPU backend context does not support buffer detachment")
    }
}

/// Backend execution queue retained by a [`crate::device::gpu::GpuQueue`].
pub trait GpuBackendQueue: Send + Sync {
    /// Query limits for this backend queue.
    ///
    /// # Returns
    ///
    /// Backend-neutral execution queue information.
    fn query_info(&self) -> GpuBackendQueueInfo;

    /// Synchronously submit one opaque command stream.
    ///
    /// # Arguments
    ///
    /// * `commands` - Bounded backend-defined command bytes owned by the caller.
    ///
    /// # Returns
    ///
    /// Nothing after the backend has completed the submitted work, or an error
    /// if the backend rejected or failed the submission.
    fn submit(&self, commands: &[u8]) -> Result<(), &'static str>;
}

/// A backend that provides GPU information and optional execution capabilities.
pub trait GpuBackend: Send + Sync {
    /// Query the current backend-neutral GPU information.
    ///
    /// # Returns
    ///
    /// Stable device information plus backend-defined opaque identity and data.
    fn query_info(&self) -> GpuBackendInfo;

    /// Query one backend-defined execution dialect.
    ///
    /// # Arguments
    ///
    /// * `index` - Backend-defined dialect index.
    ///
    /// # Returns
    ///
    /// Bounded opaque dialect information, or an error when execution dialects
    /// are not available.
    fn query_dialect(&self, _index: u32) -> Result<GpuBackendDialectInfo, &'static str> {
        Err("GPU backend does not support execution dialect queries")
    }

    /// Create an execution context for a backend-defined dialect.
    ///
    /// # Arguments
    ///
    /// * `dialect` - Opaque selection data from a prior dialect query.
    ///
    /// # Returns
    ///
    /// A backend context that retains the real backend context lifetime, or an
    /// error when execution contexts are not available.
    fn create_context(
        &self,
        _dialect: GpuBackendDialectDescriptor,
    ) -> Result<Arc<dyn GpuBackendContext>, &'static str> {
        Err("GPU backend does not support execution contexts")
    }

    /// Create a backend-owned image with generic usage and kernel-owned backing.
    ///
    /// # Arguments
    ///
    /// * `create` - Validated backend-neutral image creation parameters.
    /// * `backing` - Stable contiguous backing owned by the generic capability.
    ///
    /// # Returns
    ///
    /// A real backend image, or an error when images are unsupported or cannot
    /// be allocated.
    fn create_image(
        &self,
        _create: GpuImageCreateInfo,
        _backing: GpuImageBackingInfo,
    ) -> Result<Arc<dyn GpuBackendImage>, &'static str> {
        Err("GPU backend does not support images")
    }

    /// Create a backend-owned GPU buffer resource from generic page backing.
    ///
    /// # Arguments
    ///
    /// * `create` - Stable contiguous buffer backing owned by the generic capability.
    ///
    /// # Returns
    ///
    /// A real backend buffer, or an error when buffers are unsupported.
    fn create_buffer(
        &self,
        _create: GpuBufferCreateInfo,
    ) -> Result<Arc<dyn GpuBackendBuffer>, &'static str> {
        Err("GPU backend does not support buffers")
    }
}
