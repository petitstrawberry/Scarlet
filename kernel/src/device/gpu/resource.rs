//! Kernel-owned GPU child capability objects.

use alloc::{string::String, sync::Arc, vec::Vec};

use super::connection::{read_user_value, write_user_value};
use super::{
    GPU_ABI_VERSION, GPU_BUFFER_QUERY_INFO, GPU_IMAGE_FORMAT_BGRA8_UNORM, GPU_IMAGE_QUERY_INFO,
    GPU_IMAGE_QUERY_LAYOUT, GPU_IMAGE_USAGE_TRANSFER_DST, GPU_IMAGE_USAGE_VALID,
    GPU_MAX_IMAGE_UPLOAD_SIZE, GPU_RESULT_INVALID_ABI, GPU_TIMELINE_CREATE_POINT,
    GPU_TIMELINE_FAIL, GPU_TIMELINE_QUERY, GPU_TIMELINE_SIGNAL, GpuBackend, GpuBackendBuffer,
    GpuBackendImage, GpuBackendImageLayout, GpuBufferCreateInfo, GpuBufferInfo,
    GpuContextReadbackImageBgra, GpuContextUploadImageBgra, GpuImageBackingInfo,
    GpuImageCreateInfo, GpuImageInfo, GpuImageLayout, GpuImagePlaneLayout, GpuImageUploadInfo,
    GpuTimelineCreatePoint, GpuTimelineFail, GpuTimelineInfo, GpuTimelineSignal,
};
use crate::device::graphics::GpuBackingSegment;
use crate::environment::PAGE_SIZE;
use crate::ipc::shared_memory::{SharedMemoryObject, SharedMemoryPin};
use crate::library::std::usercopy::{copy_from_user, copy_to_user};
use crate::mem::page::{ContiguousPages, Page, allocate_raw_pages, free_raw_pages};
use crate::object::capability::ControlOps;
use crate::object::capability::memory_mapping::{
    AccessKind, AccessOp, MemoryMappingInfo, MemoryMappingOps, ResolveFaultError,
    ResolveFaultResult,
};
use crate::object::capability::selectable::{
    ReadyInterest, ReadySet, SelectWaitOutcome, Selectable,
};
use crate::sched::scheduler::current_task_id;
use crate::sync::{IrqSpinLock, Mutex, waker::Waker};
use crate::vm::addr::{phys_to_virt, virt_to_phys};

/// GPU capability object with explicitly optional kernel-object capabilities.
///
/// Unlike devices and files, GPU child objects do not share a mandatory
/// capability surface. Callers must use these accessors rather than assuming
/// that every GPU object is controllable, mappable, or selectable.
pub trait GpuObject: Send + Sync {
    /// Return control operations when this GPU object supports them.
    ///
    /// # Returns
    ///
    /// The control capability, or `None` when this object cannot be controlled.
    fn as_control_ops(&self) -> Option<&dyn ControlOps> {
        None
    }

    /// Return memory mapping operations when this GPU object supports them.
    ///
    /// # Returns
    ///
    /// The mapping capability, or `None` when this object is not CPU-mappable.
    fn as_memory_mappable(&self) -> Option<&dyn MemoryMappingOps> {
        None
    }

    /// Return selectable readiness operations when this GPU object supports them.
    ///
    /// # Returns
    ///
    /// The readiness capability, or `None` when this object is not selectable.
    fn as_selectable(&self) -> Option<&dyn Selectable> {
        None
    }

    /// Return this object as an execution context when it is one.
    ///
    /// # Returns
    ///
    /// The execution context capability, or `None` for other GPU objects.
    fn as_context(&self) -> Option<&super::GpuContext> {
        None
    }

    /// Return this object as an image when it is one.
    ///
    /// # Returns
    ///
    /// The image capability, or `None` for other GPU objects.
    fn as_image(&self) -> Option<&GpuImage> {
        None
    }

    /// Return this object as a buffer when it is one.
    ///
    /// # Returns
    ///
    /// The buffer capability, or `None` for other GPU objects.
    fn as_buffer(&self) -> Option<&GpuBuffer> {
        None
    }

    /// Return this object as a timeline when it is one.
    ///
    /// # Returns
    ///
    /// The timeline capability, or `None` for other GPU objects.
    fn as_timeline(&self) -> Option<&GpuTimeline> {
        None
    }
}

/// Return whether a generic image descriptor is supported by this ABI phase.
///
/// # Arguments
///
/// * `create` - Image format, usage, and extent to validate.
///
/// # Returns
///
/// `true` for a non-empty BGRA8 image with known color usages, or a
/// `Depth32Float` image used exclusively as a depth-stencil attachment.
pub(crate) const fn image_create_is_valid(create: GpuImageCreateInfo) -> bool {
    (create.format == GPU_IMAGE_FORMAT_BGRA8_UNORM
        || create.format == super::GPU_IMAGE_FORMAT_DEPTH32_FLOAT)
        && create.usage != 0
        && create.usage & !GPU_IMAGE_USAGE_VALID == 0
        && create.width != 0
        && create.height != 0
        && create.width <= u32::MAX / 4
        && if create.format == super::GPU_IMAGE_FORMAT_DEPTH32_FLOAT {
            create.usage == super::GPU_IMAGE_USAGE_DEPTH_STENCIL_ATTACHMENT
        } else {
            create.usage & super::GPU_IMAGE_USAGE_DEPTH_STENCIL_ATTACHMENT == 0
        }
}

/// Return whether a generic image descriptor is valid for imported SHM backing.
pub(crate) const fn imported_image_create_is_valid(create: GpuImageCreateInfo) -> bool {
    image_create_is_valid(create)
        && create.format == GPU_IMAGE_FORMAT_BGRA8_UNORM
        && create.usage == super::GPU_IMAGE_USAGE_SAMPLED | super::GPU_IMAGE_USAGE_TRANSFER_DST
}

/// Fixed SharedMemory layout retained by an imported BGRA image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GpuImportedImageLayout {
    shm_offset: usize,
    source_stride: usize,
    layer_stride: usize,
}

/// Validate one imported image layout against its pinned SharedMemory backing.
pub(crate) fn imported_image_layout(
    create: GpuImageCreateInfo,
    backing_size: usize,
    shm_offset: u64,
    source_stride: u32,
) -> Result<GpuImportedImageLayout, &'static str> {
    if !imported_image_create_is_valid(create) {
        return Err("Imported GPU image descriptor is invalid");
    }
    let shm_offset = usize::try_from(shm_offset)
        .map_err(|_| "Imported GPU image offset does not fit kernel address size")?;
    let source_stride = usize::try_from(source_stride)
        .map_err(|_| "Imported GPU image stride does not fit kernel address size")?;
    let row_bytes = usize::try_from(create.width)
        .map_err(|_| "Imported GPU image width does not fit kernel address size")?
        .checked_mul(4)
        .ok_or("Imported GPU image row width overflows")?;
    if source_stride < row_bytes {
        return Err("Imported GPU image stride is too small");
    }
    let height = usize::try_from(create.height)
        .map_err(|_| "Imported GPU image height does not fit kernel address size")?;
    let required = height
        .checked_sub(1)
        .and_then(|rows| rows.checked_mul(source_stride))
        .and_then(|offset| offset.checked_add(row_bytes))
        .ok_or("Imported GPU image range overflows")?;
    let required_end = shm_offset
        .checked_add(required)
        .ok_or("Imported GPU image range overflows")?;
    if required_end > backing_size {
        return Err("Imported GPU image range exceeds shared memory");
    }
    let layer_stride = source_stride
        .checked_mul(height)
        .ok_or("Imported GPU image layer stride overflows")?;
    u32::try_from(source_stride)
        .map_err(|_| "Imported GPU image stride does not fit backend ABI")?;
    u32::try_from(layer_stride)
        .map_err(|_| "Imported GPU image layer stride does not fit backend ABI")?;
    Ok(GpuImportedImageLayout {
        shm_offset,
        source_stride,
        layer_stride,
    })
}

