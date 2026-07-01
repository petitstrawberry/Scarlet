//! GPU device interface.
//!
//! This module defines the non-framebuffer GPU interface used by accelerated
//! backends such as virtio-gpu virgl. Display scanout remains in
//! `device::graphics`; this module is for GPU contexts, resources, command
//! submission, and fences.

use alloc::vec::Vec;

use super::{
    Device,
    graphics::{GpuDisplayResource, GraphicsDevice, output::DisplayRegion},
};
use crate::library::std::usercopy::{copy_from_user, copy_to_user};

/// Optional GPU backend features.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuFeature {
    /// Virtio-gpu virgl command submission is available.
    Virgl,
    /// Host-visible blob resources are available.
    ResourceBlob,
    /// Explicit GPU fences are available.
    Fences,
    /// Context initialization parameters are available.
    ContextInit,
}

/// GPU backend capability summary.
#[derive(Debug, Clone)]
pub struct GpuCapabilities {
    /// Supported feature list.
    pub features: Vec<GpuFeature>,
    /// Number of capsets exposed by the backend.
    pub capset_count: u32,
}

impl GpuCapabilities {
    /// Create an empty capability set.
    ///
    /// # Returns
    ///
    /// A capability set with no optional GPU features.
    pub fn empty() -> Self {
        Self {
            features: Vec::new(),
            capset_count: 0,
        }
    }

    /// Check whether a feature is present.
    ///
    /// # Arguments
    ///
    /// * `feature` - Feature to check.
    ///
    /// # Returns
    ///
    /// `true` if the feature is present.
    pub fn contains(&self, feature: GpuFeature) -> bool {
        self.features.iter().any(|item| *item == feature)
    }
}

/// GPU capset metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuCapsetInfo {
    /// Backend-defined capset identifier.
    pub id: u32,
    /// Highest supported capset version.
    pub max_version: u32,
    /// Maximum byte size of this capset.
    pub max_size: u32,
}

/// GPU command submission descriptor.
pub struct GpuCommandSubmission<'a> {
    /// GPU context that owns the command stream.
    pub context_id: u32,
    /// Backend-specific command bytes.
    pub commands: &'a [u8],
    /// Optional fence identifier signaled when the submission completes.
    pub fence_id: Option<u64>,
}

/// GPU 3D resource creation descriptor.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GpuResource3dDescription {
    /// Caller-selected resource identifier, or `0` to let the backend allocate one.
    pub resource_id: u32,
    /// Backend-specific resource target.
    pub target: u32,
    /// Backend-specific resource format.
    pub format: u32,
    /// Backend-specific bind flags.
    pub bind: u32,
    /// Resource width in pixels or elements.
    pub width: u32,
    /// Resource height in pixels or elements.
    pub height: u32,
    /// Resource depth in pixels or elements.
    pub depth: u32,
    /// Array layer count.
    pub array_size: u32,
    /// Last mip level.
    pub last_level: u32,
    /// Multisample count.
    pub nr_samples: u32,
    /// Backend-specific creation flags.
    pub flags: u32,
}

/// Physical memory segment attached to a GPU resource.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GpuMemoryEntry {
    /// Physical base address visible to the device.
    pub paddr: usize,
    /// Segment length in bytes.
    pub length: usize,
}

/// GPU 3D transfer descriptor.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GpuTransfer3d {
    /// GPU context associated with the transfer.
    pub context_id: u32,
    /// Resource identifier.
    pub resource_id: u32,
    /// Optional fence identifier signaled when the transfer completes.
    pub fence_id: Option<u64>,
    /// Offset into the attached backing memory.
    pub offset: u64,
    /// Mip level.
    pub level: u32,
    /// Row stride in bytes.
    pub stride: u32,
    /// Layer stride in bytes.
    pub layer_stride: u32,
    /// X origin.
    pub x: u32,
    /// Y origin.
    pub y: u32,
    /// Z origin.
    pub z: u32,
    /// Transfer width.
    pub width: u32,
    /// Transfer height.
    pub height: u32,
    /// Transfer depth.
    pub depth: u32,
}

