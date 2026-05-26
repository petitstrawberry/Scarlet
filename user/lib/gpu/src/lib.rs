//! GPU control library for Scarlet OS.
//!
//! This library exposes Scarlet's neutral GPU control ABI. It is intentionally
//! thin: virgl command encoding remains the caller's responsibility, while this
//! wrapper handles device opening, capset discovery, context lifetime, and
//! command submission.

#![no_std]

extern crate alloc;
extern crate scarlet_std as std;

use alloc::{string::String, vec, vec::Vec};
use std::{
    fs::File,
    handle::{HandleError, HandleResult},
};

/// GPU control command constants.
pub mod commands {
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

/// GPU capability response.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GpuCapabilitiesInfo {
    /// Bitset of `GPU_CAPABILITY_*` values.
    pub feature_bits: u32,
    /// Number of backend capsets available.
    pub capset_count: u32,
}

impl GpuCapabilitiesInfo {
    /// Check whether virgl is available.
    ///
    /// # Returns
    ///
    /// `true` if virgl command submission is supported.
    pub fn supports_virgl(&self) -> bool {
        (self.feature_bits & GPU_CAPABILITY_VIRGL) != 0
    }
}

/// Capset metadata request/response.
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

/// Capset read request.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GpuReadCapsetRequest {
    /// Backend-defined capset identifier.
    pub id: u32,
    /// Capset version to read.
    pub version: u32,
    /// Destination buffer pointer.
    pub buffer_ptr: usize,
    /// Destination buffer length in bytes.
    pub buffer_len: usize,
    /// Number of bytes copied by the kernel.
    pub bytes_written: usize,
}

/// GPU context request.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GpuContextRequest {
    /// Caller-selected GPU context identifier.
    pub context_id: u32,
    /// Optional debug-name pointer.
    pub name_ptr: usize,
    /// Debug-name length in bytes.
    pub name_len: usize,
}

/// GPU command submission request.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GpuSubmitRequest {
    /// GPU context that owns the command stream.
    pub context_id: u32,
    /// Submission flags such as `GPU_SUBMIT_FLAG_FENCE`.
    pub flags: u32,
    /// Optional fence identifier.
    pub fence_id: u64,
    /// Command buffer pointer.
    pub commands_ptr: usize,
    /// Command buffer length in bytes.
    pub commands_len: usize,
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

/// GPU 3D resource creation request.
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

/// GPU resource-only request.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GpuResourceRequest {
    /// Resource identifier.
    pub resource_id: u32,
}

/// GPU context/resource association request.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GpuContextResourceRequest {
    /// Context identifier.
    pub context_id: u32,
    /// Resource identifier.
    pub resource_id: u32,
}

/// GPU resource backing attachment request.
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

/// GPU 3D transfer request.
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

/// GPU resource present request.
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