/// Validate one rectangle transfer from an imported image's fixed backing layout.
pub(crate) fn imported_image_transfer_layout(
    image: super::GpuBackendImageInfo,
    backing_size: usize,
    layout: GpuImportedImageLayout,
    dst_x: u32,
    dst_y: u32,
    width: u32,
    height: u32,
) -> Result<GpuImageUploadInfo, &'static str> {
    if width == 0
        || height == 0
        || image.format != GPU_IMAGE_FORMAT_BGRA8_UNORM
        || image.usage & (super::GPU_IMAGE_USAGE_SAMPLED | GPU_IMAGE_USAGE_TRANSFER_DST)
            != (super::GPU_IMAGE_USAGE_SAMPLED | super::GPU_IMAGE_USAGE_TRANSFER_DST)
    {
        return Err("Imported GPU image transfer request is invalid");
    }
    let image_row_bytes = usize::try_from(image.width)
        .map_err(|_| "Imported GPU image width does not fit kernel address size")?
        .checked_mul(4)
        .ok_or("Imported GPU image row width overflows")?;
    if layout.source_stride < image_row_bytes {
        return Err("Imported GPU image stride is too small");
    }
    let image_height = usize::try_from(image.height)
        .map_err(|_| "Imported GPU image height does not fit kernel address size")?;
    if layout.layer_stride
        != layout
            .source_stride
            .checked_mul(image_height)
            .ok_or("Imported GPU image layer stride overflows")?
    {
        return Err("Imported GPU image layer stride is invalid");
    }
    let dst_x_end = dst_x
        .checked_add(width)
        .ok_or("Imported GPU image transfer x range overflows")?;
    let dst_y_end = dst_y
        .checked_add(height)
        .ok_or("Imported GPU image transfer y range overflows")?;
    if dst_x_end > image.width || dst_y_end > image.height {
        return Err("Imported GPU image transfer rectangle exceeds image bounds");
    }
    let row_bytes = usize::try_from(width)
        .map_err(|_| "Imported GPU image transfer width does not fit kernel address size")?
        .checked_mul(4)
        .ok_or("Imported GPU image transfer row width overflows")?;
    let row_offset = usize::try_from(dst_y)
        .map_err(|_| "Imported GPU image transfer y coordinate does not fit kernel address size")?
        .checked_mul(layout.source_stride)
        .and_then(|offset| {
            usize::try_from(dst_x)
                .ok()
                .and_then(|x| x.checked_mul(4))
                .and_then(|x_offset| offset.checked_add(x_offset))
        })
        .ok_or("Imported GPU image transfer offset overflows")?;
    let backing_offset = layout
        .shm_offset
        .checked_add(row_offset)
        .ok_or("Imported GPU image transfer offset overflows")?;
    let height = usize::try_from(height)
        .map_err(|_| "Imported GPU image transfer height does not fit kernel address size")?;
    let transfer_end = height
        .checked_sub(1)
        .and_then(|rows| rows.checked_mul(layout.source_stride))
        .and_then(|offset| offset.checked_add(backing_offset))
        .and_then(|offset| offset.checked_add(row_bytes))
        .ok_or("Imported GPU image transfer range overflows")?;
    if transfer_end > backing_size {
        return Err("Imported GPU image transfer range exceeds shared memory");
    }
    Ok(GpuImageUploadInfo::new(
        u64::try_from(backing_offset)
            .map_err(|_| "Imported GPU image transfer offset does not fit backend ABI")?,
        u32::try_from(layout.source_stride)
            .map_err(|_| "Imported GPU image stride does not fit backend ABI")?,
        u32::try_from(layout.layer_stride)
            .map_err(|_| "Imported GPU image layer stride does not fit backend ABI")?,
        dst_x,
        dst_y,
        width,
        u32::try_from(height)
            .map_err(|_| "Imported GPU image transfer height does not fit backend ABI")?,
    ))
}

/// Validated layout for one source-to-image BGRA upload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GpuImageUploadLayout {
    source_stride: usize,
    source_row_bytes: usize,
    destination_offset: usize,
    destination_stride: usize,
    height: usize,
    transfer: GpuImageUploadInfo,
}

#[cfg(test)]
impl GpuImageUploadLayout {
    /// Return the backend-neutral transfer metadata derived from this layout.
    pub(crate) const fn transfer(&self) -> GpuImageUploadInfo {
        self.transfer
    }
}

/// Validate a BGRA upload request and derive its kernel-backing layout.
pub(crate) fn image_upload_layout(
    request: &GpuContextUploadImageBgra,
    image: super::GpuBackendImageInfo,
    layout: GpuBackendImageLayout,
) -> Result<GpuImageUploadLayout, &'static str> {
    if request.source_ptr == 0
        || request.source_length == 0
        || request.width == 0
        || request.height == 0
        || image.format != GPU_IMAGE_FORMAT_BGRA8_UNORM
        || image.usage & GPU_IMAGE_USAGE_TRANSFER_DST == 0
        || layout.modifier != super::GPU_IMAGE_MODIFIER_LINEAR
        || layout.plane_count != 1
    {
        return Err("GPU image upload request is invalid");
    }
    let plane = layout.planes[0];
    if plane.block_width != 1 || plane.block_height != 1 || plane.bytes_per_block != 4 {
        return Err("GPU image upload layout is unsupported");
    }
    let image_row_bytes = image
        .width
        .checked_mul(4)
        .ok_or("GPU image row width overflows")?;
    let image_layer_bytes = u64::from(image.height)
        .checked_sub(1)
        .and_then(|rows| rows.checked_mul(u64::from(plane.row_pitch)))
        .and_then(|offset| offset.checked_add(u64::from(image_row_bytes)))
        .ok_or("GPU image layer size overflows")?;
    if plane.row_pitch < image_row_bytes
        || u64::from(plane.array_pitch) < image_layer_bytes
        || plane.size < image_layer_bytes
    {
        return Err("GPU image upload layout is too small for the image extent");
    }

    let row_bytes = request
        .width
        .checked_mul(4)
        .ok_or("GPU image upload row width overflows")?;
    if request.source_stride < row_bytes {
        return Err("GPU image upload source stride is too small");
    }
    let dst_x_end = request
        .dst_x
        .checked_add(request.width)
        .ok_or("GPU image upload x range overflows")?;
    let dst_y_end = request
        .dst_y
        .checked_add(request.height)
        .ok_or("GPU image upload y range overflows")?;
    if dst_x_end > image.width || dst_y_end > image.height {
        return Err("GPU image upload rectangle exceeds image bounds");
    }

    let source_length = usize::try_from(request.source_length)
        .map_err(|_| "GPU image upload source length does not fit kernel address size")?;
    let source_stride = usize::try_from(request.source_stride)
        .map_err(|_| "GPU image upload source stride does not fit kernel address size")?;
    let source_row_bytes = usize::try_from(row_bytes)
        .map_err(|_| "GPU image upload row width does not fit kernel address size")?;
    let height = usize::try_from(request.height)
        .map_err(|_| "GPU image upload height does not fit kernel address size")?;
    let required_source_len = height
        .checked_sub(1)
        .and_then(|rows| rows.checked_mul(source_stride))
        .and_then(|offset| offset.checked_add(source_row_bytes))
        .ok_or("GPU image upload source range overflows")?;
    if source_length < required_source_len {
        return Err("GPU image upload source length is too small");
    }
    let copy_size = source_row_bytes
        .checked_mul(height)
        .ok_or("GPU image upload copy size overflows")?;
    if copy_size > GPU_MAX_IMAGE_UPLOAD_SIZE as usize {
        return Err("GPU image upload exceeds the maximum copy size");
    }

    let destination_stride = usize::try_from(plane.row_pitch)
        .map_err(|_| "GPU image backing stride does not fit kernel address size")?;
    let destination_offset = usize::try_from(request.dst_y)
        .map_err(|_| "GPU image upload y coordinate does not fit kernel address size")?
        .checked_mul(destination_stride)
        .and_then(|offset| {
            usize::try_from(request.dst_x)
                .ok()
                .and_then(|x| x.checked_mul(4))
                .and_then(|x_offset| offset.checked_add(x_offset))
        })
        .and_then(|offset| usize::try_from(plane.offset).ok()?.checked_add(offset))
        .ok_or("GPU image upload destination offset overflows")?;
    let destination_end = height
        .checked_sub(1)
        .and_then(|rows| rows.checked_mul(destination_stride))
        .and_then(|offset| offset.checked_add(destination_offset))
        .and_then(|offset| offset.checked_add(source_row_bytes))
        .ok_or("GPU image upload destination range overflows")?;
    let allocation_size = usize::try_from(image.allocation_size)
        .map_err(|_| "GPU image backing size does not fit kernel address size")?;
    let plane_end = plane
        .offset
        .checked_add(plane.size)
        .and_then(|end| usize::try_from(end).ok())
        .ok_or("GPU image upload plane range overflows")?;
    if destination_end > allocation_size || destination_end > plane_end {
        return Err("GPU image upload exceeds image backing");
    }
    let backing_stride = u32::try_from(destination_stride)
        .map_err(|_| "GPU image backing stride does not fit the backend ABI")?;
    let backing_layer_stride = plane.array_pitch;
    let backing_offset = u64::try_from(destination_offset)
        .map_err(|_| "GPU image backing offset does not fit the backend ABI")?;

    Ok(GpuImageUploadLayout {
        source_stride,
        source_row_bytes,
        destination_offset,
        destination_stride,
        height,
        transfer: GpuImageUploadInfo::new(
            backing_offset,
            backing_stride,
            backing_layer_stride,
            request.dst_x,
            request.dst_y,
            request.width,
            request.height,
        ),
    })
}