/// Non-framebuffer GPU interface.
///
/// `GpuDevice` is intentionally separate from `GraphicsDevice`: a device may
/// provide scanout-only display, GPU acceleration, or both.
pub trait GpuDevice: Device {
    /// Get the GPU backend capabilities.
    ///
    /// # Returns
    ///
    /// Capability summary for this GPU backend.
    fn gpu_capabilities(&self) -> GpuCapabilities;

    /// Get metadata for a backend capset.
    ///
    /// # Arguments
    ///
    /// * `index` - Zero-based capset index.
    ///
    /// # Returns
    ///
    /// Capset metadata or an error describing why it is unavailable.
    fn get_capset_info(&self, _index: u32) -> Result<GpuCapsetInfo, &'static str> {
        Err("GPU capsets are not supported")
    }

    /// Read a backend capset into a caller-provided buffer.
    ///
    /// # Arguments
    ///
    /// * `id` - Backend-defined capset identifier.
    /// * `version` - Capset version to read.
    /// * `buffer` - Destination buffer for capset bytes.
    ///
    /// # Returns
    ///
    /// Number of bytes written or an error describing why the capset is unavailable.
    fn read_capset(
        &self,
        _id: u32,
        _version: u32,
        _buffer: &mut [u8],
    ) -> Result<usize, &'static str> {
        Err("GPU capsets are not supported")
    }

    /// Create a GPU execution context.
    ///
    /// # Arguments
    ///
    /// * `context_id` - Caller-selected context identifier.
    /// * `debug_name` - Optional human-readable context name.
    ///
    /// # Returns
    ///
    /// Success or an error describing why context creation failed.
    fn create_context(&self, _context_id: u32, _debug_name: &str) -> Result<(), &'static str> {
        Err("GPU contexts are not supported")
    }

    /// Destroy a GPU execution context.
    ///
    /// # Arguments
    ///
    /// * `context_id` - Context identifier to destroy.
    ///
    /// # Returns
    ///
    /// Success or an error describing why context destruction failed.
    fn destroy_context(&self, _context_id: u32) -> Result<(), &'static str> {
        Err("GPU contexts are not supported")
    }

    /// Create a backend 3D resource.
    ///
    /// # Arguments
    ///
    /// * `description` - Resource target, format, dimensions, and creation flags.
    ///
    /// # Returns
    ///
    /// Allocated resource identifier or an error describing why creation failed.
    fn create_3d_resource(
        &self,
        _description: GpuResource3dDescription,
    ) -> Result<u32, &'static str> {
        Err("GPU 3D resources are not supported")
    }

    /// Unreference a backend resource.
    ///
    /// # Arguments
    ///
    /// * `resource_id` - Resource identifier to release.
    ///
    /// # Returns
    ///
    /// Success or an error describing why release failed.
    fn unref_resource(&self, _resource_id: u32) -> Result<(), &'static str> {
        Err("GPU resources are not supported")
    }

    /// Attach a resource to a GPU context.
    ///
    /// # Arguments
    ///
    /// * `context_id` - Context that should see the resource.
    /// * `resource_id` - Resource to attach.
    ///
    /// # Returns
    ///
    /// Success or an error describing why attachment failed.
    fn attach_resource(&self, _context_id: u32, _resource_id: u32) -> Result<(), &'static str> {
        Err("GPU resources are not supported")
    }

    /// Attach guest memory backing to a resource.
    ///
    /// # Arguments
    ///
    /// * `resource_id` - Resource that should use the backing memory.
    /// * `entries` - Physical memory segments backing the resource.
    ///
    /// # Returns
    ///
    /// Success or an error describing why backing attachment failed.
    fn attach_resource_backing(
        &self,
        _resource_id: u32,
        _entries: &[GpuMemoryEntry],
    ) -> Result<(), &'static str> {
        Err("GPU resource backing is not supported")
    }

    /// Detach guest memory backing from a resource.
    ///
    /// # Arguments
    ///
    /// * `resource_id` - Resource whose backing should be detached.
    ///
    /// # Returns
    ///
    /// Success or an error describing why backing detachment failed.
    fn detach_resource_backing(&self, _resource_id: u32) -> Result<(), &'static str> {
        Err("GPU resource backing is not supported")
    }

    /// Detach a resource from a GPU context.
    ///
    /// # Arguments
    ///
    /// * `context_id` - Context that owns the attachment.
    /// * `resource_id` - Resource to detach.
    ///
    /// # Returns
    ///
    /// Success or an error describing why detachment failed.
    fn detach_resource(&self, _context_id: u32, _resource_id: u32) -> Result<(), &'static str> {
        Err("GPU resources are not supported")
    }

    /// Transfer attached backing memory to a host 3D resource.
    ///
    /// # Arguments
    ///
    /// * `transfer` - Transfer region, strides, and optional fence.
    ///
    /// # Returns
    ///
    /// Success or an error describing why the transfer failed.
    fn transfer_to_host_3d(&self, _transfer: GpuTransfer3d) -> Result<(), &'static str> {
        Err("GPU 3D transfers are not supported")
    }

    /// Transfer a host 3D resource into attached backing memory.
    ///
    /// # Arguments
    ///
    /// * `transfer` - Transfer region, strides, and optional fence.
    ///
    /// # Returns
    ///
    /// Success or an error describing why the transfer failed.
    fn transfer_from_host_3d(&self, _transfer: GpuTransfer3d) -> Result<(), &'static str> {
        Err("GPU 3D transfers are not supported")
    }

    /// Submit a backend-specific GPU command stream.
    ///
    /// # Arguments
    ///
    /// * `submission` - Command stream, target context, and optional fence.
    ///
    /// # Returns
    ///
    /// Success or an error describing why submission failed.
    fn submit_commands(&self, _submission: GpuCommandSubmission<'_>) -> Result<(), &'static str> {
        Err("GPU command submission is not supported")
    }
}