/// GPU control device wrapper.
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
    /// GPU wrapper or a handle error.
    pub fn open(path: &str) -> HandleResult<Self> {
        let file = File::open(path).map_err(|_| HandleError::NotFound)?;
        Ok(Self { file })
    }

    /// Query GPU backend capabilities.
    ///
    /// # Returns
    ///
    /// Backend capability information.
    pub fn capabilities(&self) -> HandleResult<GpuCapabilitiesInfo> {
        let mut info = GpuCapabilitiesInfo::default();
        self.file
            .as_handle()
            .control(commands::GPU_GET_CAPABILITIES, &mut info as *mut _ as usize)?;
        Ok(info)
    }

    /// Query one capset's metadata.
    ///
    /// # Arguments
    ///
    /// * `index` - Zero-based capset index.
    ///
    /// # Returns
    ///
    /// Capset metadata.
    pub fn capset_info(&self, index: u32) -> HandleResult<GpuCapsetInfoRequest> {
        let mut request = GpuCapsetInfoRequest {
            index,
            ..GpuCapsetInfoRequest::default()
        };
        self.file.as_handle().control(
            commands::GPU_GET_CAPSET_INFO,
            &mut request as *mut _ as usize,
        )?;
        Ok(request)
    }

    /// Read capset bytes.
    ///
    /// # Arguments
    ///
    /// * `id` - Backend-defined capset identifier.
    /// * `version` - Capset version.
    /// * `max_size` - Maximum number of bytes to read.
    ///
    /// # Returns
    ///
    /// Capset byte vector.
    pub fn read_capset(&self, id: u32, version: u32, max_size: usize) -> HandleResult<Vec<u8>> {
        let mut buffer = vec![0u8; max_size];
        let mut request = GpuReadCapsetRequest {
            id,
            version,
            buffer_ptr: buffer.as_mut_ptr() as usize,
            buffer_len: buffer.len(),
            bytes_written: 0,
        };
        self.file
            .as_handle()
            .control(commands::GPU_READ_CAPSET, &mut request as *mut _ as usize)?;
        buffer.truncate(request.bytes_written);
        Ok(buffer)
    }

    /// Create a GPU execution context.
    ///
    /// # Arguments
    ///
    /// * `context_id` - Caller-selected context identifier.
    /// * `debug_name` - Human-readable debug name.
    ///
    /// # Returns
    ///
    /// Success or a handle error.
    pub fn create_context(&self, context_id: u32, debug_name: &str) -> HandleResult<()> {
        let request = GpuContextRequest {
            context_id,
            name_ptr: debug_name.as_ptr() as usize,
            name_len: debug_name.len(),
        };
        self.file
            .as_handle()
            .control(commands::GPU_CREATE_CONTEXT, &request as *const _ as usize)?;
        Ok(())
    }

    /// Destroy a GPU execution context.
    ///
    /// # Arguments
    ///
    /// * `context_id` - Context identifier to destroy.
    ///
    /// # Returns
    ///
    /// Success or a handle error.
    pub fn destroy_context(&self, context_id: u32) -> HandleResult<()> {
        let request = GpuContextRequest {
            context_id,
            ..GpuContextRequest::default()
        };
        self.file
            .as_handle()
            .control(commands::GPU_DESTROY_CONTEXT, &request as *const _ as usize)?;
        Ok(())
    }

    /// Create a backend 3D resource.
    ///
    /// # Arguments
    ///
    /// * `description` - Resource target, format, dimensions, and creation flags.
    ///
    /// # Returns
    ///
    /// Resource identifier allocated by the backend or a handle error.
    pub fn create_3d_resource(&self, description: GpuResource3dDescription) -> HandleResult<u32> {
        let mut request = GpuCreate3dResourceRequest {
            resource_id: description.resource_id,
            target: description.target,
            format: description.format,
            bind: description.bind,
            width: description.width,
            height: description.height,
            depth: description.depth,
            array_size: description.array_size,
            last_level: description.last_level,
            nr_samples: description.nr_samples,
            flags: description.flags,
        };
        self.file.as_handle().control(
            commands::GPU_CREATE_3D_RESOURCE,
            &mut request as *mut _ as usize,
        )?;
        Ok(request.resource_id)
    }

    /// Unreference a backend resource.
    ///
    /// # Arguments
    ///
    /// * `resource_id` - Resource identifier to release.
    ///
    /// # Returns
    ///
    /// Success or a handle error.
    pub fn unref_resource(&self, resource_id: u32) -> HandleResult<()> {
        let request = GpuResourceRequest { resource_id };
        self.file
            .as_handle()
            .control(commands::GPU_UNREF_RESOURCE, &request as *const _ as usize)?;
        Ok(())
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
    /// Success or a handle error.
    pub fn attach_resource(&self, context_id: u32, resource_id: u32) -> HandleResult<()> {
        let request = GpuContextResourceRequest {
            context_id,
            resource_id,
        };
        self.file
            .as_handle()
            .control(commands::GPU_ATTACH_RESOURCE, &request as *const _ as usize)?;
        Ok(())
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
    /// Success or a handle error.
    pub fn detach_resource(&self, context_id: u32, resource_id: u32) -> HandleResult<()> {
        let request = GpuContextResourceRequest {
            context_id,
            resource_id,
        };
        self.file
            .as_handle()
            .control(commands::GPU_DETACH_RESOURCE, &request as *const _ as usize)?;
        Ok(())
    }

    /// Attach userspace memory backing to a resource.
    ///
    /// # Arguments
    ///
    /// * `resource_id` - Resource that should use the backing memory.
    /// * `buffer` - Backing memory buffer.
    ///
    /// # Returns
    ///
    /// Success or a handle error.
    pub fn attach_resource_backing(&self, resource_id: u32, buffer: &mut [u8]) -> HandleResult<()> {
        let request = GpuAttachResourceBackingRequest {
            resource_id,
            buffer_ptr: buffer.as_mut_ptr() as usize,
            buffer_len: buffer.len(),
        };
        self.file.as_handle().control(
            commands::GPU_ATTACH_RESOURCE_BACKING,
            &request as *const _ as usize,
        )?;
        Ok(())
    }

    /// Detach memory backing from a resource.
    ///
    /// # Arguments
    ///
    /// * `resource_id` - Resource whose backing should be detached.
    ///
    /// # Returns
    ///
    /// Success or a handle error.
    pub fn detach_resource_backing(&self, resource_id: u32) -> HandleResult<()> {
        let request = GpuResourceRequest { resource_id };
        self.file.as_handle().control(
            commands::GPU_DETACH_RESOURCE_BACKING,
            &request as *const _ as usize,
        )?;
        Ok(())
    }

    fn transfer_request(transfer: GpuTransfer3d) -> GpuTransfer3dRequest {
        GpuTransfer3dRequest {
            context_id: transfer.context_id,
            flags: if transfer.fence_id.is_some() {
                GPU_TRANSFER_FLAG_FENCE
            } else {
                0
            },
            fence_id: transfer.fence_id.unwrap_or(0),
            resource_id: transfer.resource_id,
            level: transfer.level,
            offset: transfer.offset,
            stride: transfer.stride,
            layer_stride: transfer.layer_stride,
            x: transfer.x,
            y: transfer.y,
            z: transfer.z,
            width: transfer.width,
            height: transfer.height,
            depth: transfer.depth,
        }
    }

    /// Transfer attached backing memory to a host 3D resource.
    ///
    /// # Arguments
    ///
    /// * `transfer` - Transfer region, strides, and optional fence.
    ///
    /// # Returns
    ///
    /// Success or a handle error.
    pub fn transfer_to_host_3d(&self, transfer: GpuTransfer3d) -> HandleResult<()> {
        let request = Self::transfer_request(transfer);
        self.file.as_handle().control(
            commands::GPU_TRANSFER_TO_HOST_3D,
            &request as *const _ as usize,
        )?;
        Ok(())
    }

    /// Transfer a host 3D resource into attached backing memory.
    ///
    /// # Arguments
    ///
    /// * `transfer` - Transfer region, strides, and optional fence.
    ///
    /// # Returns
    ///
    /// Success or a handle error.
    pub fn transfer_from_host_3d(&self, transfer: GpuTransfer3d) -> HandleResult<()> {
        let request = Self::transfer_request(transfer);
        self.file.as_handle().control(
            commands::GPU_TRANSFER_FROM_HOST_3D,
            &request as *const _ as usize,
        )?;
        Ok(())
    }

    /// Present a GPU resource region through the display pipeline.
    ///
    /// # Arguments
    ///
    /// * `resource_id` - Resource identifier.
    /// * `resource_width` - Resource width in pixels.
    /// * `resource_height` - Resource height in pixels.
    /// * `x` - Updated region X origin.
    /// * `y` - Updated region Y origin.
    /// * `width` - Updated region width.
    /// * `height` - Updated region height.
    ///
    /// # Returns
    ///
    /// Success or a handle error.
    pub fn present_resource_region(
        &self,
        resource_id: u32,
        resource_width: u32,
        resource_height: u32,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    ) -> HandleResult<()> {
        let request = GpuPresentResourceRequest {
            resource_id,
            resource_width,
            resource_height,
            x,
            y,
            width,
            height,
        };
        self.file.as_handle().control(
            commands::GPU_PRESENT_RESOURCE,
            &request as *const _ as usize,
        )?;
        Ok(())
    }

    /// Present a whole GPU resource through the display pipeline.
    ///
    /// # Arguments
    ///
    /// * `resource_id` - Resource identifier.
    /// * `width` - Resource width in pixels.
    /// * `height` - Resource height in pixels.
    ///
    /// # Returns
    ///
    /// Success or a handle error.
    pub fn present_resource(&self, resource_id: u32, width: u32, height: u32) -> HandleResult<()> {
        self.present_resource_region(resource_id, width, height, 0, 0, width, height)
    }

    /// Submit a virgl command stream.
    ///
    /// # Arguments
    ///
    /// * `context_id` - GPU context that owns the command stream.
    /// * `commands` - Backend-specific command bytes.
    /// * `fence_id` - Optional fence identifier.
    ///
    /// # Returns
    ///
    /// Success or a handle error.
    pub fn submit_commands(
        &self,
        context_id: u32,
        commands: &[u8],
        fence_id: Option<u64>,
    ) -> HandleResult<()> {
        let request = GpuSubmitRequest {
            context_id,
            flags: if fence_id.is_some() {
                GPU_SUBMIT_FLAG_FENCE
            } else {
                0
            },
            fence_id: fence_id.unwrap_or(0),
            commands_ptr: commands.as_ptr() as usize,
            commands_len: commands.len(),
        };
        self.file
            .as_handle()
            .control(commands::GPU_SUBMIT_COMMANDS, &request as *const _ as usize)?;
        Ok(())
    }

    /// Get the underlying file path-independent debug label.
    ///
    /// # Returns
    ///
    /// Static wrapper label.
    pub fn label(&self) -> String {
        String::from("gpu")
    }
}
