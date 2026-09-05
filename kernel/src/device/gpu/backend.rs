//! Backend-neutral GPU information model.

use alloc::sync::Arc;

use super::{GPU_BACKEND_ID_BYTES, GPU_BACKEND_INFO_BYTES};
use crate::device::graphics::{GpuBackingSegment, GpuDisplayResource, PixelFormat};

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
/// Generic synchronous image readback operations are available.
pub const GPU_EXECUTION_SUPPORT_IMAGE_READBACK: u32 = 1 << 7;

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

/// Maximum image planes represented by the generic GPU layout model.
pub const GPU_BACKEND_IMAGE_MAX_PLANES: usize = 4;
/// Generic modifier value for an uncompressed linear image.
pub const GPU_IMAGE_MODIFIER_LINEAR: u64 = 0;

/// Immutable layout of one backend image plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuBackendImagePlaneLayout {
    /// Byte offset of the first plane element in the backing allocation.
    pub offset: u64,
    /// Number of bytes occupied by this plane.
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
}

impl GpuBackendImagePlaneLayout {
    /// Empty plane value used for unused entries in the fixed-capacity array.
    pub const EMPTY: Self = Self {
        offset: 0,
        size: 0,
        row_pitch: 0,
        array_pitch: 0,
        block_width: 0,
        block_height: 0,
        bytes_per_block: 0,
    };
}

/// Immutable backend-selected image layout and allocation requirements.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuBackendImageLayout {
    /// Backend-neutral or backend-specific memory modifier.
    pub modifier: u64,
    /// Exact bytes required by all image planes.
    pub total_size: u64,
    /// Required physical backing alignment in bytes.
    pub alignment: u64,
    /// Number of initialized entries in `planes`.
    pub plane_count: u32,
    /// Fixed-capacity plane layouts.
    pub planes: [GpuBackendImagePlaneLayout; GPU_BACKEND_IMAGE_MAX_PLANES],
}