/// GPU character-device control commands.
pub mod gpu_commands {
    /// Query GPU backend capabilities.
    pub const GPU_GET_CAPABILITIES: u32 = 0x4700;
    /// Query one capset's metadata.
    pub const GPU_GET_CAPSET_INFO: u32 = 0x4701;
    /// Read capset bytes into a user-provided buffer.
    pub const GPU_READ_CAPSET: u32 = 0x4702;
    /// Create a GPU execution context.
    pub const GPU_CREATE_CONTEXT: u32 = 0x4703;
    /// Destroy a GPU execution context.
    pub const GPU_DESTROY_CONTEXT: u32 = 0x4704;
    /// Submit backend-specific GPU commands.
    pub const GPU_SUBMIT_COMMANDS: u32 = 0x4705;
    /// Create a backend 3D resource.
    pub const GPU_CREATE_3D_RESOURCE: u32 = 0x4706;
    /// Unreference a backend resource.
    pub const GPU_UNREF_RESOURCE: u32 = 0x4707;
    /// Attach a resource to a GPU context.
    pub const GPU_ATTACH_RESOURCE: u32 = 0x4708;
    /// Detach a resource from a GPU context.
    pub const GPU_DETACH_RESOURCE: u32 = 0x4709;
    /// Attach userspace memory backing to a resource.
    pub const GPU_ATTACH_RESOURCE_BACKING: u32 = 0x470a;
    /// Detach memory backing from a resource.
    pub const GPU_DETACH_RESOURCE_BACKING: u32 = 0x470b;
    /// Transfer attached backing memory to a host 3D resource.
    pub const GPU_TRANSFER_TO_HOST_3D: u32 = 0x470c;
    /// Transfer a host 3D resource into attached backing memory.
    pub const GPU_TRANSFER_FROM_HOST_3D: u32 = 0x470d;
    /// Present a GPU resource through the display pipeline.
    pub const GPU_PRESENT_RESOURCE: u32 = 0x470e;
}

/// Capability bit for virgl command submission.
pub const GPU_CAPABILITY_VIRGL: u32 = 1 << 0;
/// Capability bit for host-visible blob resources.
pub const GPU_CAPABILITY_RESOURCE_BLOB: u32 = 1 << 1;
/// Capability bit for explicit fences.
pub const GPU_CAPABILITY_FENCES: u32 = 1 << 2;
/// Capability bit for context initialization parameters.
pub const GPU_CAPABILITY_CONTEXT_INIT: u32 = 1 << 3;
/// Submission flag indicating `fence_id` should be used.
pub const GPU_SUBMIT_FLAG_FENCE: u32 = 1 << 0;
/// Transfer flag indicating `fence_id` should be used.
pub const GPU_TRANSFER_FLAG_FENCE: u32 = 1 << 0;
/// Maximum debug context name accepted by the neutral GPU ABI.
pub const GPU_CONTEXT_NAME_MAX: usize = 64;