/// Validated layout for one image-to-userspace BGRA readback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GpuImageReadbackLayout {
    destination_stride: usize,
    destination_row_bytes: usize,
    destination_offset: usize,
    source_offset: usize,
    source_stride: usize,
    height: usize,
    transfer: GpuImageUploadInfo,
}

/// Validate a BGRA readback request and derive backing and destination ranges.
///
/// # Arguments
///
/// * `request` - Userspace destination and source rectangle.
/// * `image` - Immutable image metadata.
/// * `layout` - Immutable backend-selected image layout.
///
/// # Returns
///
/// Validated pointer-free transfer metadata and CPU copy ranges.
pub(crate) fn image_readback_layout(
    request: &GpuContextReadbackImageBgra,
    image: super::GpuBackendImageInfo,
    layout: GpuBackendImageLayout,
) -> Result<GpuImageReadbackLayout, &'static str> {
    if request.destination_ptr == 0
        || request.destination_length == 0
        || request.width == 0
        || request.height == 0
        || image.format != GPU_IMAGE_FORMAT_BGRA8_UNORM
        || image.usage & super::GPU_IMAGE_USAGE_TRANSFER_SRC == 0
        || layout.modifier != super::GPU_IMAGE_MODIFIER_LINEAR
        || layout.plane_count != 1
    {
        return Err("GPU image readback request is invalid");
    }
    let plane = layout.planes[0];
    if plane.block_width != 1 || plane.block_height != 1 || plane.bytes_per_block != 4 {
        return Err("GPU image readback layout is unsupported");
    }
    let image_row_bytes = image
        .width
        .checked_mul(4)
        .ok_or("GPU image readback row width overflows")?;
    let image_layer_bytes = u64::from(image.height)
        .checked_sub(1)
        .and_then(|rows| rows.checked_mul(u64::from(plane.row_pitch)))
        .and_then(|offset| offset.checked_add(u64::from(image_row_bytes)))
        .ok_or("GPU image readback layer size overflows")?;
    if plane.row_pitch < image_row_bytes
        || u64::from(plane.array_pitch) < image_layer_bytes
        || plane.size < image_layer_bytes
    {
        return Err("GPU image readback layout is too small for the image extent");
    }

    let src_x_end = request
        .src_x
        .checked_add(request.width)
        .ok_or("GPU image readback x range overflows")?;
    let src_y_end = request
        .src_y
        .checked_add(request.height)
        .ok_or("GPU image readback y range overflows")?;
    if src_x_end > image.width || src_y_end > image.height {
        return Err("GPU image readback rectangle exceeds image bounds");
    }
    let row_bytes = request
        .width
        .checked_mul(4)
        .ok_or("GPU image readback row width overflows")?;
    let destination_row_end = src_x_end
        .checked_mul(4)
        .ok_or("GPU image readback destination row overflows")?;
    if request.destination_stride < destination_row_end {
        return Err("GPU image readback destination stride is too small");
    }

    let destination_length = usize::try_from(request.destination_length)
        .map_err(|_| "GPU image readback destination length does not fit kernel address size")?;
    let destination_stride = usize::try_from(request.destination_stride)
        .map_err(|_| "GPU image readback destination stride does not fit kernel address size")?;
    let destination_row_bytes = usize::try_from(row_bytes)
        .map_err(|_| "GPU image readback row width does not fit kernel address size")?;
    let height = usize::try_from(request.height)
        .map_err(|_| "GPU image readback height does not fit kernel address size")?;
    let destination_offset = usize::try_from(request.src_y)
        .ok()
        .and_then(|y| y.checked_mul(destination_stride))
        .and_then(|offset| {
            usize::try_from(request.src_x)
                .ok()
                .and_then(|x| x.checked_mul(4))
                .and_then(|x_offset| offset.checked_add(x_offset))
        })
        .ok_or("GPU image readback destination offset overflows")?;
    let destination_end = height
        .checked_sub(1)
        .and_then(|rows| rows.checked_mul(destination_stride))
        .and_then(|offset| offset.checked_add(destination_offset))
        .and_then(|offset| offset.checked_add(destination_row_bytes))
        .ok_or("GPU image readback destination range overflows")?;
    if destination_end > destination_length {
        return Err("GPU image readback destination range is too small");
    }
    let copy_size = destination_row_bytes
        .checked_mul(height)
        .ok_or("GPU image readback copy size overflows")?;
    if copy_size > GPU_MAX_IMAGE_UPLOAD_SIZE as usize {
        return Err("GPU image readback exceeds the maximum copy size");
    }

    let source_stride = usize::try_from(plane.row_pitch)
        .map_err(|_| "GPU image readback source stride does not fit kernel address size")?;
    let source_offset = usize::try_from(plane.offset)
        .ok()
        .and_then(|offset| {
            usize::try_from(request.src_y)
                .ok()
                .and_then(|y| y.checked_mul(source_stride))
                .and_then(|y_offset| offset.checked_add(y_offset))
        })
        .and_then(|offset| {
            usize::try_from(request.src_x)
                .ok()
                .and_then(|x| x.checked_mul(4))
                .and_then(|x_offset| offset.checked_add(x_offset))
        })
        .ok_or("GPU image readback source offset overflows")?;
    let source_end = height
        .checked_sub(1)
        .and_then(|rows| rows.checked_mul(source_stride))
        .and_then(|offset| offset.checked_add(source_offset))
        .and_then(|offset| offset.checked_add(destination_row_bytes))
        .ok_or("GPU image readback source range overflows")?;
    if source_end > usize::try_from(layout.total_size).unwrap_or(0) {
        return Err("GPU image readback exceeds image backing");
    }

    Ok(GpuImageReadbackLayout {
        destination_stride,
        destination_row_bytes,
        destination_offset,
        source_offset,
        source_stride,
        height,
        transfer: GpuImageUploadInfo::new(
            u64::try_from(source_offset)
                .map_err(|_| "GPU image readback offset does not fit backend ABI")?,
            plane.row_pitch,
            plane.array_pitch,
            request.src_x,
            request.src_y,
            request.width,
            request.height,
        ),
    })
}

/// Stable private page-backed allocation retained by a GPU image and its attachments.
struct GpuPrivateImageBacking {
    chunks: Vec<ContiguousPages>,
    physical_segments: Arc<[GpuBackingSegment]>,
    allocation_size: usize,
}

impl GpuPrivateImageBacking {
    const MAX_CHUNK_PAGES: usize = 512;

    fn new(layout: GpuBackendImageLayout, allow_segmented: bool) -> Result<Self, &'static str> {
        if !layout.is_valid() {
            return Err("GPU backend image layout is invalid");
        }
        let image_size = usize::try_from(layout.total_size)
            .map_err(|_| "GPU image layout size does not fit kernel address size")?;
        if layout.alignment > PAGE_SIZE as u64 {
            return Err("GPU image backing alignment exceeds the generic allocator");
        }
        let allocation_size = image_size
            .checked_add(PAGE_SIZE - 1)
            .ok_or("GPU image backing size overflows page alignment")?
            & !(PAGE_SIZE - 1);
        let page_count = allocation_size / PAGE_SIZE;
        if let Some(pages) = ContiguousPages::new(page_count) {
            return Self::from_chunks(alloc::vec![pages], allocation_size);
        }
        if !allow_segmented {
            return Err("Failed to allocate contiguous GPU image pages");
        }