impl GpuBackendImageLayout {
    /// Build the default one-plane, tightly packed 32-bit image layout.
    ///
    /// # Arguments
    ///
    /// * `create` - Validated generic image descriptor.
    ///
    /// # Returns
    ///
    /// A linear layout, or an error when its size cannot be represented.
    pub fn tight_32bpp(create: GpuImageCreateInfo) -> Result<Self, &'static str> {
        let row_pitch = create
            .width
            .checked_mul(4)
            .ok_or("GPU image row pitch overflows")?;
        Self::linear_32bpp(create, row_pitch, 1)
    }

    /// Build a one-plane linear 32-bit layout with backend-selected pitch.
    ///
    /// # Arguments
    ///
    /// * `create` - Validated generic image descriptor.
    /// * `row_pitch` - Number of bytes between adjacent rows.
    /// * `alignment` - Required physical backing alignment.
    ///
    /// # Returns
    ///
    /// A validated linear layout, or an error for invalid pitch/alignment/size.
    pub fn linear_32bpp(
        create: GpuImageCreateInfo,
        row_pitch: u32,
        alignment: u64,
    ) -> Result<Self, &'static str> {
        let row_bytes = create
            .width
            .checked_mul(4)
            .ok_or("GPU image row size overflows")?;
        if create.width == 0
            || create.height == 0
            || row_pitch < row_bytes
            || alignment == 0
            || !alignment.is_power_of_two()
        {
            return Err("GPU image linear layout is invalid");
        }
        let size = u64::from(row_pitch)
            .checked_mul(u64::from(create.height))
            .ok_or("GPU image layout size overflows")?;
        let array_pitch =
            u32::try_from(size).map_err(|_| "GPU image layer pitch exceeds the backend ABI")?;
        let mut planes = [GpuBackendImagePlaneLayout::EMPTY; GPU_BACKEND_IMAGE_MAX_PLANES];
        planes[0] = GpuBackendImagePlaneLayout {
            offset: 0,
            size,
            row_pitch,
            array_pitch,
            block_width: 1,
            block_height: 1,
            bytes_per_block: 4,
        };
        Ok(Self {
            modifier: GPU_IMAGE_MODIFIER_LINEAR,
            total_size: size,
            alignment,
            plane_count: 1,
            planes,
        })
    }

    /// Validate structural bounds required by generic allocation and upload.
    ///
    /// # Returns
    ///
    /// `true` when initialized planes are non-overflowing and fit `total_size`.
    pub fn is_valid(&self) -> bool {
        if self.total_size == 0
            || self.alignment == 0
            || !self.alignment.is_power_of_two()
            || self.plane_count == 0
            || self.plane_count as usize > GPU_BACKEND_IMAGE_MAX_PLANES
        {
            return false;
        }
        for plane in &self.planes[..self.plane_count as usize] {
            if plane.size == 0
                || plane.row_pitch == 0
                || plane.array_pitch == 0
                || plane.block_width == 0
                || plane.block_height == 0
                || plane.bytes_per_block == 0
                || plane
                    .offset
                    .checked_add(plane.size)
                    .is_none_or(|end| end > self.total_size)
            {
                return false;
            }
        }
        self.planes[self.plane_count as usize..]
            .iter()
            .all(|plane| *plane == GpuBackendImagePlaneLayout::EMPTY)
    }
}
/// Backend-neutral physical backing for a GPU image resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuImageBackingInfo {
    /// Physical address of the first backing extent.
    pub paddr: usize,
    /// Page-rounded allocation size in bytes.
    pub allocation_size: u64,
    physical_segments: Arc<[GpuBackingSegment]>,
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
    pub fn new(paddr: usize, allocation_size: u64) -> Self {
        let segment_length = usize::try_from(allocation_size).unwrap_or(0);
        Self {
            paddr,
            allocation_size,
            physical_segments: Arc::from([GpuBackingSegment::new(paddr, segment_length)]),
        }
    }

    /// Build image backing information from ordered physical extents.
    ///
    /// The extents are logically concatenated and retained by the generic
    /// image owner for the complete backend resource lifetime.
    pub fn new_segmented(
        physical_segments: Arc<[GpuBackingSegment]>,
        allocation_size: u64,
    ) -> Self {
        let paddr = physical_segments
            .first()
            .map(|segment| segment.physical_addr())
            .unwrap_or(0);
        Self {
            paddr,
            allocation_size,
            physical_segments,
        }
    }

    /// Return the ordered physical extents forming this logical allocation.
    pub fn physical_segments(&self) -> &[GpuBackingSegment] {
        &self.physical_segments
    }

    /// Return whether this backing is one physically contiguous extent.
    pub fn is_physically_contiguous(&self) -> bool {
        self.physical_segments.len() == 1
            && self
                .physical_segments
                .first()
                .is_some_and(|segment| segment.length() as u64 >= self.allocation_size)
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

/// Linear scanout layout exported by a backend image.
///
/// The generic [`crate::device::gpu::GpuImage`] combines this layout with its
/// real backing allocation and supplies the strong lifetime owner required by
/// the display subsystem. Backends must not fabricate an owner or physical
/// address in this descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuBackendLinearDisplayInfo {
    /// Byte offset of pixel `(0, 0)` within the image backing.
    pub offset: u64,
    /// Number of bytes between adjacent rows.
    pub stride: u32,
    /// Pixel format consumed by the display controller.
    pub format: PixelFormat,
}