/// Userspace GPU capability response.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GpuCapabilitiesInfo {
    /// Bitset of `GPU_CAPABILITY_*` values.
    pub feature_bits: u32,
    /// Number of backend capsets available.
    pub capset_count: u32,
}

/// Userspace capset info request/response.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GpuCapsetInfoRequest {
    /// Zero-based capset index supplied by userspace.
    pub index: u32,
    /// Backend-defined capset identifier returned by the kernel.
    pub id: u32,
    /// Highest supported capset version returned by the kernel.
    pub max_version: u32,
    /// Maximum capset byte size returned by the kernel.
    pub max_size: u32,
}

/// Userspace capset read request.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GpuReadCapsetRequest {
    /// Backend-defined capset identifier.
    pub id: u32,
    /// Capset version to read.
    pub version: u32,
    /// Userspace destination buffer pointer.
    pub buffer_ptr: usize,
    /// Destination buffer length in bytes.
    pub buffer_len: usize,
    /// Number of bytes copied by the kernel.
    pub bytes_written: usize,
}

/// Userspace GPU context request.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GpuContextRequest {
    /// Caller-selected GPU context identifier.
    pub context_id: u32,
    /// Optional userspace debug-name pointer.
    pub name_ptr: usize,
    /// Debug-name length in bytes.
    pub name_len: usize,
}

/// Userspace GPU command submission request.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GpuSubmitRequest {
    /// GPU context that owns the command stream.
    pub context_id: u32,
    /// Submission flags such as `GPU_SUBMIT_FLAG_FENCE`.
    pub flags: u32,
    /// Optional fence identifier.
    pub fence_id: u64,
    /// Userspace command buffer pointer.
    pub commands_ptr: usize,
    /// Command buffer length in bytes.
    pub commands_len: usize,
}

/// Userspace 3D resource creation request.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GpuCreate3dResourceRequest {
    /// Caller-selected resource identifier, or `0` to let the backend allocate one.
    pub resource_id: u32,
    /// Backend-specific resource target.
    pub target: u32,
    /// Backend-specific resource format.
    pub format: u32,
    /// Backend-specific bind flags.
    pub bind: u32,
    /// Resource width in pixels or elements.
    pub width: u32,
    /// Resource height in pixels or elements.
    pub height: u32,
    /// Resource depth in pixels or elements.
    pub depth: u32,
    /// Array layer count.
    pub array_size: u32,
    /// Last mip level.
    pub last_level: u32,
    /// Multisample count.
    pub nr_samples: u32,
    /// Backend-specific creation flags.
    pub flags: u32,
}

/// Userspace resource-only request.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GpuResourceRequest {
    /// Resource identifier.
    pub resource_id: u32,
}

/// Userspace context/resource association request.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GpuContextResourceRequest {
    /// Context identifier.
    pub context_id: u32,
    /// Resource identifier.
    pub resource_id: u32,
}

/// Userspace resource backing attachment request.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GpuAttachResourceBackingRequest {
    /// Resource identifier.
    pub resource_id: u32,
    /// Userspace buffer pointer.
    pub buffer_ptr: usize,
    /// Userspace buffer length in bytes.
    pub buffer_len: usize,
}

/// Userspace 3D transfer request.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GpuTransfer3dRequest {
    /// GPU context associated with the transfer.
    pub context_id: u32,
    /// Transfer flags such as `GPU_TRANSFER_FLAG_FENCE`.
    pub flags: u32,
    /// Optional fence identifier.
    pub fence_id: u64,
    /// Resource identifier.
    pub resource_id: u32,
    /// Mip level.
    pub level: u32,
    /// Offset into attached backing memory.
    pub offset: u64,
    /// Row stride in bytes.
    pub stride: u32,
    /// Layer stride in bytes.
    pub layer_stride: u32,
    /// X origin.
    pub x: u32,
    /// Y origin.
    pub y: u32,
    /// Z origin.
    pub z: u32,
    /// Transfer width.
    pub width: u32,
    /// Transfer height.
    pub height: u32,
    /// Transfer depth.
    pub depth: u32,
}