        let mut chunks = Vec::new();
        chunks
            .try_reserve(page_count.div_ceil(Self::MAX_CHUNK_PAGES))
            .map_err(|_| "Failed to reserve segmented GPU image backing")?;
        let mut remaining_pages = page_count;
        while remaining_pages != 0 {
            let mut chunk_pages = remaining_pages.min(Self::MAX_CHUNK_PAGES);
            let chunk = loop {
                if let Some(chunk) = ContiguousPages::new(chunk_pages) {
                    break chunk;
                }
                if chunk_pages == 1 {
                    return Err("Failed to allocate segmented GPU image pages");
                }
                chunk_pages = (chunk_pages / 2).max(1);
            };
            remaining_pages -= chunk.len();
            chunks.push(chunk);
        }
        Self::from_chunks(chunks, allocation_size)
    }

    fn from_chunks(
        chunks: Vec<ContiguousPages>,
        allocation_size: usize,
    ) -> Result<Self, &'static str> {
        let mut physical_segments = Vec::new();
        physical_segments
            .try_reserve_exact(chunks.len())
            .map_err(|_| "Failed to reserve GPU image segment metadata")?;
        let mut total_size = 0usize;
        for chunk in &chunks {
            let length = chunk
                .len()
                .checked_mul(PAGE_SIZE)
                .ok_or("GPU image segment length overflows")?;
            // SAFETY: every chunk is an exclusive live allocation covering
            // `length` bytes and is not visible to a backend yet.
            unsafe {
                core::ptr::write_bytes(chunk.as_vaddr() as *mut u8, 0, length);
            }
            total_size = total_size
                .checked_add(length)
                .ok_or("GPU image backing size overflows")?;
            physical_segments.push(GpuBackingSegment::new(chunk.as_paddr(), length));
        }
        if total_size != allocation_size {
            return Err("Segmented GPU image backing size is inconsistent");
        }
        Ok(Self {
            chunks,
            physical_segments: Arc::from(physical_segments),
            allocation_size,
        })
    }

    fn for_each_range(
        &self,
        offset: usize,
        length: usize,
        mut operation: impl FnMut(usize, usize) -> Result<(), &'static str>,
    ) -> Result<(), &'static str> {
        let end = offset
            .checked_add(length)
            .ok_or("GPU image backing range overflows")?;
        if end > self.allocation_size {
            return Err("GPU image backing range exceeds allocation");
        }
        let mut logical_base = 0usize;
        let mut cursor = offset;
        let mut remaining = length;
        for chunk in &self.chunks {
            if remaining == 0 {
                break;
            }
            let chunk_length = chunk
                .len()
                .checked_mul(PAGE_SIZE)
                .ok_or("GPU image chunk length overflows")?;
            let chunk_end = logical_base
                .checked_add(chunk_length)
                .ok_or("GPU image chunk range overflows")?;
            if cursor < chunk_end {
                let chunk_offset = cursor.saturating_sub(logical_base);
                let part_length = remaining.min(chunk_length - chunk_offset);
                let address = chunk
                    .as_vaddr()
                    .checked_add(chunk_offset)
                    .ok_or("GPU image chunk address overflows")?;
                operation(address, part_length)?;
                cursor += part_length;
                remaining -= part_length;
            }
            logical_base = chunk_end;
        }
        if remaining == 0 {
            Ok(())
        } else {
            Err("GPU image backing range is incomplete")
        }
    }

    fn copy_from_user(
        &self,
        task: &crate::task::Task,
        source_address: usize,
        destination_offset: usize,
        length: usize,
    ) -> Result<(), &'static str> {
        let mut copied = 0usize;
        self.for_each_range(destination_offset, length, |destination, part_length| {
            let source = source_address
                .checked_add(copied)
                .ok_or("GPU image upload source address overflows")?;
            // SAFETY: `for_each_range` supplies an exclusive live chunk range;
            // the image upload mutex serializes writers to this backing.
            let destination =
                unsafe { core::slice::from_raw_parts_mut(destination as *mut u8, part_length) };
            copy_from_user(task, source, destination)
                .map_err(|_| "Failed to copy GPU image pixels from user")?;
            copied += part_length;
            Ok(())
        })
    }

    fn clean_range(&self, offset: usize, length: usize) -> Result<(), &'static str> {
        self.for_each_range(offset, length, |address, part_length| {
            crate::arch::clean_dcache_to_poc_range(address, part_length);
            Ok(())
        })
    }

    fn copy_to_user(
        &self,
        task: &crate::task::Task,
        destination_address: usize,
        source_offset: usize,
        length: usize,
    ) -> Result<(), &'static str> {
        let mut copied = 0usize;
        self.for_each_range(source_offset, length, |source, part_length| {
            crate::arch::invalidate_dcache_to_poc_range(source, part_length);
            // SAFETY: `for_each_range` supplies a live initialized backing
            // range retained by this image for the complete copy operation.
            let source = unsafe { core::slice::from_raw_parts(source as *const u8, part_length) };
            let destination = destination_address
                .checked_add(copied)
                .ok_or("GPU image readback destination address overflows")?;
            copy_to_user(task, destination, source)
                .map_err(|_| "Failed to copy GPU image pixels to user")?;
            copied += part_length;
            Ok(())
        })
    }
}

/// Fixed layout of a sampled BGRA image imported from SharedMemory.
struct GpuImportedImageBacking {
    pin: SharedMemoryPin,
    layout: GpuImportedImageLayout,
}

/// Private image backing that is either allocated by the kernel or pinned from SharedMemory.
pub(crate) enum GpuImageBacking {
    Private(GpuPrivateImageBacking),
    Imported(GpuImportedImageBacking),
}