impl GpuBackendLinearDisplayInfo {
    /// Build a linear display layout descriptor.
    ///
    /// # Arguments
    ///
    /// * `offset` - Byte offset of pixel `(0, 0)` in the backing allocation.
    /// * `stride` - Number of bytes between adjacent rows.
    /// * `format` - Display-controller pixel format.
    ///
    /// # Returns
    ///
    /// A backend-neutral linear layout descriptor.
    pub const fn new(offset: u64, stride: u32, format: PixelFormat) -> Self {
        Self {
            offset,
            stride,
            format,
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

    /// Return a linear scanout layout backed by this image's generic allocation.
    ///
    /// # Returns
    ///
    /// The layout needed to construct a cross-device display resource, or
    /// `None` when the image is not linear or is not presentable. The generic
    /// image object supplies the physical address and lifetime owner.
    fn linear_display_info(&self) -> Option<GpuBackendLinearDisplayInfo> {
        None
    }
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
    /// A non-zero opaque attachment token authorized only for this context, or
    /// an error when the image does not belong to this backend or cannot attach.
    /// This token is not required to equal the image resource identity token.
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

    /// Read a rectangle from an attached image into its generic CPU backing.
    ///
    /// # Arguments
    ///
    /// * `image` - Backend image retained by the calling kernel capability.
    /// * `readback` - Validated image-backing rectangle to populate.
    ///
    /// # Returns
    ///
    /// Nothing after the backend has synchronously completed GPU-to-backing
    /// transfer, or an error when readback is unavailable.
    fn readback_image_bgra(
        &self,
        _image: &dyn GpuBackendImage,
        _readback: GpuImageUploadInfo,
    ) -> Result<(), &'static str> {
        Err("GPU backend context does not support image readback")
    }

    /// Attach a buffer so opaque commands in this context may reference it.
    ///
    /// # Arguments
    ///
    /// * `buffer` - Backend buffer retained by the calling kernel capability.
    ///
    /// # Returns
    ///
    /// A non-zero opaque attachment token authorized only for this context.
    /// This token is not required to equal the buffer resource identity token.
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

/// Classified failure from a backend queue submission.
///
/// Rejected command bytes are a per-submit error and must not poison the queue
/// or a timeline. Only a confirmed hardware fault, timeout, or reset should be
/// reported as [`Self::DeviceLost`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuBackendSubmitError {
    /// The copied backend command payload is invalid or unauthorized.
    Rejected(&'static str),
    /// The backend cannot execute this submit in its current state, but has not
    /// confirmed that the device was lost.
    Unavailable(&'static str),
    /// A hardware timeout, fault, or reset made the device unusable.
    DeviceLost(&'static str),
}

/// Async admission result with explicit ownership on side-effect-free rejection.
#[derive(Debug)]
pub enum GpuBackendEnqueueError {
    /// No work accepted; return the entire request without waiting for capacity.
    Busy(super::GpuSubmission),
    /// No work accepted; return the request and a classified rejection.
    Rejected(GpuBackendSubmitError, super::GpuSubmission),
    /// A prefix may be accepted. The backend must independently retain the
    /// request and eventually settle its completion, including on handle close.
    Failed(GpuBackendSubmitError),
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
    fn submit(&self, commands: &[u8]) -> Result<(), GpuBackendSubmitError>;

    /// Query the bounded number of asynchronously retained submissions.
    ///
    /// # Returns
    ///
    /// Zero when async submission is not implemented, otherwise a queue limit.
    /// The generic layer may impose a smaller bound. This is not a promise that
    /// a slot is currently available and must not wait for hardware progress.
    fn async_capacity(&self) -> u32 {
        0
    }

    /// Enqueue owned work without waiting for its GPU completion or a free slot.
    ///
    /// # Arguments
    ///
    /// * `submission` - Owned commands, backing, attachment authority, and a
    ///   kernel-only completion producer. An empty stream is a queue checkpoint.
    ///   Generic attachment locks remain held during this call, so no detach can
    ///   race validation/enqueue. Retain backend mappings for the snapshot after
    ///   return; all accepted requests must progress without observer polling.
    ///
    /// # Returns
    ///
    /// Success after acceptance, not after GPU retirement. Busy/Rejected must
    /// return the unchanged request and certify no work from it was accepted.
    /// Failed may cover a prefix and must leave all possibly accepted work owned
    /// independently by the driver. Completion covers earlier queue work too.
    /// Never substitute the synchronous `submit` implementation for this method.
    fn enqueue(&self, submission: super::GpuSubmission) -> Result<(), GpuBackendEnqueueError> {
        Err(GpuBackendEnqueueError::Rejected(
            GpuBackendSubmitError::Unavailable("GPU backend does not support asynchronous submit"),
            submission,
        ))
    }
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

    /// Return whether image backing may contain multiple physical extents.
    ///
    /// Backends returning `true` must map the ordered extents into one logical
    /// device address range before exposing the image to command execution.
    fn supports_segmented_image_backing(&self) -> bool {
        false
    }

    /// Plan immutable image layout before the generic backing is allocated.
    ///
    /// # Arguments
    ///
    /// * `create` - Validated backend-neutral image creation parameters.
    ///
    /// # Returns
    ///
    /// Exact allocation and plane layout requirements. The default preserves
    /// the existing tightly packed 32-bit layout for backends such as VirtIO.
    fn plan_image(
        &self,
        create: GpuImageCreateInfo,
    ) -> Result<GpuBackendImageLayout, &'static str> {
        GpuBackendImageLayout::tight_32bpp(create)
    }

    /// Create a backend image using the exact pre-allocation layout plan.
    ///
    /// # Arguments
    ///
    /// * `create` - Validated backend-neutral image creation parameters.
    /// * `layout` - Immutable layout returned by [`Self::plan_image`].
    /// * `backing` - Stable contiguous backing owned by the generic capability.
    ///
    /// # Returns
    ///
    /// A real backend image. Existing backends may inherit the compatibility
    /// implementation; native backends should verify and retain `layout`.
    fn create_image_with_layout(
        &self,
        create: GpuImageCreateInfo,
        _layout: GpuBackendImageLayout,
        backing: GpuImageBackingInfo,
    ) -> Result<Arc<dyn GpuBackendImage>, &'static str> {
        self.create_image(create, backing)
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