/// Userspace GPU resource present request.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GpuPresentResourceRequest {
    /// Resource identifier.
    pub resource_id: u32,
    /// Resource width in pixels.
    pub resource_width: u32,
    /// Resource height in pixels.
    pub resource_height: u32,
    /// Updated region X origin.
    pub x: u32,
    /// Updated region Y origin.
    pub y: u32,
    /// Updated region width.
    pub width: u32,
    /// Updated region height.
    pub height: u32,
}

fn gpu_feature_bits(capabilities: &GpuCapabilities) -> u32 {
    let mut bits = 0;
    for feature in capabilities.features.iter() {
        bits |= match feature {
            GpuFeature::Virgl => GPU_CAPABILITY_VIRGL,
            GpuFeature::ResourceBlob => GPU_CAPABILITY_RESOURCE_BLOB,
            GpuFeature::Fences => GPU_CAPABILITY_FENCES,
            GpuFeature::ContextInit => GPU_CAPABILITY_CONTEXT_INIT,
        };
    }
    bits
}

fn read_user_value<T: Copy>(ptr: usize) -> Result<T, &'static str> {
    if ptr == 0 {
        return Err("GPU ioctl pointer is null");
    }

    let task = crate::task::mytask().ok_or("No current task for GPU ioctl")?;
    let mut value = core::mem::MaybeUninit::<T>::uninit();
    // SAFETY: `value` is uninitialized storage for exactly one `T`; viewing it
    // as bytes is only used to fill the storage before initialization.
    let bytes = unsafe {
        core::slice::from_raw_parts_mut(value.as_mut_ptr() as *mut u8, core::mem::size_of::<T>())
    };
    copy_from_user(task, ptr, bytes).map_err(|_| "Failed to copy GPU ioctl from user")?;

    // SAFETY: `bytes` covers the whole `T` storage and has just been filled by
    // `copy_from_user`.
    Ok(unsafe { value.assume_init() })
}

fn write_user_value<T: Copy>(ptr: usize, value: &T) -> Result<(), &'static str> {
    if ptr == 0 {
        return Err("GPU ioctl pointer is null");
    }

    let task = crate::task::mytask().ok_or("No current task for GPU ioctl")?;
    // SAFETY: `value` is a valid initialized `T`; exposing its bytes for a copy
    // to userspace does not outlive this function call.
    let bytes = unsafe {
        core::slice::from_raw_parts(value as *const T as *const u8, core::mem::size_of::<T>())
    };
    copy_to_user(task, ptr, bytes).map_err(|_| "Failed to copy GPU ioctl to user")?;
    Ok(())
}

fn read_user_bytes(ptr: usize, len: usize) -> Result<Vec<u8>, &'static str> {
    if len == 0 {
        return Ok(Vec::new());
    }
    if ptr == 0 {
        return Err("GPU user buffer is null");
    }

    let task = crate::task::mytask().ok_or("No current task for GPU user buffer")?;
    let mut bytes = alloc::vec![0u8; len];
    copy_from_user(task, ptr, &mut bytes).map_err(|_| "Failed to copy GPU buffer from user")?;
    Ok(bytes)
}

fn write_user_bytes(ptr: usize, bytes: &[u8]) -> Result<(), &'static str> {
    if bytes.is_empty() {
        return Ok(());
    }
    if ptr == 0 {
        return Err("GPU user buffer is null");
    }

    let task = crate::task::mytask().ok_or("No current task for GPU user buffer")?;
    copy_to_user(task, ptr, bytes).map_err(|_| "Failed to copy GPU buffer to user")
}