impl GpuImageBacking {
    fn private(layout: GpuBackendImageLayout, allow_segmented: bool) -> Result<Self, &'static str> {
        Ok(Self::Private(GpuPrivateImageBacking::new(
            layout,
            allow_segmented,
        )?))
    }

    fn imported(
        create: GpuImageCreateInfo,
        shared_memory: Arc<dyn SharedMemoryObject>,
        shm_offset: u64,
        source_stride: u32,
    ) -> Result<Self, &'static str> {
        let shm_size = shared_memory.size();
        let pin = SharedMemoryPin::new(shared_memory, 0, shm_size)?;
        let layout =
            imported_image_layout(create, pin.backing().size(), shm_offset, source_stride)?;
        Ok(Self::Imported(GpuImportedImageBacking { pin, layout }))
    }

    fn info(&self) -> Result<GpuImageBackingInfo, &'static str> {
        match self {
            Self::Private(backing) => Ok(GpuImageBackingInfo::new_segmented(
                Arc::clone(&backing.physical_segments),
                u64::try_from(backing.allocation_size)
                    .map_err(|_| "GPU image backing size does not fit backend metadata")?,
            )),
            Self::Imported(backing) => {
                let backing = backing.pin.backing();
                Ok(GpuImageBackingInfo::new(
                    backing.paddr(),
                    u64::try_from(backing.size()).map_err(
                        |_| "Imported GPU image backing size does not fit backend metadata",
                    )?,
                ))
            }
        }
    }

    fn private_backing(&self) -> Result<&GpuPrivateImageBacking, &'static str> {
        match self {
            Self::Private(backing) => Ok(backing),
            Self::Imported(_) => Err("Imported GPU images cannot copy pixels from userspace"),
        }
    }

    fn physical_segments_for_range(
        &self,
        offset: usize,
        length: usize,
    ) -> Result<Arc<[GpuBackingSegment]>, &'static str> {
        let (segments, allocation_size) = match self {
            Self::Private(backing) => (
                Arc::clone(&backing.physical_segments),
                backing.allocation_size,
            ),
            Self::Imported(backing) => {
                let backing = backing.pin.backing();
                (
                    Arc::from([GpuBackingSegment::new(backing.paddr(), backing.size())]),
                    backing.size(),
                )
            }
        };
        let end = offset
            .checked_add(length)
            .ok_or("GPU image physical range overflows")?;
        if end > allocation_size {
            return Err("GPU image physical range exceeds backing");
        }
        if offset == 0 && length == allocation_size {
            return Ok(segments);
        }

        let mut result = Vec::new();
        let mut logical_base = 0usize;
        let mut cursor = offset;
        let mut remaining = length;
        for segment in segments.iter().copied() {
            if remaining == 0 {
                break;
            }
            let segment_end = logical_base
                .checked_add(segment.length())
                .ok_or("GPU image physical segment range overflows")?;
            if cursor < segment_end {
                let segment_offset = cursor.saturating_sub(logical_base);
                let part_length = remaining.min(segment.length() - segment_offset);
                result.push(GpuBackingSegment::new(
                    segment
                        .physical_addr()
                        .checked_add(segment_offset)
                        .ok_or("GPU image physical segment address overflows")?,
                    part_length,
                ));
                cursor += part_length;
                remaining -= part_length;
            }
            logical_base = segment_end;
        }
        if remaining != 0 {
            return Err("GPU image physical range is incomplete");
        }
        Ok(Arc::from(result))
    }

    fn imported_transfer_layout(
        &self,
        image: super::GpuBackendImageInfo,
        dst_x: u32,
        dst_y: u32,
        width: u32,
        height: u32,
    ) -> Result<GpuImageUploadInfo, &'static str> {
        let Self::Imported(backing) = self else {
            return Err("GPU image does not use imported shared memory");
        };
        imported_image_transfer_layout(
            image,
            backing.pin.backing().size(),
            backing.layout,
            dst_x,
            dst_y,
            width,
            height,
        )
    }

    fn clean_imported_transfer_range(
        &self,
        transfer: GpuImageUploadInfo,
    ) -> Result<(), &'static str> {
        let Self::Imported(backing) = self else {
            return Err("GPU image does not use imported shared memory");
        };
        let transfer_offset = usize::try_from(transfer.backing_offset)
            .map_err(|_| "Imported GPU image transfer offset does not fit kernel address size")?;
        let transfer_stride = usize::try_from(transfer.backing_stride)
            .map_err(|_| "Imported GPU image transfer stride does not fit kernel address size")?;
        let transfer_height = usize::try_from(transfer.height)
            .map_err(|_| "Imported GPU image transfer height does not fit kernel address size")?;
        let row_bytes = usize::try_from(transfer.width)
            .map_err(|_| "Imported GPU image transfer width does not fit kernel address size")?
            .checked_mul(4)
            .ok_or("Imported GPU image transfer row width overflows")?;
        if transfer_stride != backing.layout.source_stride
            || usize::try_from(transfer.backing_layer_stride)
                .map_err(|_| "Imported GPU image layer stride does not fit kernel address size")?
                != backing.layout.layer_stride
            || transfer_height == 0
        {
            return Err("Imported GPU image transfer layout changed unexpectedly");
        }
        let transfer_length = transfer_height
            .checked_sub(1)
            .and_then(|rows| rows.checked_mul(transfer_stride))
            .and_then(|offset| offset.checked_add(row_bytes))
            .ok_or("Imported GPU image transfer range overflows")?;
        let backing_info = backing.pin.backing();
        let transfer_end = transfer_offset
            .checked_add(transfer_length)
            .ok_or("Imported GPU image transfer range overflows")?;
        if transfer_end > backing_info.size() {
            return Err("Imported GPU image transfer range exceeds shared memory");
        }
        let direct_map_vaddr = phys_to_virt(backing_info.paddr())
            .checked_add(transfer_offset)
            .ok_or("Imported GPU image direct-map address overflows")?;
        // The SharedMemory pin retains this contiguous backing unchanged until
        // transfer completion, so its physical direct-map address remains valid.
        crate::arch::clean_dcache_to_poc_range(direct_map_vaddr, transfer_length);
        Ok(())
    }
}

/// Kernel-owned backend image capability.
pub struct GpuImage {
    backend_image: Arc<dyn GpuBackendImage>,
    backing: Arc<GpuImageBacking>,
    layout: GpuBackendImageLayout,
    upload_lock: Mutex<()>,
}