fn user_buffer_to_memory_entries(
    buffer_ptr: usize,
    buffer_len: usize,
) -> Result<Vec<GpuMemoryEntry>, &'static str> {
    if buffer_ptr == 0 || buffer_len == 0 {
        return Err("GPU backing buffer is invalid");
    }
    buffer_ptr
        .checked_add(buffer_len)
        .ok_or("GPU backing buffer range overflows")?;

    let task = crate::task::mytask().ok_or("No current task for GPU backing buffer")?;
    let mut entries = Vec::new();
    let mut cursor = buffer_ptr;
    let mut remaining = buffer_len;
    while remaining != 0 {
        let page_offset = cursor & (crate::environment::PAGE_SIZE - 1);
        let chunk_len = (crate::environment::PAGE_SIZE - page_offset).min(remaining);
        let paddr = task
            .vm_manager
            .translate_to_phys_with_access(
                cursor,
                crate::object::capability::memory_mapping::AccessOp::Store,
            )
            .ok_or("Failed to translate GPU backing buffer")?;
        entries.push(GpuMemoryEntry {
            paddr,
            length: chunk_len,
        });
        cursor = cursor
            .checked_add(chunk_len)
            .ok_or("GPU backing buffer range overflows")?;
        remaining -= chunk_len;
    }
    Ok(entries)
}

/// Character device exposing a `GpuDevice` to userspace.
pub struct GpuCharDevice {
    gpu: alloc::sync::Arc<dyn GpuDevice>,
    display: alloc::sync::Arc<dyn GraphicsDevice>,
}

impl GpuCharDevice {
    /// Create a GPU character device wrapper.
    ///
    /// # Arguments
    ///
    /// * `gpu` - GPU backend to expose.
    /// * `display` - Display endpoint used to present GPU resources.
    ///
    /// # Returns
    ///
    /// A new GPU character device.
    pub fn new(
        gpu: alloc::sync::Arc<dyn GpuDevice>,
        display: alloc::sync::Arc<dyn GraphicsDevice>,
    ) -> Self {
        Self { gpu, display }
    }

    fn handle_get_capabilities(&self, arg: usize) -> Result<i32, &'static str> {
        let capabilities = self.gpu.gpu_capabilities();
        let info = GpuCapabilitiesInfo {
            feature_bits: gpu_feature_bits(&capabilities),
            capset_count: capabilities.capset_count,
        };
        write_user_value(arg, &info)?;
        Ok(0)
    }

    fn handle_get_capset_info(&self, arg: usize) -> Result<i32, &'static str> {
        let mut request: GpuCapsetInfoRequest = read_user_value(arg)?;
        let info = self.gpu.get_capset_info(request.index)?;
        request.id = info.id;
        request.max_version = info.max_version;
        request.max_size = info.max_size;
        write_user_value(arg, &request)?;
        Ok(0)
    }

    fn handle_read_capset(&self, arg: usize) -> Result<i32, &'static str> {
        let mut request: GpuReadCapsetRequest = read_user_value(arg)?;
        if request.buffer_ptr == 0 || request.buffer_len == 0 {
            return Err("GPU capset buffer is invalid");
        }

        let mut buffer = alloc::vec![0u8; request.buffer_len];
        let bytes_written = self
            .gpu
            .read_capset(request.id, request.version, &mut buffer)?;
        request.bytes_written = bytes_written.min(buffer.len());
        write_user_bytes(request.buffer_ptr, &buffer[..request.bytes_written])?;
        write_user_value(arg, &request)?;
        Ok(0)
    }

    fn handle_create_context(&self, arg: usize) -> Result<i32, &'static str> {
        let request: GpuContextRequest = read_user_value(arg)?;
        let name_len = request.name_len.min(GPU_CONTEXT_NAME_MAX);
        let mut name_bytes = [0u8; GPU_CONTEXT_NAME_MAX];
        if request.name_ptr != 0 && name_len != 0 {
            let source = read_user_bytes(request.name_ptr, name_len)?;
            name_bytes[..name_len].copy_from_slice(&source);
        }
        let debug_name = core::str::from_utf8(&name_bytes[..name_len])
            .map_err(|_| "GPU context name is not valid UTF-8")?;
        self.gpu.create_context(request.context_id, debug_name)?;
        Ok(0)
    }

    fn handle_destroy_context(&self, arg: usize) -> Result<i32, &'static str> {
        let request: GpuContextRequest = read_user_value(arg)?;
        self.gpu.destroy_context(request.context_id)?;
        Ok(0)
    }

    fn handle_create_3d_resource(&self, arg: usize) -> Result<i32, &'static str> {
        let mut request: GpuCreate3dResourceRequest = read_user_value(arg)?;
        if request.width == 0 || request.height == 0 || request.depth == 0 {
            return Err("GPU 3D resource dimensions are invalid");
        }

        let resource_id = self.gpu.create_3d_resource(GpuResource3dDescription {
            resource_id: request.resource_id,
            target: request.target,
            format: request.format,
            bind: request.bind,
            width: request.width,
            height: request.height,
            depth: request.depth,
            array_size: request.array_size,
            last_level: request.last_level,
            nr_samples: request.nr_samples,
            flags: request.flags,
        })?;
        request.resource_id = resource_id;
        write_user_value(arg, &request)?;
        Ok(0)
    }

    fn handle_unref_resource(&self, arg: usize) -> Result<i32, &'static str> {
        let request: GpuResourceRequest = read_user_value(arg)?;
        self.gpu.unref_resource(request.resource_id)?;
        Ok(0)
    }

    fn handle_attach_resource(&self, arg: usize) -> Result<i32, &'static str> {
        let request: GpuContextResourceRequest = read_user_value(arg)?;
        self.gpu
            .attach_resource(request.context_id, request.resource_id)?;
        Ok(0)
    }

    fn handle_detach_resource(&self, arg: usize) -> Result<i32, &'static str> {
        let request: GpuContextResourceRequest = read_user_value(arg)?;
        self.gpu
            .detach_resource(request.context_id, request.resource_id)?;
        Ok(0)
    }

    fn handle_attach_resource_backing(&self, arg: usize) -> Result<i32, &'static str> {
        let request: GpuAttachResourceBackingRequest = read_user_value(arg)?;
        let entries = user_buffer_to_memory_entries(request.buffer_ptr, request.buffer_len)?;
        self.gpu
            .attach_resource_backing(request.resource_id, &entries)?;
        Ok(0)
    }

    fn handle_detach_resource_backing(&self, arg: usize) -> Result<i32, &'static str> {
        let request: GpuResourceRequest = read_user_value(arg)?;
        self.gpu.detach_resource_backing(request.resource_id)?;
        Ok(0)
    }

    fn gpu_transfer_from_request(request: GpuTransfer3dRequest) -> GpuTransfer3d {
        GpuTransfer3d {
            context_id: request.context_id,
            resource_id: request.resource_id,
            fence_id: if (request.flags & GPU_TRANSFER_FLAG_FENCE) != 0 {
                Some(request.fence_id)
            } else {
                None
            },
            offset: request.offset,
            level: request.level,
            stride: request.stride,
            layer_stride: request.layer_stride,
            x: request.x,
            y: request.y,
            z: request.z,
            width: request.width,
            height: request.height,
            depth: request.depth,
        }
    }

    fn handle_transfer_to_host_3d(&self, arg: usize) -> Result<i32, &'static str> {
        let request: GpuTransfer3dRequest = read_user_value(arg)?;
        self.gpu
            .transfer_to_host_3d(Self::gpu_transfer_from_request(request))?;
        Ok(0)
    }

    fn handle_transfer_from_host_3d(&self, arg: usize) -> Result<i32, &'static str> {
        let request: GpuTransfer3dRequest = read_user_value(arg)?;
        self.gpu
            .transfer_from_host_3d(Self::gpu_transfer_from_request(request))?;
        Ok(0)
    }

    fn handle_submit_commands(&self, arg: usize) -> Result<i32, &'static str> {
        let request: GpuSubmitRequest = read_user_value(arg)?;
        if request.commands_ptr == 0 || request.commands_len == 0 {
            return Err("GPU command buffer is invalid");
        }

        let commands = read_user_bytes(request.commands_ptr, request.commands_len)?;
        let fence_id = if (request.flags & GPU_SUBMIT_FLAG_FENCE) != 0 {
            Some(request.fence_id)
        } else {
            None
        };
        self.gpu.submit_commands(GpuCommandSubmission {
            context_id: request.context_id,
            commands: &commands,
            fence_id,
        })?;
        Ok(0)
    }

    fn handle_present_resource(&self, arg: usize) -> Result<i32, &'static str> {
        let request: GpuPresentResourceRequest = read_user_value(arg)?;
        if request.resource_id == 0 || request.resource_width == 0 || request.resource_height == 0 {
            return Err("GPU present resource is invalid");
        }

        self.display.present_gpu_resource_region(
            GpuDisplayResource::new(
                request.resource_id,
                request.resource_width,
                request.resource_height,
            ),
            DisplayRegion::new(request.x, request.y, request.width, request.height),
        )?;
        Ok(0)
    }
}