impl GpuImage {
    /// Create a real backend image with kernel-owned backing.
    ///
    /// # Arguments
    ///
    /// * `backend` - Backend that creates and retains the real image resource.
    /// * `create` - Generic descriptor used to create the image.
    ///
    /// # Returns
    ///
    /// A capability retaining the backend image and its backing, or an error
    /// when allocation, backend creation, or immutable metadata validation fails.
    pub fn new(
        backend: Arc<dyn GpuBackend>,
        create: GpuImageCreateInfo,
    ) -> Result<Self, &'static str> {
        if !image_create_is_valid(create) {
            return Err("GPU image descriptor is invalid");
        }
        let layout = backend.plan_image(create)?;
        if !layout.is_valid() {
            return Err("GPU backend returned an invalid image layout");
        }
        let backing = Arc::new(GpuImageBacking::private(
            layout,
            backend.supports_segmented_image_backing(),
        )?);
        let backing_info = backing.info()?;
        if backing_info.allocation_size < layout.total_size {
            return Err("GPU image backing is smaller than the backend layout");
        }
        let backing_allocation_size = backing_info.allocation_size;
        let backend_image = backend.create_image_with_layout(create, layout, backing_info)?;
        let info = backend_image.query_info();
        if info.format != create.format
            || info.usage != create.usage
            || info.width != create.width
            || info.height != create.height
            || info.command_resource_token == 0
            || info.allocation_size != backing_allocation_size
        {
            return Err("GPU backend image metadata is invalid");
        }
        Ok(Self {
            backend_image,
            backing,
            layout,
            upload_lock: Mutex::new(()),
        })
    }

    /// Create a sampled BGRA image backed by a pinned SharedMemory object.
    ///
    /// # Arguments
    ///
    /// * `backend` - Backend that creates and retains the real image resource.
    /// * `create` - Validated sampled BGRA image descriptor.
    /// * `shared_memory` - Strong SharedMemory capability owner retained by the image.
    /// * `shm_offset` - Byte offset of pixel `(0, 0)` in SharedMemory.
    /// * `source_stride` - Number of bytes between SharedMemory source rows.
    ///
    /// # Returns
    ///
    /// A capability retaining both the backend image and its pinned SharedMemory
    /// backing, or an error when any layout or backend validation fails.
    pub(crate) fn new_imported(
        backend: Arc<dyn GpuBackend>,
        create: GpuImageCreateInfo,
        shared_memory: Arc<dyn SharedMemoryObject>,
        shm_offset: u64,
        source_stride: u32,
    ) -> Result<Self, &'static str> {
        let layout = backend.plan_image(create)?;
        if !layout.is_valid() {
            return Err("GPU backend returned an invalid imported image layout");
        }
        let backing = Arc::new(GpuImageBacking::imported(
            create,
            shared_memory,
            shm_offset,
            source_stride,
        )?);
        let backing_info = backing.info()?;
        if backing_info.allocation_size < layout.total_size {
            return Err("Imported GPU image backing is smaller than the backend layout");
        }
        let backing_allocation_size = backing_info.allocation_size;
        let backend_image = backend.create_image_with_layout(create, layout, backing_info)?;
        let info = backend_image.query_info();
        if info.format != create.format
            || info.usage != create.usage
            || info.width != create.width
            || info.height != create.height
            || info.command_resource_token == 0
            || info.allocation_size != backing_allocation_size
        {
            return Err("GPU backend imported image metadata is invalid");
        }
        Ok(Self {
            backend_image,
            backing,
            layout,
            upload_lock: Mutex::new(()),
        })
    }

    /// Return immutable backend-neutral image information.
    ///
    /// # Returns
    ///
    /// The image format, usage, extent, command token, and allocation size.
    pub fn query_info(&self) -> super::GpuBackendImageInfo {
        self.backend_image.query_info()
    }
    /// Return the immutable backend-selected image layout.
    ///
    /// # Returns
    ///
    /// Exact modifier, allocation, and plane metadata fixed at image creation.
    pub const fn layout(&self) -> GpuBackendImageLayout {
        self.layout
    }

    /// Clone the backend image for a context lifetime reference.
    ///
    /// # Returns
    ///
    /// A strong reference to the real backend image.
    pub(crate) fn backend_image(&self) -> Arc<dyn GpuBackendImage> {
        Arc::clone(&self.backend_image)
    }

    /// Clone the image backing for a context lifetime reference.
    pub(crate) fn backing(&self) -> Arc<GpuImageBacking> {
        Arc::clone(&self.backing)
    }

    pub(crate) fn upload_bgra_from_user<F>(
        &self,
        source_ptr: usize,
        layout: GpuImageUploadLayout,
        transfer: F,
    ) -> Result<(), &'static str>
    where
        F: FnOnce(&dyn GpuBackendImage, GpuImageUploadInfo) -> Result<(), &'static str>,
    {
        let _upload_guard = self.upload_lock.lock();
        let task = crate::task::mytask().ok_or("No current task for GPU image upload")?;
        let backing = self.backing.private_backing()?;
        for row in 0..layout.height {
            let source_offset = row
                .checked_mul(layout.source_stride)
                .ok_or("GPU image upload source row offset overflows")?;
            let source_address = source_ptr
                .checked_add(source_offset)
                .ok_or("GPU image upload source address overflows")?;
            let destination_offset = row
                .checked_mul(layout.destination_stride)
                .and_then(|offset| offset.checked_add(layout.destination_offset))
                .ok_or("GPU image upload destination row offset overflows")?;
            backing.copy_from_user(
                &task,
                source_address,
                destination_offset,
                layout.source_row_bytes,
            )?;
        }
        for row in 0..layout.height {
            let destination_offset = row
                .checked_mul(layout.destination_stride)
                .and_then(|offset| offset.checked_add(layout.destination_offset))
                .ok_or("GPU image upload destination row offset overflows")?;
            backing.clean_range(destination_offset, layout.source_row_bytes)?;
        }
        transfer(self.backend_image.as_ref(), layout.transfer)
    }

    pub(crate) fn transfer_imported_bgra<F>(
        &self,
        dst_x: u32,
        dst_y: u32,
        width: u32,
        height: u32,
        transfer: F,
    ) -> Result<(), &'static str>
    where
        F: FnOnce(&dyn GpuBackendImage, GpuImageUploadInfo) -> Result<(), &'static str>,
    {
        let _upload_guard = self.upload_lock.lock();
        let layout = self.backing.imported_transfer_layout(
            self.query_info(),
            dst_x,
            dst_y,
            width,
            height,
        )?;
        self.backing.clean_imported_transfer_range(layout)?;
        transfer(self.backend_image.as_ref(), layout)
    }

    pub(crate) fn readback_bgra_to_user<F>(
        &self,
        destination_ptr: usize,
        layout: GpuImageReadbackLayout,
        readback: F,
    ) -> Result<(), &'static str>
    where
        F: FnOnce(&dyn GpuBackendImage, GpuImageUploadInfo) -> Result<(), &'static str>,
    {
        let _upload_guard = self.upload_lock.lock();
        let task = crate::task::mytask().ok_or("No current task for GPU image readback")?;
        let backing = self.backing.private_backing()?;
        readback(self.backend_image.as_ref(), layout.transfer)?;
        for row in 0..layout.height {
            let source_offset = row
                .checked_mul(layout.source_stride)
                .and_then(|offset| offset.checked_add(layout.source_offset))
                .ok_or("GPU image readback source row offset overflows")?;
            let destination_address = row
                .checked_mul(layout.destination_stride)
                .and_then(|offset| offset.checked_add(layout.destination_offset))
                .and_then(|offset| destination_ptr.checked_add(offset))
                .ok_or("GPU image readback destination row address overflows")?;
            backing.copy_to_user(
                &task,
                destination_address,
                source_offset,
                layout.destination_row_bytes,
            )?;
        }
        Ok(())
    }

    /// Return this image's display descriptor when it is presentable.
    ///
    /// # Returns
    ///
    /// An internal descriptor accepted by the graphics display bridge.
    pub(crate) fn display_resource(&self) -> Option<crate::device::graphics::GpuDisplayResource> {
        if let Some(resource) = self.backend_image.display_resource() {
            return Some(resource);
        }
        let layout = self.backend_image.linear_display_info()?;
        let image = self.backend_image.query_info();
        let backing = self.backing.info().ok()?;
        let offset = usize::try_from(layout.offset).ok()?;
        let allocation_size = backing.allocation_size.checked_sub(layout.offset)?;
        let allocation_size_usize = usize::try_from(allocation_size).ok()?;
        let physical_segments = self
            .backing
            .physical_segments_for_range(offset, allocation_size_usize)
            .ok()?;
        let owner: Arc<dyn crate::device::graphics::GpuDisplayBackingOwner> = self.backing.clone();
        crate::device::graphics::GpuDisplayResource::new_linear_segments(
            physical_segments,
            allocation_size,
            image.width,
            image.height,
            layout.stride,
            layout.format,
            owner,
        )
        .ok()
    }

    fn fill_query_layout(&self, response: &mut GpuImageLayout) {
        response.clear_response();
        if response.abi_version != GPU_ABI_VERSION {
            response.result = GPU_RESULT_INVALID_ABI;
            return;
        }
        let layout = self.layout;
        response.modifier = layout.modifier;
        response.total_size = layout.total_size;
        response.alignment = layout.alignment;
        response.plane_count = layout.plane_count;
        for (destination, source) in response
            .planes
            .iter_mut()
            .zip(layout.planes.iter())
            .take(layout.plane_count as usize)
        {
            *destination = GpuImagePlaneLayout {
                offset: source.offset,
                size: source.size,
                row_pitch: source.row_pitch,
                array_pitch: source.array_pitch,
                block_width: source.block_width,
                block_height: source.block_height,
                bytes_per_block: source.bytes_per_block,
                reserved: 0,
            };
        }
    }
    fn handle_query_layout(&self, arg: usize) -> Result<i32, &'static str> {
        let mut response: GpuImageLayout = read_user_value(arg)?;
        self.fill_query_layout(&mut response);
        write_user_value(arg, &response)?;
        Ok(0)
    }
    fn fill_query_info(&self, info: &mut GpuImageInfo) {
        info.clear_response();
        if info.abi_version != GPU_ABI_VERSION {
            info.result = GPU_RESULT_INVALID_ABI;
            return;
        }
        let backend_info = self.query_info();
        info.format = backend_info.format;
        info.usage = backend_info.usage;
        info.width = backend_info.width;
        info.height = backend_info.height;
        info.command_resource_token = backend_info.command_resource_token;
        info.allocation_size = backend_info.allocation_size;
    }

    fn handle_query_info(&self, arg: usize) -> Result<i32, &'static str> {
        let mut info: GpuImageInfo = read_user_value(arg)?;
        self.fill_query_info(&mut info);
        write_user_value(arg, &info)?;
        Ok(0)
    }
}

impl ControlOps for GpuImage {
    fn control(&self, command: u32, arg: usize) -> Result<i32, &'static str> {
        match command {
            GPU_IMAGE_QUERY_INFO => self.handle_query_info(arg),
            GPU_IMAGE_QUERY_LAYOUT => self.handle_query_layout(arg),
            _ => Err("Unsupported GPU image control command"),
        }
    }
}

impl GpuObject for GpuImage {
    fn as_control_ops(&self) -> Option<&dyn ControlOps> {
        Some(self)
    }

    fn as_image(&self) -> Option<&GpuImage> {
        Some(self)
    }
}

/// Stable page-backed allocation retained by a GPU buffer and its attachments.
pub(crate) struct GpuBufferBacking {
    paddr: usize,
    allocation_size: usize,
}

impl Drop for GpuBufferBacking {
    fn drop(&mut self) {
        let page_count = self.allocation_size / PAGE_SIZE;
        if page_count != 0 {
            let pages = phys_to_virt(self.paddr) as *mut Page;
            free_raw_pages(pages, page_count);
        }
    }
}

/// Kernel-owned, fixed-size GPU buffer.
pub struct GpuBuffer {
    backing: Arc<GpuBufferBacking>,
    backend_buffer: Arc<dyn GpuBackendBuffer>,
    flags: u32,
}