impl Device for GpuCharDevice {
    fn device_type(&self) -> super::DeviceType {
        super::DeviceType::Char
    }

    fn name(&self) -> &'static str {
        "gpu"
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
        self
    }

    fn as_char_device(&self) -> Option<&dyn super::char::CharDevice> {
        Some(self)
    }
}

impl super::char::CharDevice for GpuCharDevice {
    fn read_byte(&self) -> Option<u8> {
        None
    }

    fn write_byte(&self, _byte: u8) -> Result<(), &'static str> {
        Err("GPU devices do not support byte writes")
    }

    fn write(&self, _buffer: &[u8]) -> Result<usize, &'static str> {
        Err("GPU devices do not support stream writes")
    }

    fn can_read(&self) -> bool {
        false
    }

    fn can_write(&self) -> bool {
        false
    }
}

impl crate::object::capability::ControlOps for GpuCharDevice {
    fn control(&self, command: u32, arg: usize) -> Result<i32, &'static str> {
        use gpu_commands::*;

        match command {
            GPU_GET_CAPABILITIES => self.handle_get_capabilities(arg),
            GPU_GET_CAPSET_INFO => self.handle_get_capset_info(arg),
            GPU_READ_CAPSET => self.handle_read_capset(arg),
            GPU_CREATE_CONTEXT => self.handle_create_context(arg),
            GPU_DESTROY_CONTEXT => self.handle_destroy_context(arg),
            GPU_SUBMIT_COMMANDS => self.handle_submit_commands(arg),
            GPU_CREATE_3D_RESOURCE => self.handle_create_3d_resource(arg),
            GPU_UNREF_RESOURCE => self.handle_unref_resource(arg),
            GPU_ATTACH_RESOURCE => self.handle_attach_resource(arg),
            GPU_DETACH_RESOURCE => self.handle_detach_resource(arg),
            GPU_ATTACH_RESOURCE_BACKING => self.handle_attach_resource_backing(arg),
            GPU_DETACH_RESOURCE_BACKING => self.handle_detach_resource_backing(arg),
            GPU_TRANSFER_TO_HOST_3D => self.handle_transfer_to_host_3d(arg),
            GPU_TRANSFER_FROM_HOST_3D => self.handle_transfer_from_host_3d(arg),
            GPU_PRESENT_RESOURCE => self.handle_present_resource(arg),
            _ => Err("Unsupported GPU control command"),
        }
    }
}

impl crate::object::capability::MemoryMappingOps for GpuCharDevice {
    fn get_mapping_info(
        &self,
        _offset: usize,
        _length: usize,
    ) -> Result<crate::object::capability::MemoryMappingInfo, &'static str> {
        Err("GPU device memory mapping is not implemented")
    }

    fn on_mapped(&self, _vaddr: usize, _paddr: usize, _length: usize, _offset: usize) {}

    fn on_unmapped(&self, _vaddr: usize, _length: usize) {}

    fn supports_mmap(&self) -> bool {
        false
    }
}

impl crate::object::capability::selectable::Selectable for GpuCharDevice {
    fn wait_until_ready(
        &self,
        _interest: crate::object::capability::selectable::ReadyInterest,
        _trapframe: &mut crate::arch::Trapframe,
        _timeout_ticks: Option<u64>,
        _min_wait_ticks: u64,
    ) -> crate::object::capability::selectable::SelectWaitOutcome {
        crate::object::capability::selectable::SelectWaitOutcome::Ready
    }
}