impl GpuBuffer {
    /// Allocate a page-backed GPU buffer.
    ///
    /// # Arguments
    ///
    /// * `backend` - Backend state retained independently of the creating connection.
    /// * `size_bytes` - Requested non-zero byte size.
    /// * `flags` - Validated GPU buffer creation flags.
    ///
    /// # Returns
    ///
    /// A buffer with a stable, contiguous page allocation.
    pub fn new(
        backend: Arc<dyn GpuBackend>,
        size_bytes: usize,
        flags: u32,
    ) -> Result<Self, &'static str> {
        if size_bytes == 0 {
            return Err("GPU buffer size must be non-zero");
        }
        let allocation_size = size_bytes
            .checked_add(PAGE_SIZE - 1)
            .ok_or("GPU buffer size overflows page alignment")?
            & !(PAGE_SIZE - 1);
        let page_count = allocation_size / PAGE_SIZE;
        let pages = allocate_raw_pages(page_count);
        if pages.is_null() {
            return Err("Failed to allocate GPU buffer pages");
        }
        // SAFETY: `pages` is a valid contiguous allocation of `allocation_size`
        // bytes returned by `allocate_raw_pages`; zeroing initializes it before
        // any userspace mapping can observe the buffer.
        unsafe {
            core::ptr::write_bytes(pages as *mut u8, 0, allocation_size);
        }
        let backing = Arc::new(GpuBufferBacking {
            paddr: virt_to_phys(pages as usize),
            allocation_size,
        });
        let backend_buffer = backend.create_buffer(GpuBufferCreateInfo::new(
            backing.paddr,
            u64::try_from(backing.allocation_size)
                .map_err(|_| "GPU buffer allocation size does not fit backend metadata")?,
        ))?;
        let backend_info = backend_buffer.query_info();
        if backend_info.command_resource_token == 0
            || backend_info.allocation_size != backing.allocation_size as u64
        {
            return Err("GPU backend buffer metadata is invalid");
        }
        Ok(Self {
            backing,
            backend_buffer,
            flags,
        })
    }

    /// Return whether the buffer may be CPU mapped.
    ///
    /// # Returns
    ///
    /// `true` when the CPU-visible creation flag was requested.
    pub const fn cpu_visible(&self) -> bool {
        (self.flags & super::GPU_BUFFER_FLAG_CPU_VISIBLE) != 0
    }

    /// Return the page-rounded backing allocation size.
    ///
    /// # Returns
    ///
    /// The stable contiguous allocation size in bytes.
    pub fn allocation_size(&self) -> usize {
        self.backing.allocation_size
    }

    /// Return immutable backend-neutral buffer information.
    ///
    /// # Returns
    ///
    /// The opaque command resource token and allocation size.
    pub fn backend_info(&self) -> super::GpuBackendBufferInfo {
        self.backend_buffer.query_info()
    }

    /// Clone the backend buffer for a context lifetime reference.
    pub(crate) fn backend_buffer(&self) -> Arc<dyn GpuBackendBuffer> {
        Arc::clone(&self.backend_buffer)
    }

    /// Clone the page backing for a context lifetime reference.
    pub(crate) fn backing(&self) -> Arc<GpuBufferBacking> {
        Arc::clone(&self.backing)
    }

    fn query_info(&self, info: &mut GpuBufferInfo) {
        info.clear_response();
        if info.abi_version != GPU_ABI_VERSION {
            info.result = GPU_RESULT_INVALID_ABI;
            return;
        }
        info.flags = self.flags;
        info.size_bytes = self.allocation_size() as u64;
        info.cpu_visible = u32::from(self.cpu_visible());
        let backend_info = self.backend_info();
        info.command_resource_token = backend_info.command_resource_token;
        info.allocation_size = backend_info.allocation_size;
    }

    fn handle_query_info(&self, arg: usize) -> Result<i32, &'static str> {
        let mut info = read_user_value(arg)?;
        self.query_info(&mut info);
        write_user_value(arg, &info)?;
        Ok(0)
    }
}

impl MemoryMappingOps for GpuBuffer {
    fn get_mapping_info(
        &self,
        offset: usize,
        length: usize,
    ) -> Result<MemoryMappingInfo, &'static str> {
        if !self.cpu_visible() {
            return Err("GPU buffer is not CPU-visible");
        }
        if offset % PAGE_SIZE != 0 || length == 0 || length % PAGE_SIZE != 0 {
            return Err("GPU buffer mappings must be non-empty and page-aligned");
        }
        let end = offset
            .checked_add(length)
            .ok_or("GPU buffer mapping range overflows")?;
        if end > self.allocation_size() {
            return Err("GPU buffer mapping range exceeds allocation");
        }
        let paddr = self
            .backing
            .paddr
            .checked_add(offset)
            .ok_or("GPU buffer physical address overflows")?;
        Ok(MemoryMappingInfo::new(paddr, 0x3, true))
    }

    fn get_mapping_info_with(
        &self,
        offset: usize,
        length: usize,
        is_shared: bool,
    ) -> Result<MemoryMappingInfo, &'static str> {
        if !is_shared {
            return Err("GPU buffers require shared CPU mappings");
        }
        self.get_mapping_info(offset, length)
    }

    fn supports_private_mmap(&self) -> bool {
        false
    }

    fn resolve_fault(
        &self,
        access: &AccessKind,
        page_idx: usize,
        _vm_start: usize,
    ) -> Result<ResolveFaultResult, ResolveFaultError> {
        if !self.cpu_visible() {
            return Err(ResolveFaultError::Invalid);
        }
        if matches!(access.op, AccessOp::Instruction) {
            return Err(ResolveFaultError::Invalid);
        }
        let offset = page_idx
            .checked_mul(PAGE_SIZE)
            .ok_or(ResolveFaultError::Invalid)?;
        if offset >= self.allocation_size() {
            return Err(ResolveFaultError::Unmapped);
        }
        Ok(ResolveFaultResult {
            paddr_page_base: self
                .backing
                .paddr
                .checked_add(offset)
                .ok_or(ResolveFaultError::Invalid)?,
            is_tail: false,
        })
    }

    fn supports_mmap(&self) -> bool {
        self.cpu_visible()
    }

    fn mmap_owner_name(&self) -> String {
        String::from("gpu-buffer")
    }
}

impl ControlOps for GpuBuffer {
    fn control(&self, command: u32, arg: usize) -> Result<i32, &'static str> {
        match command {
            GPU_BUFFER_QUERY_INFO => self.handle_query_info(arg),
            _ => Err("Unsupported GPU buffer control command"),
        }
    }
}

impl GpuObject for GpuBuffer {
    fn as_control_ops(&self) -> Option<&dyn ControlOps> {
        Some(self)
    }

    fn as_memory_mappable(&self) -> Option<&dyn MemoryMappingOps> {
        self.cpu_visible().then_some(self)
    }

    fn as_buffer(&self) -> Option<&GpuBuffer> {
        Some(self)
    }
}

struct GpuTimelineState {
    value: u64,
    failed: bool,
}

struct GpuTimelineShared {
    state: IrqSpinLock<GpuTimelineState>,
    waker: Waker,
}

/// Monotonic GPU timeline with sticky failure state.
pub struct GpuTimeline {
    backend: Arc<dyn GpuBackend>,
    shared: Arc<GpuTimelineShared>,
}

impl GpuTimeline {
    /// Create a timeline with an initial value.
    ///
    /// # Arguments
    ///
    /// * `backend` - Backend state retained independently of the creating connection.
    /// * `initial_value` - Initial completed timeline value.
    ///
    /// # Returns
    ///
    /// A monotonic timeline.
    pub fn new(backend: Arc<dyn GpuBackend>, initial_value: u64) -> Self {
        Self {
            backend,
            shared: Arc::new(GpuTimelineShared {
                state: IrqSpinLock::new(GpuTimelineState {
                    value: initial_value,
                    failed: false,
                }),
                waker: Waker::new_interruptible("gpu_timeline"),
            }),
        }
    }

    /// Query the current timeline value and failure state.
    ///
    /// # Returns
    ///
    /// `(value, failed)` for this timeline.
    pub fn state(&self) -> (u64, bool) {
        let state = self.shared.state.lock();
        (state.value, state.failed)
    }

    /// Advance the timeline monotonically and wake point waiters.
    ///
    /// # Arguments
    ///
    /// * `value` - New completed value, which must not be less than the current value.
    ///
    /// # Returns
    ///
    /// The completed value after signalling.
    pub fn signal(&self, value: u64) -> Result<u64, &'static str> {
        {
            let mut state = self.shared.state.lock();
            if state.failed {
                return Err("GPU timeline has failed");
            }
            if value < state.value {
                return Err("GPU timeline signal would decrease value");
            }
            state.value = value;
        }
        self.shared.waker.wake_all();
        Ok(value)
    }

    /// Put the timeline into its sticky failed state and wake point waiters.
    ///
    /// # Returns
    ///
    /// Nothing. The failed state remains set for the lifetime of the timeline.
    pub fn fail(&self) {
        {
            let mut state = self.shared.state.lock();
            state.failed = true;
        }
        self.shared.waker.wake_all();
    }

    fn is_ready_for(&self, target: u64) -> bool {
        let state = self.shared.state.lock();
        state.failed || state.value >= target
    }

    fn query_info(&self, info: &mut GpuTimelineInfo) {
        info.clear_response();
        if info.abi_version != GPU_ABI_VERSION {
            info.result = GPU_RESULT_INVALID_ABI;
            return;
        }
        let (value, failed) = self.state();
        info.current_value = value;
        info.failed = u32::from(failed);
        let _ = &self.backend;
    }

    fn handle_query(&self, arg: usize) -> Result<i32, &'static str> {
        let mut info = read_user_value(arg)?;
        self.query_info(&mut info);
        write_user_value(arg, &info)?;
        Ok(0)
    }

    fn handle_signal(&self, arg: usize) -> Result<i32, &'static str> {
        let mut request: GpuTimelineSignal = read_user_value(arg)?;
        let reserved = request.reserved;
        request.clear_response();
        if request.abi_version != GPU_ABI_VERSION {
            request.result = GPU_RESULT_INVALID_ABI;
        } else if reserved != 0 {
            request.result = super::GPU_RESULT_INVALID_ARGUMENT;
        } else if self.signal(request.value).is_err() {
            request.result = super::GPU_RESULT_INVALID_STATE;
        }
        let (value, failed) = self.state();
        request.current_value = value;
        request.failed = u32::from(failed);
        write_user_value(arg, &request)?;
        Ok(0)
    }

    fn handle_fail(&self, arg: usize) -> Result<i32, &'static str> {
        let mut request: GpuTimelineFail = read_user_value(arg)?;
        let reserved = request.reserved;
        request.clear_response();
        if request.abi_version != GPU_ABI_VERSION {
            request.result = GPU_RESULT_INVALID_ABI;
        } else if reserved != 0 {
            request.result = super::GPU_RESULT_INVALID_ARGUMENT;
        } else {
            self.fail();
        }
        let (value, failed) = self.state();
        request.current_value = value;
        request.failed = u32::from(failed);
        write_user_value(arg, &request)?;
        Ok(0)
    }

    fn handle_create_point(&self, arg: usize) -> Result<i32, &'static str> {
        let mut request: GpuTimelineCreatePoint = read_user_value(arg)?;
        request.clear_response();
        if request.abi_version != GPU_ABI_VERSION {
            request.result = GPU_RESULT_INVALID_ABI;
            write_user_value(arg, &request)?;
            return Ok(0);
        }
        let task = crate::task::mytask().ok_or("No current task for GPU timeline point")?;
        let point: Arc<dyn GpuObject> = Arc::new(GpuTimelinePoint::new(
            Arc::new(self.clone_for_point()),
            request.target_value,
        ));
        let handle = task.handle_table.insert_with_metadata(
            crate::object::KernelObject::Gpu(point),
            super::child_handle_metadata(crate::object::handle::AccessMode::ReadOnly),
        );
        match handle {
            Ok(handle) => {
                request.point_handle = handle;
                let (value, failed) = self.state();
                request.current_value = value;
                request.failed = u32::from(failed);
                if let Err(error) = write_user_value(arg, &request) {
                    task.handle_table.remove(handle);
                    return Err(error);
                }
                Ok(0)
            }
            Err(_) => {
                request.result = super::GPU_RESULT_OUT_OF_RESOURCES;
                write_user_value(arg, &request)?;
                Ok(0)
            }
        }
    }

    fn clone_for_point(&self) -> Self {
        Self {
            backend: Arc::clone(&self.backend),
            shared: Arc::clone(&self.shared),
        }
    }

    fn wait_for_target(
        &self,
        target: u64,
        interest: ReadyInterest,
        trapframe: &mut crate::arch::Trapframe,
        timeout_ns: Option<u64>,
        min_wait_ns: u64,
    ) -> SelectWaitOutcome {
        if self.is_ready_for(target) {
            return SelectWaitOutcome::Ready;
        }
        if !interest.read && !interest.except {
            return SelectWaitOutcome::TimedOut;
        }
        let cpu_id = crate::arch::get_cpu().get_cpuid();
        let task_id = current_task_id(cpu_id).unwrap_or(0);
        let deadline_ns =
            timeout_ns.map(|duration| crate::timer::get_time_ns().saturating_add(duration));
        let mut first_wait = true;
        loop {
            let remaining_timeout = match deadline_ns {
                Some(deadline) => {
                    let remaining = deadline.saturating_sub(crate::timer::get_time_ns());
                    if remaining == 0 {
                        return SelectWaitOutcome::TimedOut;
                    }
                    Some(remaining)
                }
                None => None,
            };
            let woke = if first_wait && min_wait_ns > 0 {
                self.shared.waker.wait_with_min_timeout(
                    task_id,
                    trapframe,
                    remaining_timeout,
                    min_wait_ns,
                )
            } else {
                self.shared
                    .waker
                    .wait_with_timeout(task_id, trapframe, remaining_timeout)
            };
            first_wait = false;
            if self.is_ready_for(target) {
                return SelectWaitOutcome::Ready;
            }
            if !woke {
                return SelectWaitOutcome::TimedOut;
            }
        }
    }
}

impl ControlOps for GpuTimeline {
    fn control(&self, command: u32, arg: usize) -> Result<i32, &'static str> {
        match command {
            GPU_TIMELINE_QUERY => self.handle_query(arg),
            GPU_TIMELINE_SIGNAL => self.handle_signal(arg),
            GPU_TIMELINE_FAIL => self.handle_fail(arg),
            GPU_TIMELINE_CREATE_POINT => self.handle_create_point(arg),
            _ => Err("Unsupported GPU timeline control command"),
        }
    }
}

impl GpuObject for GpuTimeline {
    fn as_control_ops(&self) -> Option<&dyn ControlOps> {
        Some(self)
    }

    fn as_timeline(&self) -> Option<&GpuTimeline> {
        Some(self)
    }
}

/// Fixed-target readiness point for a GPU timeline.
pub struct GpuTimelinePoint {
    timeline: Arc<GpuTimeline>,
    target_value: u64,
}

impl GpuTimelinePoint {
    /// Create a point that becomes ready once `timeline` reaches `target_value`.
    ///
    /// # Arguments
    ///
    /// * `timeline` - Timeline state retained by this point.
    /// * `target_value` - Fixed completed value that makes the point ready.
    ///
    /// # Returns
    ///
    /// A selectable timeline point.
    pub fn new(timeline: Arc<GpuTimeline>, target_value: u64) -> Self {
        Self {
            timeline,
            target_value,
        }
    }
}

impl Selectable for GpuTimelinePoint {
    fn current_ready(&self, interest: ReadyInterest) -> ReadySet {
        let ready = self.timeline.is_ready_for(self.target_value);
        ReadySet {
            read: ready && interest.read,
            write: false,
            except: ready && interest.except,
        }
    }

    fn wait_until_ready(
        &self,
        interest: ReadyInterest,
        trapframe: &mut crate::arch::Trapframe,
        timeout_ns: Option<u64>,
        min_wait_ns: u64,
    ) -> SelectWaitOutcome {
        self.timeline.wait_for_target(
            self.target_value,
            interest,
            trapframe,
            timeout_ns,
            min_wait_ns,
        )
    }
}

impl GpuObject for GpuTimelinePoint {
    fn as_selectable(&self) -> Option<&dyn Selectable> {
        Some(self)
    }
}
