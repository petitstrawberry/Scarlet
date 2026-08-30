//! # VirtIO GPU Device Driver
//!
//! This module provides a driver for VirtIO GPU devices, implementing the
//! GraphicsDevice trait for integration with the kernel's graphics subsystem.
//!
//! The driver supports basic framebuffer operations and display management
//! according to the VirtIO GPU specification.

use crate::sync::{IrqRwSpinLock, IrqSpinLock};
use alloc::{sync::Arc, vec::Vec};

use crate::{
    device::{
        Device, DeviceType,
        gpu::{
            GPU_DIALECT_INFO_BYTES, GPU_EXECUTION_SUPPORT_DEPTH,
            GPU_EXECUTION_SUPPORT_IMAGE_READBACK, GPU_EXECUTION_SUPPORT_IMAGE_UPLOAD,
            GPU_EXECUTION_SUPPORT_MEMORY, GPU_EXECUTION_SUPPORT_PRESENTATION,
            GPU_EXECUTION_SUPPORT_QUEUE, GPU_EXECUTION_SUPPORT_TIMELINE,
            GPU_IMAGE_FORMAT_BGRA8_UNORM, GPU_IMAGE_FORMAT_DEPTH32_FLOAT,
            GPU_IMAGE_USAGE_DEPTH_STENCIL_ATTACHMENT, GPU_IMAGE_USAGE_PRESENTABLE,
            GPU_IMAGE_USAGE_RENDER_TARGET, GPU_IMAGE_USAGE_SAMPLED, GPU_IMAGE_USAGE_TRANSFER_DST,
            GPU_IMAGE_USAGE_TRANSFER_SRC, GpuBackend, GpuBackendBuffer, GpuBackendBufferInfo,
            GpuBackendContext, GpuBackendContextInfo, GpuBackendDialectDescriptor,
            GpuBackendDialectInfo, GpuBackendImage, GpuBackendImageInfo, GpuBackendInfo,
            GpuBackendQueue, GpuBackendQueueInfo, GpuBackendSubmitError, GpuBufferCreateInfo,
            GpuDeviceInfo, GpuDeviceState, GpuImageBackingInfo, GpuImageCreateInfo,
            GpuImageUploadInfo,
        },
        graphics::{
            FramebufferConfig, GpuDisplayResource, GraphicsDevice, PixelFormat,
            output::DisplayRegion,
        },
    },
    drivers::virtio::{
        device::VirtioDevice,
        pci::VirtioPciTransport,
        queue::{DescriptorFlag, VirtQueue},
    },
    mem::page::ContiguousPages,
    object::capability::{ControlOps, MemoryMappingOps, Selectable},
    vm::addr::virt_to_phys,
};
use core::ptr;

// VirtIO GPU Constants
const VIRTIO_GPU_F_VIRGL: u32 = 0;
const VIRTIO_GPU_F_EDID: u32 = 1;
const VIRTIO_GPU_F_CONTEXT_INIT: u32 = 4;

// VirtIO GPU Control Commands
const VIRTIO_GPU_CMD_GET_DISPLAY_INFO: u32 = 0x0100;
const VIRTIO_GPU_CMD_RESOURCE_CREATE_2D: u32 = 0x0101;
const VIRTIO_GPU_CMD_RESOURCE_UNREF: u32 = 0x0102;
const VIRTIO_GPU_CMD_SET_SCANOUT: u32 = 0x0103;
const VIRTIO_GPU_CMD_RESOURCE_FLUSH: u32 = 0x0104;
const VIRTIO_GPU_CMD_TRANSFER_TO_HOST_2D: u32 = 0x0105;
const VIRTIO_GPU_CMD_RESOURCE_ATTACH_BACKING: u32 = 0x0106;
const VIRTIO_GPU_CMD_RESOURCE_DETACH_BACKING: u32 = 0x0107;
const VIRTIO_GPU_CMD_GET_CAPSET_INFO: u32 = 0x0108;
const VIRTIO_GPU_CMD_GET_CAPSET: u32 = 0x0109;
const VIRTIO_GPU_CMD_CTX_CREATE: u32 = 0x0200;
const VIRTIO_GPU_CMD_CTX_DESTROY: u32 = 0x0201;
const VIRTIO_GPU_CMD_CTX_ATTACH_RESOURCE: u32 = 0x0202;
const VIRTIO_GPU_CMD_CTX_DETACH_RESOURCE: u32 = 0x0203;
const VIRTIO_GPU_CMD_RESOURCE_CREATE_3D: u32 = 0x0204;
const VIRTIO_GPU_CMD_TRANSFER_TO_HOST_3D: u32 = 0x0205;
const VIRTIO_GPU_CMD_TRANSFER_FROM_HOST_3D: u32 = 0x0206;
const VIRTIO_GPU_CMD_SUBMIT_3D: u32 = 0x0207;

// VirtIO GPU Response Types
const VIRTIO_GPU_RESP_OK_NODATA: u32 = 0x1100;
const VIRTIO_GPU_RESP_OK_DISPLAY_INFO: u32 = 0x1101;
const VIRTIO_GPU_RESP_OK_CAPSET_INFO: u32 = 0x1102;
const VIRTIO_GPU_RESP_OK_CAPSET: u32 = 0x1103;
// VirtIO GPU error responses occupy this protocol-defined range. They report
// a rejected control command, not a transport failure.
const VIRTIO_GPU_RESP_ERR_UNSPEC: u32 = 0x1200;
const VIRTIO_GPU_RESP_ERR_INVALID_PARAMETER: u32 = 0x1205;

// VirtIO GPU command flags
const VIRTIO_GPU_FLAG_FENCE: u32 = 1;
const VIRTIO_GPU_MAX_CONTEXT_NAME: usize = 64;
const VIRTIO_GPU_CONFIG_NUM_CAPSETS_OFFSET: usize = 12;
const VIRTIO_GPU_CONTROL_QUEUE_SIZE: usize = 64;
const VIRTIO_GPU_CURSOR_QUEUE_SIZE: usize = 16;
const VIRTIO_GPU_CONTROL_TIMEOUT_NS: u64 = 2_000_000_000;
const VIRTIO_GPU_CONTROL_MAX_SPINS: u64 = 10_000_000;
// Preserve the established VirGL/QEMU transport budget independently of the
// generic GPU ABI's larger absolute bound.
const VIRTIO_GPU_MAX_OPAQUE_COMMAND_SIZE: u32 = 64 * 1024;

// VirtIO GPU Formats
const VIRTIO_GPU_FORMAT_B8G8R8A8_UNORM: u32 = 1;
const VIRGL_FORMAT_Z32_FLOAT: u32 = 18;
const VIRTIO_GPU_FORMAT_B8G8R8X8_UNORM: u32 = 2;
const VIRTIO_GPU_FORMAT_A8R8G8B8_UNORM: u32 = 3;
const VIRTIO_GPU_FORMAT_X8R8G8B8_UNORM: u32 = 4;
const VIRTIO_GPU_FORMAT_R8G8B8A8_UNORM: u32 = 67;
const VIRTIO_GPU_FORMAT_X8B8G8R8_UNORM: u32 = 68;
const VIRTIO_GPU_FORMAT_A8B8G8R8_UNORM: u32 = 121;
const VIRTIO_GPU_FORMAT_R8G8B8X8_UNORM: u32 = 134;

// VirGL Gallium texture target and bind flags.
const PIPE_BUFFER: u32 = 0;
const PIPE_TEXTURE_2D: u32 = 2;
const PIPE_BIND_RENDER_TARGET: u32 = 1 << 1;
const PIPE_BIND_DEPTH_STENCIL: u32 = 1 << 0;
const PIPE_BIND_SAMPLER_VIEW: u32 = 1 << 3;
const PIPE_BIND_VERTEX_BUFFER: u32 = 1 << 4;
const PIPE_BIND_SCANOUT: u32 = 1 << 19;

// Maximum number of scanouts
const VIRTIO_GPU_MAX_SCANOUTS: usize = 16;

/// VirtIO GPU command header
#[repr(C)]
#[derive(Clone, Copy)]
struct VirtioGpuCtrlHdr {
    hdr_type: u32,
    flags: u32,
    fence_id: u64,
    ctx_id: u32,
    padding: u32,
}

/// VirtIO GPU rectangle
#[repr(C)]
#[derive(Clone, Copy)]
struct VirtioGpuRect {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

/// VirtIO GPU display info
#[repr(C)]
#[derive(Clone, Copy)]
struct VirtioGpuRespDisplayInfo {
    hdr: VirtioGpuCtrlHdr,
    pmodes: [VirtioGpuDisplayOne; VIRTIO_GPU_MAX_SCANOUTS],
}

/// VirtIO GPU display mode
#[repr(C)]
#[derive(Clone, Copy)]
struct VirtioGpuDisplayOne {
    r: VirtioGpuRect,
    enabled: u32,
    flags: u32,
}

/// VirtIO GPU resource create 2D
#[repr(C)]
struct VirtioGpuResourceCreate2d {
    hdr: VirtioGpuCtrlHdr,
    resource_id: u32,
    format: u32,
    width: u32,
    height: u32,
}

/// VirtIO GPU resource unref.
#[repr(C)]
struct VirtioGpuResourceUnref {
    hdr: VirtioGpuCtrlHdr,
    resource_id: u32,
    padding: u32,
}

/// VirtIO GPU resource create 3D.
#[repr(C)]
struct VirtioGpuResourceCreate3d {
    hdr: VirtioGpuCtrlHdr,
    resource_id: u32,
    target: u32,
    format: u32,
    bind: u32,
    width: u32,
    height: u32,
    depth: u32,
    array_size: u32,
    last_level: u32,
    nr_samples: u32,
    flags: u32,
    padding: u32,
}

/// VirtIO GPU context/resource association command.
#[repr(C)]
struct VirtioGpuCtxResource {
    hdr: VirtioGpuCtrlHdr,
    resource_id: u32,
    padding: u32,
}

/// VirtIO GPU 3D transfer box.
#[repr(C)]
struct VirtioGpuBox {
    x: u32,
    y: u32,
    z: u32,
    width: u32,
    height: u32,
    depth: u32,
}

/// VirtIO GPU 3D host transfer command.
#[repr(C)]
struct VirtioGpuTransferHost3d {
    hdr: VirtioGpuCtrlHdr,
    box_: VirtioGpuBox,
    offset: u64,
    resource_id: u32,
    level: u32,
    stride: u32,
    layer_stride: u32,
}

/// VirtIO GPU set scanout
#[repr(C)]
struct VirtioGpuSetScanout {
    hdr: VirtioGpuCtrlHdr,
    r: VirtioGpuRect,
    scanout_id: u32,
    resource_id: u32,
}

/// VirtIO GPU resource flush
#[repr(C)]
struct VirtioGpuResourceFlush {
    hdr: VirtioGpuCtrlHdr,
    r: VirtioGpuRect,
    resource_id: u32,
    padding: u32,
}

/// VirtIO GPU transfer to host 2D
#[repr(C)]
struct VirtioGpuTransferToHost2d {
    hdr: VirtioGpuCtrlHdr,
    r: VirtioGpuRect,
    offset: u64,
    resource_id: u32,
    padding: u32,
}

/// VirtIO GPU resource attach backing
#[repr(C)]
struct VirtioGpuResourceAttachBacking {
    hdr: VirtioGpuCtrlHdr,
    resource_id: u32,
    nr_entries: u32,
}

/// VirtIO GPU resource detach backing.
#[repr(C)]
struct VirtioGpuResourceDetachBacking {
    hdr: VirtioGpuCtrlHdr,
    resource_id: u32,
    padding: u32,
}

/// VirtIO GPU memory entry
#[repr(C)]
struct VirtioGpuMemEntry {
    addr: u64,
    length: u32,
    padding: u32,
}

/// Internal acceleration resource descriptor retained for later backend phases.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct VirtioGpuAccelerationResource3d {
    resource_id: u32,
    target: u32,
    format: u32,
    bind: u32,
    width: u32,
    height: u32,
    depth: u32,
    array_size: u32,
    last_level: u32,
    nr_samples: u32,
    flags: u32,
}

/// Internal guest-memory entry retained for later backend phases.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct VirtioGpuAccelerationMemoryEntry {
    paddr: usize,
    length: usize,
}

/// Internal acceleration transfer descriptor retained for later backend phases.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct VirtioGpuAccelerationTransfer3d {
    context_id: u32,
    resource_id: u32,
    fence_id: Option<u64>,
    offset: u64,
    level: u32,
    stride: u32,
    layer_stride: u32,
    x: u32,
    y: u32,
    z: u32,
    width: u32,
    height: u32,
    depth: u32,
}

fn virgl_image_bind(usage: u32) -> u32 {
    let mut bind = 0;
    if usage & GPU_IMAGE_USAGE_RENDER_TARGET != 0 {
        bind |= PIPE_BIND_RENDER_TARGET;
    }
    if usage & GPU_IMAGE_USAGE_DEPTH_STENCIL_ATTACHMENT != 0 {
        bind |= PIPE_BIND_DEPTH_STENCIL;
    }
    if usage & GPU_IMAGE_USAGE_SAMPLED != 0 {
        bind |= PIPE_BIND_SAMPLER_VIEW;
    }
    if usage & GPU_IMAGE_USAGE_PRESENTABLE != 0 {
        bind |= PIPE_BIND_SCANOUT;
    }
    bind
}

/// Internal VirtIO backend dialect metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct VirtioGpuBackendDialectInfo {
    id: u32,
    max_version: u32,
    max_size: u32,
}

/// VirtIO GPU capset info request.
#[repr(C)]
struct VirtioGpuGetCapsetInfo {
    hdr: VirtioGpuCtrlHdr,
    capset_index: u32,
    padding: u32,
}

/// VirtIO GPU capset info response.
#[repr(C)]
#[derive(Clone, Copy)]
struct VirtioGpuRespCapsetInfo {
    hdr: VirtioGpuCtrlHdr,
    capset_id: u32,
    capset_max_version: u32,
    capset_max_size: u32,
    padding: u32,
}

/// VirtIO GPU capset read request.
#[repr(C)]
struct VirtioGpuGetCapset {
    hdr: VirtioGpuCtrlHdr,
    capset_id: u32,
    capset_version: u32,
}

/// VirtIO GPU context create request.
#[repr(C)]
struct VirtioGpuCtxCreate {
    hdr: VirtioGpuCtrlHdr,
    nlen: u32,
    context_init: u32,
    debug_name: [u8; VIRTIO_GPU_MAX_CONTEXT_NAME],
}

/// VirtIO GPU context destroy request.
#[repr(C)]
struct VirtioGpuCtxDestroy {
    hdr: VirtioGpuCtrlHdr,
}

/// VirtIO GPU 3D command submission header.
#[repr(C)]
struct VirtioGpuCmdSubmit3d {
    hdr: VirtioGpuCtrlHdr,
    size: u32,
    padding: u32,
}

/// Append a plain old data structure to a byte vector.
fn append_pod_bytes<T>(buffer: &mut Vec<u8>, value: &T) {
    // SAFETY: VirtIO command structures in this module are #[repr(C)] and are
    // sent to the device as their raw byte representation. The borrowed value
    // lives for the duration of the copy into `buffer`.
    let bytes = unsafe {
        core::slice::from_raw_parts(value as *const T as *const u8, core::mem::size_of::<T>())
    };
    buffer.extend_from_slice(bytes);
}

fn validate_execution_response(
    response: VirtioGpuCtrlHdr,
    expected_fence_id: Option<u64>,
) -> Result<(), &'static str> {
    if response.hdr_type != VIRTIO_GPU_RESP_OK_NODATA {
        return Err("VirtIO GPU execution command returned unexpected response");
    }
    if let Some(fence_id) = expected_fence_id
        && (response.flags & VIRTIO_GPU_FLAG_FENCE == 0 || response.fence_id != fence_id)
    {
        return Err("VirtIO GPU execution command returned an unexpected fence response");
    }
    Ok(())
}

/// Validate a response to an opaque VirGL command submission.
///
/// A VirtIO GPU error response is a rejection of this command stream. It must
/// remain local to the submit so callers can reuse their queue and timeline.
/// A malformed success response is not evidence of device loss either, so it
/// is surfaced as temporarily unavailable rather than poisoning a timeline.
fn validate_submit_response(
    response: VirtioGpuCtrlHdr,
    expected_fence_id: Option<u64>,
) -> Result<(), GpuBackendSubmitError> {
    if (VIRTIO_GPU_RESP_ERR_UNSPEC..=VIRTIO_GPU_RESP_ERR_INVALID_PARAMETER)
        .contains(&response.hdr_type)
    {
        return Err(GpuBackendSubmitError::Rejected(
            "VirtIO GPU rejected the execution command stream",
        ));
    }
    validate_execution_response(response, expected_fence_id)
        .map_err(GpuBackendSubmitError::Unavailable)
}

/// Classify a synchronous control-queue transport failure during submission.
///
/// Only a timeout (or the permanent state left by one) proves that the device
/// can no longer safely serve the queue. Allocation and descriptor pressure
/// are retryable per-submit failures.
fn classify_submit_transport_error(error: &'static str) -> GpuBackendSubmitError {
    match error {
        "VirtIO GPU control queue timed out"
        | "VirtIO GPU control queue is unavailable after a timeout" => {
            GpuBackendSubmitError::DeviceLost(error)
        }
        _ => GpuBackendSubmitError::Unavailable(error),
    }
}

/// VirtIO GPU Device Core
pub struct VirtioGpuDeviceCore {
    base_addr: usize,
    pci_transport: Option<VirtioPciTransport>,
    virtqueues: IrqSpinLock<[VirtQueue<'static>; 2]>, // Control queue (0) and Cursor queue (1)
    display_info: IrqRwSpinLock<Option<VirtioGpuRespDisplayInfo>>,
    framebuffer_addr: IrqRwSpinLock<Option<usize>>,
    framebuffer_alloc: IrqRwSpinLock<Option<ContiguousPages>>,
    retired_framebuffer_allocs: IrqSpinLock<Vec<ContiguousPages>>,
    resource_id: IrqSpinLock<u32>,
    acceleration_context_id: IrqSpinLock<u32>,
    acceleration_fence_id: IrqSpinLock<u64>,
    current_resource_id: IrqRwSpinLock<Option<u32>>,
    scanout_resource_id: IrqRwSpinLock<Option<u32>>,
    negotiated_features: IrqRwSpinLock<u64>,
    transport_ready: IrqRwSpinLock<bool>,
    control_queue_failed: IrqRwSpinLock<bool>,
    initialized: IrqSpinLock<bool>,
    // Track resources and their associated memory
    resources: IrqSpinLock<alloc::collections::BTreeMap<u32, (usize, usize)>>, // resource_id -> (addr, size)
}

impl VirtioGpuDeviceCore {
    /// Create a new VirtIO GPU device
    ///
    /// # Arguments
    ///
    /// * `base_addr` - The base address of the device
    ///
    /// # Returns
    ///
    /// A new instance of `VirtioGpuDevice`
    pub fn new(base_addr: usize) -> Self {
        Self::new_with_transport(base_addr, None)
    }

    /// Create a new VirtIO GPU device core backed by the PCI transport.
    ///
    /// # Arguments
    ///
    /// * `transport` - Mapped VirtIO PCI configuration regions
    ///
    /// # Returns
    ///
    /// A new instance of `VirtioGpuDeviceCore`.
    pub fn new_pci(transport: VirtioPciTransport) -> Self {
        Self::new_with_transport(transport.common_cfg, Some(transport))
    }

    fn new_with_transport(base_addr: usize, pci_transport: Option<VirtioPciTransport>) -> Self {
        let mut device = Self {
            base_addr,
            pci_transport,
            virtqueues: IrqSpinLock::new([
                VirtQueue::new(VIRTIO_GPU_CONTROL_QUEUE_SIZE),
                VirtQueue::new(VIRTIO_GPU_CURSOR_QUEUE_SIZE),
            ]),
            display_info: IrqRwSpinLock::new(None),
            framebuffer_addr: IrqRwSpinLock::new(None),
            framebuffer_alloc: IrqRwSpinLock::new(None),
            retired_framebuffer_allocs: IrqSpinLock::new(Vec::new()),
            resource_id: IrqSpinLock::new(1),
            acceleration_context_id: IrqSpinLock::new(1),
            acceleration_fence_id: IrqSpinLock::new(1),
            current_resource_id: IrqRwSpinLock::new(None),
            scanout_resource_id: IrqRwSpinLock::new(None),
            negotiated_features: IrqRwSpinLock::new(0),
            transport_ready: IrqRwSpinLock::new(false),
            control_queue_failed: IrqRwSpinLock::new(false),
            initialized: IrqSpinLock::new(false),
            resources: IrqSpinLock::new(alloc::collections::BTreeMap::new()),
        };

        // Initialize virtqueues first
        {
            let mut virtqueues = device.virtqueues.lock();
            for queue in virtqueues.iter_mut() {
                queue.init();
            }
        }

        // Initialize the VirtIO device - this will set up the queues with the device
        match device.init() {
            Ok(features) => {
                *device.negotiated_features.write() = features;
                *device.transport_ready.write() = true;
            }
            Err(_) => {
                crate::early_println!("[Virtio GPU] Warning: Failed to initialize VirtIO device");
            }
        }

        // crate::early_println!("[Virtio GPU] Device created and initialized at {:#x}", base_addr);
        device
    }

    /// Get next resource ID
    fn next_resource_id(&self) -> u32 {
        let mut id = self.resource_id.lock();
        let current = *id;
        *id += 1;
        current
    }

    fn backend_cookie(&self) -> u64 {
        self as *const Self as usize as u64
    }

    fn next_acceleration_context_id(&self) -> Result<u32, &'static str> {
        let mut id = self.acceleration_context_id.lock();
        let current = *id;
        *id = current
            .checked_add(1)
            .ok_or("VirtIO GPU acceleration context IDs are exhausted")?;
        Ok(current)
    }

    fn next_acceleration_fence_id(&self) -> Result<u64, &'static str> {
        let mut id = self.acceleration_fence_id.lock();
        let current = *id;
        *id = current
            .checked_add(1)
            .ok_or("VirtIO GPU acceleration fence IDs are exhausted")?;
        Ok(current)
    }

    /// Send a command to the control queue
    fn send_control_command<T>(&self, cmd: &T) -> Result<(), &'static str> {
        let mut resp_buffer = [0u8; 128];
        self.send_control_command_with_resp_buffer(cmd, &mut resp_buffer)
    }

    fn send_execution_command<T>(&self, cmd: &T) -> Result<(), &'static str> {
        // SAFETY: the command remains borrowed until the synchronous control
        // request completes, and every command type passed here is repr(C).
        let cmd_buffer = unsafe {
            core::slice::from_raw_parts(cmd as *const T as *const u8, core::mem::size_of::<T>())
        };
        self.send_execution_bytes(cmd_buffer)
    }

    fn send_execution_bytes(&self, cmd_buffer: &[u8]) -> Result<(), &'static str> {
        let mut resp_buffer = [0u8; core::mem::size_of::<VirtioGpuCtrlHdr>()];
        self.send_control_bytes_with_resp_buffer(cmd_buffer, &mut resp_buffer)?;
        // SAFETY: `resp_buffer` contains exactly one VirtIO GPU control header
        // written by the synchronous control queue response path and may be
        // unaligned because it is byte storage.
        let response =
            unsafe { core::ptr::read_unaligned(resp_buffer.as_ptr() as *const VirtioGpuCtrlHdr) };
        validate_execution_response(response, None)
    }

    /// Send a command to the control queue, using a caller-provided response buffer.
    fn send_control_command_with_resp_buffer<T>(
        &self,
        cmd: &T,
        resp_buffer: &mut [u8],
    ) -> Result<(), &'static str> {
        // SAFETY: The command object remains borrowed until the synchronous
        // control queue request completes.
        let cmd_buffer = unsafe {
            core::slice::from_raw_parts(cmd as *const T as *const u8, core::mem::size_of::<T>())
        };
        self.send_control_bytes_with_resp_buffer(cmd_buffer, resp_buffer)
    }

    /// Send raw command bytes to the control queue, using a caller-provided response buffer.
    fn send_control_bytes_with_resp_buffer(
        &self,
        cmd_buffer: &[u8],
        resp_buffer: &mut [u8],
    ) -> Result<(), &'static str> {
        if cmd_buffer.is_empty() || resp_buffer.is_empty() {
            return Err("VirtIO GPU control buffers must not be empty");
        }
        if *self.control_queue_failed.read() {
            return Err("VirtIO GPU control queue is unavailable after a timeout");
        }

        let page_size = crate::environment::PAGE_SIZE;
        let cmd_pages = cmd_buffer.len().div_ceil(page_size);
        let resp_pages = resp_buffer.len().div_ceil(page_size);
        let cmd_dma = ContiguousPages::new(cmd_pages)
            .ok_or("Failed to allocate VirtIO GPU command DMA buffer")?;
        let resp_dma = ContiguousPages::new(resp_pages)
            .ok_or("Failed to allocate VirtIO GPU response DMA buffer")?;
        // SAFETY: both PMM allocations cover at least the corresponding slice
        // length and remain alive until the synchronous queue request finishes.
        unsafe {
            core::ptr::copy_nonoverlapping(
                cmd_buffer.as_ptr(),
                cmd_dma.as_ptr() as *mut u8,
                cmd_buffer.len(),
            );
            core::ptr::write_bytes(resp_dma.as_ptr() as *mut u8, 0, resp_buffer.len());
        }

        let mut virtqueues = self.virtqueues.lock();
        let control_queue = &mut virtqueues[0]; // Control queue is index 0

        // Allocate descriptors
        let cmd_desc = control_queue
            .alloc_desc()
            .ok_or("Failed to allocate command descriptor")?;
        let resp_desc = match control_queue.alloc_desc() {
            Some(desc) => desc,
            None => {
                // Free the already allocated cmd_desc before returning error
                control_queue.free_desc(cmd_desc);
                return Err("Failed to allocate response descriptor");
            }
        };

        // Set up command descriptor (device readable)
        let cmd_desc_ptr =
            &mut control_queue.desc[cmd_desc] as *mut crate::drivers::virtio::queue::Descriptor;
        unsafe {
            core::ptr::write_volatile(&mut (*cmd_desc_ptr).addr, cmd_dma.as_paddr() as u64);
            core::ptr::write_volatile(&mut (*cmd_desc_ptr).len, cmd_buffer.len() as u32);
            core::ptr::write_volatile(&mut (*cmd_desc_ptr).flags, DescriptorFlag::Next as u16);
            core::ptr::write_volatile(&mut (*cmd_desc_ptr).next, resp_desc as u16);
        }

        // Set up response descriptor (device writable)
        let resp_desc_ptr =
            &mut control_queue.desc[resp_desc] as *mut crate::drivers::virtio::queue::Descriptor;
        unsafe {
            core::ptr::write_volatile(&mut (*resp_desc_ptr).addr, resp_dma.as_paddr() as u64);
            core::ptr::write_volatile(&mut (*resp_desc_ptr).len, resp_buffer.len() as u32);
            core::ptr::write_volatile(&mut (*resp_desc_ptr).flags, DescriptorFlag::Write as u16);
        }

        // Submit the request to the queue
        if let Err(e) = control_queue.push(cmd_desc) {
            // Free descriptors if push fails
            control_queue.free_desc(resp_desc);
            control_queue.free_desc(cmd_desc);
            return Err(e);
        }

        // Notify the device
        self.notify(0); // Notify control queue

        // Wait for the device-owned used index through VirtQueue's volatile
        // accessor. A second plain dereference of used.idx used to follow this
        // loop; that is not a valid way to observe DMA updates and could spin
        // forever even after the device completed the request.
        let wait_started_ns = crate::timer::get_time_ns();
        let mut wait_spins = 0u64;
        while control_queue.is_busy() {
            wait_spins = wait_spins.saturating_add(1);
            let timed_out = crate::timer::get_time_ns().saturating_sub(wait_started_ns)
                >= VIRTIO_GPU_CONTROL_TIMEOUT_NS;
            if timed_out || wait_spins >= VIRTIO_GPU_CONTROL_MAX_SPINS {
                *self.control_queue_failed.write() = true;

                // The device may still own both descriptors. Keep their DMA
                // allocations alive and leave the descriptors reserved rather
                // than allowing a late device write into freed memory. The
                // failed queue is permanently rejected above, so this bounded
                // one-request quarantine cannot accumulate.
                core::mem::forget(cmd_dma);
                core::mem::forget(resp_dma);
                return Err("VirtIO GPU control queue timed out");
            }
            core::hint::spin_loop();
        }

        // Process response
        let _resp_idx = match control_queue.pop() {
            Some(idx) => idx,
            None => {
                // Free descriptors even if pop fails (device may have processed them)
                control_queue.free_desc(resp_desc);
                control_queue.free_desc(cmd_desc);
                return Err("No response from device");
            }
        };

        // Free descriptors (responsibility of driver, not VirtQueue)
        control_queue.free_desc(resp_desc);
        control_queue.free_desc(cmd_desc);

        // SAFETY: the response PMM allocation covers `resp_buffer.len()` bytes
        // and the device has completed writing before this copy executes.
        unsafe {
            core::ptr::copy_nonoverlapping(
                resp_dma.as_ptr() as *const u8,
                resp_buffer.as_mut_ptr(),
                resp_buffer.len(),
            );
        }

        Ok(())
    }

    fn negotiated_feature_enabled(&self, feature: u32) -> bool {
        (*self.negotiated_features.read() & (1u64 << feature)) != 0
    }

    fn read_config_u32(&self, offset: usize) -> u32 {
        self.read_config(offset)
    }

    fn require_virgl(&self) -> Result<(), &'static str> {
        if self.acceleration_usable() {
            Ok(())
        } else {
            Err("virtio-gpu VirGL acceleration is not usable")
        }
    }

    fn acceleration_usable(&self) -> bool {
        *self.transport_ready.read()
            && self.negotiated_feature_enabled(VIRTIO_GPU_F_VIRGL)
            && self.read_config_u32(VIRTIO_GPU_CONFIG_NUM_CAPSETS_OFFSET) != 0
    }

    fn context_init_supported(&self) -> bool {
        self.negotiated_feature_enabled(VIRTIO_GPU_F_CONTEXT_INIT)
    }

    fn gpu_backend_info(&self) -> GpuBackendInfo {
        let negotiated_features = *self.negotiated_features.read();
        let mut opaque_info = [0u8; 12];
        opaque_info[..8].copy_from_slice(&negotiated_features.to_le_bytes());
        let acceleration_usable = self.acceleration_usable();
        let dialect_count = if acceleration_usable {
            self.read_config_u32(VIRTIO_GPU_CONFIG_NUM_CAPSETS_OFFSET)
        } else {
            0
        };
        opaque_info[8..].copy_from_slice(&dialect_count.to_le_bytes());

        let state = if *self.transport_ready.read() {
            GpuDeviceState::Ready
        } else {
            GpuDeviceState::Unavailable
        };
        GpuBackendInfo::new(
            GpuDeviceInfo::new(
                state,
                GPU_EXECUTION_SUPPORT_MEMORY
                    | GPU_EXECUTION_SUPPORT_TIMELINE
                    | if acceleration_usable {
                        GPU_EXECUTION_SUPPORT_IMAGE_UPLOAD
                            | GPU_EXECUTION_SUPPORT_IMAGE_READBACK
                            | GPU_EXECUTION_SUPPORT_DEPTH
                            | GPU_EXECUTION_SUPPORT_QUEUE
                            | GPU_EXECUTION_SUPPORT_PRESENTATION
                    } else {
                        0
                    },
                if acceleration_usable {
                    VIRTIO_GPU_MAX_OPAQUE_COMMAND_SIZE
                } else {
                    0
                },
            ),
            negotiated_features,
            b"virtio-gpu",
            &opaque_info,
        )
    }

    fn get_backend_dialect_info(
        &self,
        index: u32,
    ) -> Result<VirtioGpuBackendDialectInfo, &'static str> {
        self.require_virgl()?;

        let cmd = VirtioGpuGetCapsetInfo {
            hdr: VirtioGpuCtrlHdr {
                hdr_type: VIRTIO_GPU_CMD_GET_CAPSET_INFO,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
            capset_index: index,
            padding: 0,
        };
        let mut resp_buffer = [0u8; core::mem::size_of::<VirtioGpuRespCapsetInfo>()];
        self.send_control_command_with_resp_buffer(&cmd, &mut resp_buffer)?;

        // SAFETY: The response buffer is exactly the response structure size and
        // may be unaligned because it is byte storage.
        let response = unsafe {
            core::ptr::read_unaligned(resp_buffer.as_ptr() as *const VirtioGpuRespCapsetInfo)
        };
        if response.hdr.hdr_type != VIRTIO_GPU_RESP_OK_CAPSET_INFO {
            return Err("GET_CAPSET_INFO returned unexpected response");
        }

        Ok(VirtioGpuBackendDialectInfo {
            id: response.capset_id,
            max_version: response.capset_max_version,
            max_size: response.capset_max_size,
        })
    }

    fn read_backend_dialect(
        &self,
        id: u32,
        version: u32,
        buffer: &mut [u8],
    ) -> Result<usize, &'static str> {
        self.require_virgl()?;

        let cmd = VirtioGpuGetCapset {
            hdr: VirtioGpuCtrlHdr {
                hdr_type: VIRTIO_GPU_CMD_GET_CAPSET,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
            capset_id: id,
            capset_version: version,
        };
        let header_size = core::mem::size_of::<VirtioGpuCtrlHdr>();
        let mut resp_buffer = alloc::vec![0u8; header_size + buffer.len()];
        self.send_control_command_with_resp_buffer(&cmd, &mut resp_buffer)?;

        // SAFETY: The response starts with a VirtIO GPU control header. The
        // byte buffer may be unaligned, so use read_unaligned.
        let response =
            unsafe { core::ptr::read_unaligned(resp_buffer.as_ptr() as *const VirtioGpuCtrlHdr) };
        if response.hdr_type != VIRTIO_GPU_RESP_OK_CAPSET {
            return Err("GET_CAPSET returned unexpected response");
        }

        let data_len = buffer
            .len()
            .min(resp_buffer.len().saturating_sub(header_size));
        buffer[..data_len].copy_from_slice(&resp_buffer[header_size..header_size + data_len]);
        Ok(data_len)
    }

    fn create_acceleration_context(
        &self,
        context_id: u32,
        debug_name: &str,
        context_init: u32,
    ) -> Result<(), &'static str> {
        self.require_virgl()?;

        let mut name = [0u8; VIRTIO_GPU_MAX_CONTEXT_NAME];
        let name_bytes = debug_name.as_bytes();
        let name_len = name_bytes.len().min(VIRTIO_GPU_MAX_CONTEXT_NAME);
        name[..name_len].copy_from_slice(&name_bytes[..name_len]);

        let cmd = VirtioGpuCtxCreate {
            hdr: VirtioGpuCtrlHdr {
                hdr_type: VIRTIO_GPU_CMD_CTX_CREATE,
                flags: 0,
                fence_id: 0,
                ctx_id: context_id,
                padding: 0,
            },
            nlen: name_len as u32,
            context_init,
            debug_name: name,
        };
        self.send_execution_command(&cmd)
    }

    fn destroy_acceleration_context(&self, context_id: u32) -> Result<(), &'static str> {
        self.require_virgl()?;

        let cmd = VirtioGpuCtxDestroy {
            hdr: VirtioGpuCtrlHdr {
                hdr_type: VIRTIO_GPU_CMD_CTX_DESTROY,
                flags: 0,
                fence_id: 0,
                ctx_id: context_id,
                padding: 0,
            },
        };
        self.send_execution_command(&cmd)
    }

    #[allow(dead_code)]
    fn create_acceleration_resource(
        &self,
        description: VirtioGpuAccelerationResource3d,
    ) -> Result<u32, &'static str> {
        self.require_virgl()?;
        if description.width == 0 || description.height == 0 || description.depth == 0 {
            return Err("Cannot create a zero-sized GPU 3D resource");
        }

        let resource_id = if description.resource_id == 0 {
            self.next_resource_id()
        } else {
            description.resource_id
        };
        let cmd = VirtioGpuResourceCreate3d {
            hdr: VirtioGpuCtrlHdr {
                hdr_type: VIRTIO_GPU_CMD_RESOURCE_CREATE_3D,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
            resource_id,
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
            padding: 0,
        };
        self.send_execution_command(&cmd)?;
        Ok(resource_id)
    }

    #[allow(dead_code)]
    fn unref_acceleration_resource(&self, resource_id: u32) -> Result<(), &'static str> {
        self.require_virgl()?;
        if resource_id == 0 {
            return Err("Cannot unreference resource 0");
        }

        let cmd = VirtioGpuResourceUnref {
            hdr: VirtioGpuCtrlHdr {
                hdr_type: VIRTIO_GPU_CMD_RESOURCE_UNREF,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
            resource_id,
            padding: 0,
        };
        self.send_execution_command(&cmd)
    }

    #[allow(dead_code)]
    fn attach_acceleration_resource(
        &self,
        context_id: u32,
        resource_id: u32,
    ) -> Result<(), &'static str> {
        self.require_virgl()?;
        if resource_id == 0 {
            return Err("Cannot attach resource 0");
        }

        let cmd = VirtioGpuCtxResource {
            hdr: VirtioGpuCtrlHdr {
                hdr_type: VIRTIO_GPU_CMD_CTX_ATTACH_RESOURCE,
                flags: 0,
                fence_id: 0,
                ctx_id: context_id,
                padding: 0,
            },
            resource_id,
            padding: 0,
        };
        self.send_execution_command(&cmd)
    }

    #[allow(dead_code)]
    fn detach_acceleration_resource(
        &self,
        context_id: u32,
        resource_id: u32,
    ) -> Result<(), &'static str> {
        self.require_virgl()?;
        if resource_id == 0 {
            return Err("Cannot detach resource 0");
        }

        let cmd = VirtioGpuCtxResource {
            hdr: VirtioGpuCtrlHdr {
                hdr_type: VIRTIO_GPU_CMD_CTX_DETACH_RESOURCE,
                flags: 0,
                fence_id: 0,
                ctx_id: context_id,
                padding: 0,
            },
            resource_id,
            padding: 0,
        };
        self.send_execution_command(&cmd)
    }

    #[allow(dead_code)]
    fn transfer_acceleration_resource(
        &self,
        command_type: u32,
        transfer: VirtioGpuAccelerationTransfer3d,
    ) -> Result<(), &'static str> {
        self.require_virgl()?;
        if transfer.resource_id == 0 {
            return Err("Cannot transfer resource 0");
        }
        if transfer.width == 0 || transfer.height == 0 || transfer.depth == 0 {
            return Err("Cannot issue a zero-sized GPU 3D transfer");
        }

        let cmd = VirtioGpuTransferHost3d {
            hdr: VirtioGpuCtrlHdr {
                hdr_type: command_type,
                flags: if transfer.fence_id.is_some() {
                    VIRTIO_GPU_FLAG_FENCE
                } else {
                    0
                },
                fence_id: transfer.fence_id.unwrap_or(0),
                ctx_id: transfer.context_id,
                padding: 0,
            },
            box_: VirtioGpuBox {
                x: transfer.x,
                y: transfer.y,
                z: transfer.z,
                width: transfer.width,
                height: transfer.height,
                depth: transfer.depth,
            },
            offset: transfer.offset,
            resource_id: transfer.resource_id,
            level: transfer.level,
            stride: transfer.stride,
            layer_stride: transfer.layer_stride,
        };
        let mut resp_buffer = [0u8; core::mem::size_of::<VirtioGpuCtrlHdr>()];
        self.send_control_command_with_resp_buffer(&cmd, &mut resp_buffer)?;
        // SAFETY: `resp_buffer` contains exactly one VirtIO GPU control header
        // written by the synchronous control queue response path and may be
        // unaligned because it is byte storage.
        let response =
            unsafe { core::ptr::read_unaligned(resp_buffer.as_ptr() as *const VirtioGpuCtrlHdr) };
        validate_execution_response(response, transfer.fence_id)
    }

    #[allow(dead_code)]
    fn transfer_acceleration_to_host(
        &self,
        transfer: VirtioGpuAccelerationTransfer3d,
    ) -> Result<(), &'static str> {
        self.transfer_acceleration_resource(VIRTIO_GPU_CMD_TRANSFER_TO_HOST_3D, transfer)
    }

    #[allow(dead_code)]
    fn transfer_acceleration_from_host(
        &self,
        transfer: VirtioGpuAccelerationTransfer3d,
    ) -> Result<(), &'static str> {
        self.transfer_acceleration_resource(VIRTIO_GPU_CMD_TRANSFER_FROM_HOST_3D, transfer)
    }

    fn submit_acceleration_commands(
        &self,
        context_id: u32,
        commands: &[u8],
        fence_id: Option<u64>,
    ) -> Result<(), GpuBackendSubmitError> {
        self.require_virgl()
            .map_err(GpuBackendSubmitError::Unavailable)?;
        if commands.is_empty() {
            return Err(GpuBackendSubmitError::Rejected(
                "Cannot submit an empty GPU command stream",
            ));
        }

        let header = VirtioGpuCmdSubmit3d {
            hdr: VirtioGpuCtrlHdr {
                hdr_type: VIRTIO_GPU_CMD_SUBMIT_3D,
                flags: if fence_id.is_some() {
                    VIRTIO_GPU_FLAG_FENCE
                } else {
                    0
                },
                fence_id: fence_id.unwrap_or(0),
                ctx_id: context_id,
                padding: 0,
            },
            size: commands.len() as u32,
            padding: 0,
        };

        let mut cmd_buffer =
            Vec::with_capacity(core::mem::size_of::<VirtioGpuCmdSubmit3d>() + commands.len());
        append_pod_bytes(&mut cmd_buffer, &header);
        cmd_buffer.extend_from_slice(commands);

        let mut resp_buffer = [0u8; core::mem::size_of::<VirtioGpuCtrlHdr>()];
        self.send_control_bytes_with_resp_buffer(&cmd_buffer, &mut resp_buffer)
            .map_err(classify_submit_transport_error)?;
        // SAFETY: `resp_buffer` contains exactly one VirtIO GPU control header
        // written by the synchronous control queue response path and may be
        // unaligned because it is byte storage.
        let response =
            unsafe { core::ptr::read_unaligned(resp_buffer.as_ptr() as *const VirtioGpuCtrlHdr) };
        validate_submit_response(response, fence_id)
    }

    /// Get display information from the device
    fn get_display_info_internal(&self) -> Result<(), &'static str> {
        // Create get display info command
        let cmd = VirtioGpuCtrlHdr {
            hdr_type: VIRTIO_GPU_CMD_GET_DISPLAY_INFO,
            flags: 0,
            fence_id: 0,
            ctx_id: 0,
            padding: 0,
        };

        // Send command.
        // GET_DISPLAY_INFO returns a `virtio_gpu_resp_display_info`, which is 408 bytes
        // (header + 16 scanouts). If we provide a smaller response buffer, QEMU logs:
        // `virtio_gpu_ctrl_response: response size incorrect 128 vs 408`.
        // Avoid allocating this response buffer on the stack; use the heap instead.
        let mut resp_buffer = alloc::vec![0u8; core::mem::size_of::<VirtioGpuRespDisplayInfo>()];
        self.send_control_command_with_resp_buffer(&cmd, &mut resp_buffer)?;

        let mut display_info = unsafe {
            core::ptr::read_unaligned(resp_buffer.as_ptr() as *const VirtioGpuRespDisplayInfo)
        };
        if display_info.hdr.hdr_type != VIRTIO_GPU_RESP_OK_DISPLAY_INFO {
            return Err("GET_DISPLAY_INFO returned unexpected response");
        }

        let primary = display_info.pmodes[0];
        if primary.enabled == 0 || primary.r.width == 0 || primary.r.height == 0 {
            if self.display_info.read().is_some() {
                return Ok(());
            }

            display_info = VirtioGpuRespDisplayInfo {
                hdr: VirtioGpuCtrlHdr {
                    hdr_type: VIRTIO_GPU_RESP_OK_DISPLAY_INFO,
                    flags: 0,
                    fence_id: 0,
                    ctx_id: 0,
                    padding: 0,
                },
                pmodes: [VirtioGpuDisplayOne {
                    r: VirtioGpuRect {
                        x: 0,
                        y: 0,
                        width: 1920,
                        height: 1080,
                    },
                    enabled: 1,
                    flags: 0,
                }; VIRTIO_GPU_MAX_SCANOUTS],
            };

            // Only enable the first display
            for i in 1..VIRTIO_GPU_MAX_SCANOUTS {
                display_info.pmodes[i].enabled = 0;
            }
        }

        *self.display_info.write() = Some(display_info);
        Ok(())
    }

    /// Create a 2D resource
    fn create_2d_resource(
        &self,
        width: u32,
        height: u32,
        format: u32,
    ) -> Result<u32, &'static str> {
        let resource_id = self.next_resource_id();

        let cmd = VirtioGpuResourceCreate2d {
            hdr: VirtioGpuCtrlHdr {
                hdr_type: VIRTIO_GPU_CMD_RESOURCE_CREATE_2D,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
            resource_id,
            format,
            width,
            height,
        };

        self.send_control_command(&cmd)?;
        Ok(resource_id)
    }

    /// Attach backing memory to a resource.
    fn attach_backing_entries_to_resource(
        &self,
        resource_id: u32,
        entries: &[VirtioGpuAccelerationMemoryEntry],
    ) -> Result<(), &'static str> {
        if resource_id == 0 {
            return Err("Cannot attach backing to resource 0");
        }
        if entries.is_empty() {
            return Err("Cannot attach empty resource backing");
        }
        if entries.len() > u32::MAX as usize {
            return Err("Too many GPU backing entries");
        }

        let attach = VirtioGpuResourceAttachBacking {
            hdr: VirtioGpuCtrlHdr {
                hdr_type: VIRTIO_GPU_CMD_RESOURCE_ATTACH_BACKING,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
            resource_id,
            nr_entries: entries.len() as u32,
        };
        let mut cmd_buffer = Vec::with_capacity(
            core::mem::size_of::<VirtioGpuResourceAttachBacking>()
                + entries.len() * core::mem::size_of::<VirtioGpuMemEntry>(),
        );
        append_pod_bytes(&mut cmd_buffer, &attach);
        for entry in entries {
            if entry.length == 0 || entry.length > u32::MAX as usize {
                return Err("GPU backing entry length is invalid");
            }
            append_pod_bytes(
                &mut cmd_buffer,
                &VirtioGpuMemEntry {
                    addr: entry.paddr as u64,
                    length: entry.length as u32,
                    padding: 0,
                },
            );
        }

        self.send_execution_bytes(&cmd_buffer)
    }

    /// Attach one contiguous backing memory range to a resource.
    fn attach_backing_to_resource(
        &self,
        resource_id: u32,
        addr: usize,
        size: usize,
    ) -> Result<(), &'static str> {
        self.attach_backing_entries_to_resource(
            resource_id,
            core::slice::from_ref(&VirtioGpuAccelerationMemoryEntry {
                paddr: addr,
                length: size,
            }),
        )
    }

    /// Detach backing memory from a resource.
    fn detach_backing_from_resource(&self, resource_id: u32) -> Result<(), &'static str> {
        if resource_id == 0 {
            return Err("Cannot detach backing from resource 0");
        }

        let cmd = VirtioGpuResourceDetachBacking {
            hdr: VirtioGpuCtrlHdr {
                hdr_type: VIRTIO_GPU_CMD_RESOURCE_DETACH_BACKING,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
            resource_id,
            padding: 0,
        };
        self.send_execution_command(&cmd)
    }

    /// Set up framebuffer
    fn setup_framebuffer(&self) -> Result<(), &'static str> {
        let display_info = self.display_info.read();
        let display_info = display_info.as_ref().ok_or("No display info available")?;
        let primary_display = &display_info.pmodes[0];
        if primary_display.enabled == 0 {
            return Err("Primary display not enabled");
        }
        let width = primary_display.r.width;
        let height = primary_display.r.height;
        let resource_id =
            self.create_2d_resource(width, height, VIRTIO_GPU_FORMAT_B8G8R8X8_UNORM)?;
        let fb_size = (width * height * 4) as usize;
        let fb_pages = (fb_size + 4095) / 4096;
        let fb_alloc =
            ContiguousPages::new(fb_pages).ok_or("Failed to allocate framebuffer memory")?;
        let fb_addr = fb_alloc.as_paddr();

        self.attach_backing_to_resource(resource_id, fb_addr, fb_size)?;
        self.set_primary_scanout(resource_id, width, height)?;
        unsafe {
            let fb_virt = crate::vm::addr::phys_to_virt(fb_addr) as *mut u8;
            ptr::write_bytes(fb_virt, 0, fb_size);
        }
        {
            let mut resources = self.resources.lock();
            resources.insert(resource_id, (fb_addr, fb_size));
        }
        if let Some(old_alloc) = self.framebuffer_alloc.write().replace(fb_alloc) {
            self.retired_framebuffer_allocs.lock().push(old_alloc);
        }
        *self.framebuffer_addr.write() = Some(fb_addr);
        *self.current_resource_id.write() = Some(resource_id);
        Ok(())
    }

    fn poll_display_resize(&self) -> Result<(), &'static str> {
        let old_mode = {
            let display_info = self.display_info.read();
            display_info
                .as_ref()
                .map(|info| (info.pmodes[0].r.width, info.pmodes[0].r.height))
        };

        let old_display_info = *self.display_info.read();
        self.get_display_info_internal()?;

        let new_mode = {
            let display_info = self.display_info.read();
            display_info
                .as_ref()
                .map(|info| (info.pmodes[0].r.width, info.pmodes[0].r.height))
        };

        if let (Some((old_width, old_height)), Some((new_width, new_height))) = (old_mode, new_mode)
        {
            if (old_width != new_width || old_height != new_height)
                && new_width != 0
                && new_height != 0
            {
                crate::early_println!(
                    "[virtio-gpu] display resize: {}x{} -> {}x{}",
                    old_width,
                    old_height,
                    new_width,
                    new_height
                );
                if let Err(e) = self.setup_framebuffer() {
                    *self.display_info.write() = old_display_info;
                    return Err(e);
                }
            }
        }

        Ok(())
    }

    fn get_display_name(&self) -> &'static str {
        "virtio-gpu"
    }

    fn get_framebuffer_config(&self) -> Result<FramebufferConfig, &'static str> {
        let display_info = self.display_info.read();
        let display_info = display_info.as_ref().ok_or("Device not initialized")?;

        let primary_display = &display_info.pmodes[0];
        if primary_display.enabled == 0 {
            return Err("Primary display not enabled");
        }

        Ok(FramebufferConfig::new(
            primary_display.r.width,
            primary_display.r.height,
            PixelFormat::BGRA8888, // VirtIO GPU typically uses BGRA format
        ))
    }

    fn get_framebuffer_address(&self) -> Result<usize, &'static str> {
        self.framebuffer_addr
            .read()
            .ok_or("Framebuffer not initialized")
    }

    fn clamp_region_to_rect(
        region: DisplayRegion,
        width: u32,
        height: u32,
    ) -> Option<VirtioGpuRect> {
        if region.width == 0 || region.height == 0 || region.x >= width || region.y >= height {
            return None;
        }

        Some(VirtioGpuRect {
            x: region.x,
            y: region.y,
            width: region.width.min(width - region.x),
            height: region.height.min(height - region.y),
        })
    }

    fn set_primary_scanout(
        &self,
        resource_id: u32,
        width: u32,
        height: u32,
    ) -> Result<(), &'static str> {
        if resource_id == 0 || width == 0 || height == 0 {
            return Err("Primary scanout resource is invalid");
        }
        if *self.scanout_resource_id.read() == Some(resource_id) {
            return Ok(());
        }

        let scanout_cmd = VirtioGpuSetScanout {
            hdr: VirtioGpuCtrlHdr {
                hdr_type: VIRTIO_GPU_CMD_SET_SCANOUT,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
            r: VirtioGpuRect {
                x: 0,
                y: 0,
                width,
                height,
            },
            scanout_id: 0,
            resource_id,
        };
        self.send_control_command(&scanout_cmd)?;
        *self.scanout_resource_id.write() = Some(resource_id);
        Ok(())
    }

    fn flush_resource_region(
        &self,
        resource_id: u32,
        rect: VirtioGpuRect,
    ) -> Result<(), &'static str> {
        let flush_cmd = VirtioGpuResourceFlush {
            hdr: VirtioGpuCtrlHdr {
                hdr_type: VIRTIO_GPU_CMD_RESOURCE_FLUSH,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
            r: rect,
            resource_id,
            padding: 0,
        };

        self.send_control_command(&flush_cmd)
    }

    fn present_framebuffer_region(&self, region: DisplayRegion) -> Result<(), &'static str> {
        let display_info = self.display_info.read();
        let display_info = display_info.as_ref().ok_or("Device not initialized")?;
        let primary = display_info.pmodes[0];
        if primary.enabled == 0 {
            return Err("Primary display not enabled");
        }
        let Some(rect) = Self::clamp_region_to_rect(region, primary.r.width, primary.r.height)
        else {
            return Ok(());
        };

        let resource_id = self
            .current_resource_id
            .read()
            .ok_or("No framebuffer resource found")?;
        self.set_primary_scanout(resource_id, primary.r.width, primary.r.height)?;

        let offset = ((rect.y as u64 * primary.r.width as u64) + rect.x as u64) * 4;
        let transfer_cmd = VirtioGpuTransferToHost2d {
            hdr: VirtioGpuCtrlHdr {
                hdr_type: VIRTIO_GPU_CMD_TRANSFER_TO_HOST_2D,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
            r: rect,
            offset,
            resource_id,
            padding: 0,
        };

        self.send_control_command(&transfer_cmd)?;
        self.flush_resource_region(resource_id, rect)
    }

    fn present_gpu_resource_region(
        &self,
        resource: GpuDisplayResource,
        region: DisplayRegion,
    ) -> Result<(), &'static str> {
        if resource.backend_cookie() != self.backend_cookie() {
            return Err("GPU display resource belongs to a different VirtIO GPU device");
        }
        if resource.resource_id() == 0 || resource.width() == 0 || resource.height() == 0 {
            return Err("GPU display resource is invalid");
        }
        let Some(rect) = Self::clamp_region_to_rect(region, resource.width(), resource.height())
        else {
            return Ok(());
        };

        self.set_primary_scanout(resource.resource_id(), resource.width(), resource.height())?;
        self.flush_resource_region(resource.resource_id(), rect)
    }
}

impl VirtioDevice for VirtioGpuDeviceCore {
    fn pci_transport(&self) -> Option<VirtioPciTransport> {
        self.pci_transport
    }

    fn get_base_addr(&self) -> usize {
        self.base_addr
    }

    fn get_virtqueue_count(&self) -> usize {
        2 // Control queue and cursor queue
    }

    fn get_virtqueue_size(&self, queue_idx: usize) -> usize {
        if queue_idx >= self.get_virtqueue_count() {
            panic!("Invalid queue index: {}", queue_idx);
        }

        let virtqueues = self.virtqueues.lock();
        virtqueues[queue_idx].get_queue_size()
    }

    fn get_queue_desc_addr(&self, queue_idx: usize) -> Option<u64> {
        if queue_idx >= self.get_virtqueue_count() {
            return None;
        }

        let virtqueues = self.virtqueues.lock();
        Some(virt_to_phys(virtqueues[queue_idx].desc.as_ptr() as usize) as u64)
    }

    fn get_queue_driver_addr(&self, queue_idx: usize) -> Option<u64> {
        if queue_idx >= self.get_virtqueue_count() {
            return None;
        }

        let virtqueues = self.virtqueues.lock();
        Some(virt_to_phys(virtqueues[queue_idx].avail.flags as *const u16 as usize) as u64)
    }

    fn get_queue_device_addr(&self, queue_idx: usize) -> Option<u64> {
        if queue_idx >= self.get_virtqueue_count() {
            return None;
        }

        let virtqueues = self.virtqueues.lock();
        Some(virt_to_phys(virtqueues[queue_idx].used.flags as *const u16 as usize) as u64)
    }

    fn get_supported_features(&self, device_features: u64) -> u64 {
        let mut supported = (1u64 << VIRTIO_GPU_F_VIRGL)
            | (1u64 << VIRTIO_GPU_F_EDID)
            | (1u64 << VIRTIO_GPU_F_CONTEXT_INIT);
        if self.pci_transport().is_some() {
            supported |= 1u64 << crate::drivers::virtio::features::VIRTIO_F_VERSION_1;
        }
        device_features & supported
    }
}

pub struct VirtioGpuDevice {
    core: Arc<IrqSpinLock<VirtioGpuDeviceCore>>,
}

impl VirtioGpuDevice {
    /// Create a new VirtIO GPU device
    ///
    /// # Arguments
    ///
    /// * `base_addr` - The base address of the device
    ///
    /// # Returns
    ///
    /// A new instance of `VirtioGpuDevice`
    pub fn new(base_addr: usize) -> Self {
        Self {
            core: Arc::new(IrqSpinLock::new(VirtioGpuDeviceCore::new(base_addr))),
        }
    }

    /// Create a new VirtIO GPU device backed by the PCI transport.
    ///
    /// # Arguments
    ///
    /// * `transport` - Mapped VirtIO PCI configuration regions
    ///
    /// # Returns
    ///
    /// A new instance of `VirtioGpuDevice`.
    pub fn new_pci(transport: VirtioPciTransport) -> Self {
        Self {
            core: Arc::new(IrqSpinLock::new(VirtioGpuDeviceCore::new_pci(transport))),
        }
    }
}

impl Device for VirtioGpuDevice {
    fn device_type(&self) -> DeviceType {
        DeviceType::Graphics
    }

    fn name(&self) -> &'static str {
        "virtio-gpu"
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
        self
    }

    fn as_graphics_device(&self) -> Option<&dyn GraphicsDevice> {
        Some(self)
    }
}

impl ControlOps for VirtioGpuDevice {
    // VirtIO GPU devices don't support control operations by default
    fn control(&self, _command: u32, _arg: usize) -> Result<i32, &'static str> {
        Err("Control operations not supported")
    }
}

impl MemoryMappingOps for VirtioGpuDevice {
    fn get_mapping_info(
        &self,
        _offset: usize,
        _length: usize,
    ) -> Result<crate::object::capability::MemoryMappingInfo, &'static str> {
        Err("Memory mapping not supported by VirtIO GPU device")
    }

    fn on_mapped(&self, _vaddr: usize, _paddr: usize, _length: usize, _offset: usize) {
        // VirtIO GPU devices don't support memory mapping
    }

    fn on_unmapped(&self, _vaddr: usize, _length: usize) {
        // VirtIO GPU devices don't support memory mapping
    }

    fn supports_mmap(&self) -> bool {
        false
    }
}

impl Selectable for VirtioGpuDevice {
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

/// VirtIO acceleration adapter for the backend-neutral GPU control interface.
pub struct VirtioGpuBackend {
    core: Arc<IrqSpinLock<VirtioGpuDeviceCore>>,
}

impl VirtioGpuBackend {
    /// Create a backend adapter for a VirtIO GPU display device.
    ///
    /// # Arguments
    ///
    /// * `device` - VirtIO display device whose acceleration transport is queried.
    ///
    /// # Returns
    ///
    /// A backend-neutral GPU adapter sharing the device transport.
    pub fn from_device(device: &VirtioGpuDevice) -> Self {
        Self {
            core: Arc::clone(&device.core),
        }
    }

    fn dialect_token(info: VirtioGpuBackendDialectInfo) -> u64 {
        (u64::from(info.id) << 32) | u64::from(info.max_version)
    }

    fn query_dialect_info(
        core: &VirtioGpuDeviceCore,
        index: u32,
    ) -> Result<GpuBackendDialectInfo, &'static str> {
        let dialect = core.get_backend_dialect_info(index)?;
        let dialect_size = usize::try_from(dialect.max_size)
            .map_err(|_| "VirtIO GPU dialect size does not fit kernel address size")?
            .min(GPU_DIALECT_INFO_BYTES);
        let mut opaque_info = [0u8; GPU_DIALECT_INFO_BYTES];
        let read_size = if dialect_size == 0 {
            0
        } else {
            core.read_backend_dialect(
                dialect.id,
                dialect.max_version,
                &mut opaque_info[..dialect_size],
            )?
        };
        Ok(GpuBackendDialectInfo::new(
            index,
            Self::dialect_token(dialect),
            &opaque_info[..read_size],
        ))
    }
}

/// Real VirtIO execution context retained by a backend-neutral GPU context.
struct VirtioGpuBackendContext {
    core: Arc<IrqSpinLock<VirtioGpuDeviceCore>>,
    context_id: u32,
    info: GpuBackendContextInfo,
    attached_resources: IrqSpinLock<Vec<u32>>,
}

/// Real VirtIO image resource using generic kernel-owned backing when required.
struct VirtioGpuBackendImage {
    core: Arc<IrqSpinLock<VirtioGpuDeviceCore>>,
    resource_id: u32,
    create: GpuImageCreateInfo,
    allocation_size: u64,
    has_attached_backing: bool,
    backend_cookie: u64,
}

/// Real VirtIO buffer resource backed by a generic GPU buffer allocation.
struct VirtioGpuBackendBuffer {
    core: Arc<IrqSpinLock<VirtioGpuDeviceCore>>,
    resource_id: u32,
    allocation_size: u64,
    backend_cookie: u64,
}

impl Drop for VirtioGpuBackendImage {
    fn drop(&mut self) {
        let core = self.core.lock();
        if self.has_attached_backing {
            let _ = core.detach_backing_from_resource(self.resource_id);
        }
        let _ = core.unref_acceleration_resource(self.resource_id);
    }
}

impl GpuBackendImage for VirtioGpuBackendImage {
    fn query_info(&self) -> GpuBackendImageInfo {
        GpuBackendImageInfo::new(
            self.create,
            u64::from(self.resource_id),
            self.allocation_size,
        )
    }

    fn backend_cookie(&self) -> u64 {
        self.backend_cookie
    }

    fn display_resource(&self) -> Option<GpuDisplayResource> {
        (self.create.usage & GPU_IMAGE_USAGE_PRESENTABLE != 0).then_some(GpuDisplayResource::new(
            self.resource_id,
            self.create.width,
            self.create.height,
            self.backend_cookie,
        ))
    }
}

impl Drop for VirtioGpuBackendBuffer {
    fn drop(&mut self) {
        let core = self.core.lock();
        let _ = core.detach_backing_from_resource(self.resource_id);
        let _ = core.unref_acceleration_resource(self.resource_id);
    }
}

impl GpuBackendBuffer for VirtioGpuBackendBuffer {
    fn query_info(&self) -> GpuBackendBufferInfo {
        GpuBackendBufferInfo::new(u64::from(self.resource_id), self.allocation_size)
    }

    fn backend_cookie(&self) -> u64 {
        self.backend_cookie
    }
}

impl Drop for VirtioGpuBackendContext {
    fn drop(&mut self) {
        let core = self.core.lock();
        let mut attached_resources = self.attached_resources.lock();
        for resource_id in attached_resources.drain(..) {
            let _ = core.detach_acceleration_resource(self.context_id, resource_id);
        }
        let _ = core.destroy_acceleration_context(self.context_id);
    }
}

impl VirtioGpuBackendContext {
    fn transfer_image_bgra(
        &self,
        image: &dyn GpuBackendImage,
        transfer: GpuImageUploadInfo,
    ) -> Result<(), &'static str> {
        let info = image.query_info();
        let resource_id = u32::try_from(info.command_resource_token)
            .map_err(|_| "VirtIO GPU image token does not fit a resource ID")?;
        let core = self.core.lock();
        if resource_id == 0 || image.backend_cookie() != core.backend_cookie() {
            return Err("GPU image belongs to a different VirtIO GPU device");
        }
        if !self
            .attached_resources
            .lock()
            .iter()
            .any(|&attached_resource_id| attached_resource_id == resource_id)
        {
            return Err("VirtIO GPU image is not attached to this context");
        }
        let fence_id = core.next_acceleration_fence_id()?;
        core.transfer_acceleration_to_host(VirtioGpuAccelerationTransfer3d {
            context_id: self.context_id,
            resource_id,
            fence_id: Some(fence_id),
            offset: transfer.backing_offset,
            level: 0,
            stride: transfer.backing_stride,
            layer_stride: transfer.backing_layer_stride,
            x: transfer.dst_x,
            y: transfer.dst_y,
            z: 0,
            width: transfer.width,
            height: transfer.height,
            depth: 1,
        })
    }

    fn transfer_image_from_host_bgra(
        &self,
        image: &dyn GpuBackendImage,
        readback: GpuImageUploadInfo,
    ) -> Result<(), &'static str> {
        let info = image.query_info();
        let resource_id = u32::try_from(info.command_resource_token)
            .map_err(|_| "VirtIO GPU image token does not fit a resource ID")?;
        let core = self.core.lock();
        if resource_id == 0
            || image.backend_cookie() != core.backend_cookie()
            || info.usage & GPU_IMAGE_USAGE_TRANSFER_SRC == 0
        {
            return Err("GPU image cannot be read back from this VirtIO GPU device");
        }
        if !self
            .attached_resources
            .lock()
            .iter()
            .any(|&attached_resource_id| attached_resource_id == resource_id)
        {
            return Err("VirtIO GPU image is not attached to this context");
        }
        let fence_id = core.next_acceleration_fence_id()?;
        core.transfer_acceleration_from_host(VirtioGpuAccelerationTransfer3d {
            context_id: self.context_id,
            resource_id,
            fence_id: Some(fence_id),
            offset: readback.backing_offset,
            level: 0,
            stride: readback.backing_stride,
            layer_stride: readback.backing_layer_stride,
            x: readback.dst_x,
            y: readback.dst_y,
            z: 0,
            width: readback.width,
            height: readback.height,
            depth: 1,
        })
    }
}

impl GpuBackendContext for VirtioGpuBackendContext {
    fn query_info(&self) -> GpuBackendContextInfo {
        self.info
    }

    fn create_queue(&self) -> Result<Arc<dyn GpuBackendQueue>, &'static str> {
        Ok(Arc::new(VirtioGpuBackendQueue {
            core: Arc::clone(&self.core),
            context_id: self.context_id,
        }))
    }

    fn attach_image(&self, image: &dyn GpuBackendImage) -> Result<u64, &'static str> {
        let info = image.query_info();
        let resource_id = u32::try_from(info.command_resource_token)
            .map_err(|_| "VirtIO GPU image token does not fit a resource ID")?;
        let core = self.core.lock();
        if resource_id == 0 || image.backend_cookie() != core.backend_cookie() {
            return Err("GPU image belongs to a different VirtIO GPU device");
        }
        let mut attached_resources = self.attached_resources.lock();
        if attached_resources
            .iter()
            .any(|&attached_resource_id| attached_resource_id == resource_id)
        {
            return Err("VirtIO GPU image is already attached to this context");
        }
        core.attach_acceleration_resource(self.context_id, resource_id)?;
        if attached_resources.try_reserve(1).is_err() {
            let _ = core.detach_acceleration_resource(self.context_id, resource_id);
            return Err("Failed to retain VirtIO GPU image attachment");
        }
        attached_resources.push(resource_id);
        Ok(u64::from(resource_id))
    }

    fn detach_image(&self, image: &dyn GpuBackendImage) -> Result<(), &'static str> {
        let info = image.query_info();
        let resource_id = u32::try_from(info.command_resource_token)
            .map_err(|_| "VirtIO GPU image token does not fit a resource ID")?;
        let core = self.core.lock();
        if resource_id == 0 || image.backend_cookie() != core.backend_cookie() {
            return Err("GPU image belongs to a different VirtIO GPU device");
        }
        let mut attached_resources = self.attached_resources.lock();
        let index = attached_resources
            .iter()
            .position(|&attached_resource_id| attached_resource_id == resource_id)
            .ok_or("VirtIO GPU image is not attached to this context")?;
        core.detach_acceleration_resource(self.context_id, resource_id)?;
        attached_resources.swap_remove(index);
        Ok(())
    }

    fn upload_image_bgra(
        &self,
        image: &dyn GpuBackendImage,
        upload: GpuImageUploadInfo,
    ) -> Result<(), &'static str> {
        self.transfer_image_bgra(image, upload)
    }

    fn transfer_imported_image_bgra(
        &self,
        image: &dyn GpuBackendImage,
        transfer: GpuImageUploadInfo,
    ) -> Result<(), &'static str> {
        self.transfer_image_bgra(image, transfer)
    }

    fn readback_image_bgra(
        &self,
        image: &dyn GpuBackendImage,
        readback: GpuImageUploadInfo,
    ) -> Result<(), &'static str> {
        self.transfer_image_from_host_bgra(image, readback)
    }

    fn attach_buffer(&self, buffer: &dyn GpuBackendBuffer) -> Result<u64, &'static str> {
        let info = buffer.query_info();
        let resource_id = u32::try_from(info.command_resource_token)
            .map_err(|_| "VirtIO GPU buffer token does not fit a resource ID")?;
        if resource_id == 0 || buffer.backend_cookie() != self.core.lock().backend_cookie() {
            return Err("GPU buffer belongs to a different VirtIO GPU device");
        }
        let core = self.core.lock();
        core.attach_acceleration_resource(self.context_id, resource_id)?;
        let mut attached_resources = self.attached_resources.lock();
        if attached_resources.try_reserve(1).is_err() {
            let _ = core.detach_acceleration_resource(self.context_id, resource_id);
            return Err("Failed to retain VirtIO GPU buffer attachment");
        }
        attached_resources.push(resource_id);
        Ok(u64::from(resource_id))
    }

    fn detach_buffer(&self, buffer: &dyn GpuBackendBuffer) -> Result<(), &'static str> {
        let info = buffer.query_info();
        let resource_id = u32::try_from(info.command_resource_token)
            .map_err(|_| "VirtIO GPU buffer token does not fit a resource ID")?;
        let core = self.core.lock();
        if resource_id == 0 || buffer.backend_cookie() != core.backend_cookie() {
            return Err("GPU buffer belongs to a different VirtIO GPU device");
        }
        core.detach_acceleration_resource(self.context_id, resource_id)?;
        let mut attached_resources = self.attached_resources.lock();
        if let Some(index) = attached_resources
            .iter()
            .position(|&attached_resource_id| attached_resource_id == resource_id)
        {
            attached_resources.swap_remove(index);
        }
        Ok(())
    }
}

/// Real VirtIO execution queue retained by a backend-neutral GPU queue.
struct VirtioGpuBackendQueue {
    core: Arc<IrqSpinLock<VirtioGpuDeviceCore>>,
    context_id: u32,
}

impl GpuBackendQueue for VirtioGpuBackendQueue {
    fn query_info(&self) -> GpuBackendQueueInfo {
        GpuBackendQueueInfo::new(VIRTIO_GPU_MAX_OPAQUE_COMMAND_SIZE)
    }

    fn submit(&self, commands: &[u8]) -> Result<(), GpuBackendSubmitError> {
        let core = self.core.lock();
        let fence_id = core
            .next_acceleration_fence_id()
            .map_err(GpuBackendSubmitError::Unavailable)?;
        core.submit_acceleration_commands(self.context_id, commands, Some(fence_id))
    }
}

impl GpuBackend for VirtioGpuBackend {
    fn query_info(&self) -> GpuBackendInfo {
        self.core.lock().gpu_backend_info()
    }

    fn query_dialect(&self, index: u32) -> Result<GpuBackendDialectInfo, &'static str> {
        let core = self.core.lock();
        Self::query_dialect_info(&core, index)
    }

    fn create_context(
        &self,
        dialect: GpuBackendDialectDescriptor,
    ) -> Result<Arc<dyn GpuBackendContext>, &'static str> {
        let core = self.core.lock();
        core.require_virgl()?;
        let effective_index = if core.context_init_supported() {
            dialect.index
        } else {
            0
        };
        let effective_dialect = core.get_backend_dialect_info(effective_index)?;
        let effective_token = Self::dialect_token(effective_dialect);
        if core.context_init_supported() && dialect.token != effective_token {
            return Err("VirtIO GPU dialect token does not match the selected dialect");
        }
        let context_init = if core.context_init_supported() {
            u32::from(
                u8::try_from(effective_dialect.id)
                    .map_err(|_| "VirtIO GPU dialect ID cannot initialize a context")?,
            )
        } else {
            0
        };
        let context_id = core.next_acceleration_context_id()?;
        core.create_acceleration_context(context_id, "scarlet-gpu", context_init)?;
        Ok(Arc::new(VirtioGpuBackendContext {
            core: Arc::clone(&self.core),
            context_id,
            info: GpuBackendContextInfo::new(effective_index, effective_token),
            attached_resources: IrqSpinLock::new(Vec::new()),
        }))
    }

    fn create_image(
        &self,
        create: GpuImageCreateInfo,
        backing: GpuImageBackingInfo,
    ) -> Result<Arc<dyn GpuBackendImage>, &'static str> {
        if backing.paddr == 0 || backing.allocation_size == 0 || !backing.is_physically_contiguous()
        {
            return Err("VirtIO GPU image backing is invalid");
        }
        let backing_size = usize::try_from(backing.allocation_size)
            .map_err(|_| "VirtIO GPU image backing does not fit kernel address size")?;
        let has_attached_backing = create.usage
            & (GPU_IMAGE_USAGE_PRESENTABLE
                | GPU_IMAGE_USAGE_TRANSFER_DST
                | GPU_IMAGE_USAGE_TRANSFER_SRC)
            != 0;
        let core = self.core.lock();
        core.require_virgl()?;
        let resource_id = core.create_acceleration_resource(VirtioGpuAccelerationResource3d {
            resource_id: 0,
            target: PIPE_TEXTURE_2D,
            format: match create.format {
                GPU_IMAGE_FORMAT_BGRA8_UNORM => VIRTIO_GPU_FORMAT_B8G8R8A8_UNORM,
                GPU_IMAGE_FORMAT_DEPTH32_FLOAT => VIRGL_FORMAT_Z32_FLOAT,
                _ => return Err("VirtIO GPU image format is unsupported"),
            },
            bind: virgl_image_bind(create.usage),
            width: create.width,
            height: create.height,
            depth: 1,
            array_size: 1,
            last_level: 0,
            nr_samples: 0,
            flags: 0,
        })?;
        if has_attached_backing
            && let Err(error) =
                core.attach_backing_to_resource(resource_id, backing.paddr, backing_size)
        {
            let _ = core.unref_acceleration_resource(resource_id);
            return Err(error);
        }
        Ok(Arc::new(VirtioGpuBackendImage {
            core: Arc::clone(&self.core),
            resource_id,
            create,
            allocation_size: backing.allocation_size,
            has_attached_backing,
            backend_cookie: core.backend_cookie(),
        }))
    }

    fn create_buffer(
        &self,
        create: GpuBufferCreateInfo,
    ) -> Result<Arc<dyn GpuBackendBuffer>, &'static str> {
        if create.paddr == 0 || create.allocation_size == 0 {
            return Err("VirtIO GPU buffer backing is invalid");
        }
        let backing_size = usize::try_from(create.allocation_size)
            .map_err(|_| "VirtIO GPU buffer allocation does not fit kernel address size")?;
        let core = self.core.lock();
        core.require_virgl()?;
        let resource_id = core.create_acceleration_resource(VirtioGpuAccelerationResource3d {
            resource_id: 0,
            target: PIPE_BUFFER,
            format: 0,
            bind: PIPE_BIND_VERTEX_BUFFER,
            width: u32::try_from(create.allocation_size)
                .map_err(|_| "VirtIO GPU buffer allocation exceeds PIPE_BUFFER limits")?,
            height: 1,
            depth: 1,
            array_size: 1,
            last_level: 0,
            nr_samples: 0,
            flags: 0,
        })?;
        if let Err(error) = core.attach_backing_to_resource(resource_id, create.paddr, backing_size)
        {
            let _ = core.unref_acceleration_resource(resource_id);
            return Err(error);
        }
        Ok(Arc::new(VirtioGpuBackendBuffer {
            core: Arc::clone(&self.core),
            resource_id,
            allocation_size: create.allocation_size,
            backend_cookie: core.backend_cookie(),
        }))
    }
}

impl GraphicsDevice for VirtioGpuDevice {
    fn get_display_name(&self) -> &'static str {
        "virtio-gpu"
    }

    fn get_framebuffer_config(&self) -> Result<FramebufferConfig, &'static str> {
        self.core.lock().get_framebuffer_config()
    }

    fn get_framebuffer_address(&self) -> Result<usize, &'static str> {
        self.core.lock().get_framebuffer_address()
    }

    fn get_framebuffer_info(&self) -> Result<(FramebufferConfig, usize), &'static str> {
        let core = self.core.lock();
        let _ = core.poll_display_resize();
        let config = core.get_framebuffer_config()?;
        let physical_addr = core.get_framebuffer_address()?;
        Ok((config, physical_addr))
    }

    fn present_framebuffer_region(
        &self,
        config: &FramebufferConfig,
        physical_addr: usize,
        region: DisplayRegion,
    ) -> Result<(), &'static str> {
        let core = self.core.lock();
        let current_config = core.get_framebuffer_config()?;
        let current_physical_addr = core.get_framebuffer_address()?;
        if current_physical_addr != physical_addr {
            return Err(
                "Framebuffer physical address does not match current virtio-gpu framebuffer",
            );
        }
        if current_config.width != config.width
            || current_config.height != config.height
            || current_config.format != config.format
            || current_config.stride != config.stride
        {
            return Err("Framebuffer config does not match current virtio-gpu framebuffer");
        }

        core.present_framebuffer_region(region)
    }

    fn present_gpu_resource_region(
        &self,
        resource: GpuDisplayResource,
        region: DisplayRegion,
    ) -> Result<(), &'static str> {
        self.core
            .lock()
            .present_gpu_resource_region(resource, region)
    }

    fn init_graphics(&self) -> Result<(), &'static str> {
        {
            let core = self.core.lock();
            let mut initialized = core.initialized.lock();
            if *initialized {
                crate::early_println!("[virtio-gpu] init_graphics: already initialized");
                return Ok(());
            }
            *initialized = true;
        }

        crate::early_println!("[virtio-gpu] init_graphics: get_display_info");

        // Get display information
        self.core.lock().get_display_info_internal()?;

        crate::early_println!("[virtio-gpu] init_graphics: setup_framebuffer");

        // Set up framebuffer
        self.core.lock().setup_framebuffer()?;

        crate::early_println!("[virtio-gpu] init_graphics: done");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn virtio_gpu_cursor_queue_fits_qemu_limit() {
        assert_eq!(VIRTIO_GPU_CURSOR_QUEUE_SIZE, 16);
    }

    #[test_case]
    fn virgl_image_bind_flags_follow_generic_usage() {
        assert_eq!(
            virgl_image_bind(GPU_IMAGE_USAGE_SAMPLED),
            PIPE_BIND_SAMPLER_VIEW
        );
        assert_eq!(
            virgl_image_bind(
                GPU_IMAGE_USAGE_RENDER_TARGET
                    | GPU_IMAGE_USAGE_PRESENTABLE
                    | GPU_IMAGE_USAGE_SAMPLED,
            ),
            PIPE_BIND_RENDER_TARGET | PIPE_BIND_SCANOUT | PIPE_BIND_SAMPLER_VIEW
        );
        assert_eq!(
            virgl_image_bind(GPU_IMAGE_USAGE_DEPTH_STENCIL_ATTACHMENT),
            PIPE_BIND_DEPTH_STENCIL
        );
    }

    #[test_case]
    fn virgl_submit_rejection_is_not_device_loss() {
        let response = VirtioGpuCtrlHdr {
            hdr_type: VIRTIO_GPU_RESP_ERR_INVALID_PARAMETER,
            flags: 0,
            fence_id: 0,
            ctx_id: 0,
            padding: 0,
        };
        assert_eq!(
            validate_submit_response(response, None),
            Err(GpuBackendSubmitError::Rejected(
                "VirtIO GPU rejected the execution command stream",
            ))
        );
        assert_eq!(
            classify_submit_transport_error("VirtIO GPU control queue timed out"),
            GpuBackendSubmitError::DeviceLost("VirtIO GPU control queue timed out")
        );
    }

    /// Physical address of the VirtIO GPU device on QEMU RISC-V virt.
    const VIRTIO_GPU_PADDR: usize = 0x10002000;

    /// Map the VirtIO GPU MMIO region for use in tests.
    /// Returns the virtual address to pass to `VirtioGpuDevice::new`.
    fn map_gpu() -> usize {
        crate::vm::ioremap(VIRTIO_GPU_PADDR, crate::environment::PAGE_SIZE)
            .expect("ioremap should succeed for VirtIO GPU test device")
    }

    #[test_case]
    fn test_virtio_gpu_device_creation() {
        let vaddr = map_gpu();
        let device = VirtioGpuDevice::new(vaddr);
        // The stored base address is the virtual (ioremap'd) address, not the physical one.
        assert_eq!(device.core.lock().get_base_addr(), vaddr);
        assert_eq!(device.core.lock().get_virtqueue_count(), 2);
        assert_eq!(device.device_type(), DeviceType::Graphics);
        assert_eq!(device.name(), "virtio-gpu");
        assert_eq!(device.core.lock().get_display_name(), "virtio-gpu");
        crate::vm::iounmap(vaddr);
    }

    #[test_case]
    fn test_virtio_gpu_resource_id_generation() {
        let vaddr = map_gpu();
        let device = VirtioGpuDevice::new(vaddr);
        assert_eq!(device.core.lock().next_resource_id(), 1);
        assert_eq!(device.core.lock().next_resource_id(), 2);
        assert_eq!(device.core.lock().next_resource_id(), 3);
        crate::vm::iounmap(vaddr);
    }

    #[test_case]
    fn test_virtio_gpu_before_init() {
        let vaddr = map_gpu();
        let device = VirtioGpuDevice::new(vaddr);
        // Should fail before explicit graphics initialization
        assert!(device.get_framebuffer_config().is_err());
        assert!(device.get_framebuffer_address().is_err());
        crate::vm::iounmap(vaddr);
    }

    #[cfg(target_arch = "riscv64")]
    #[test_case]
    fn test_virtio_gpu_init_graphics() {
        let vaddr = map_gpu();
        let mut device = VirtioGpuDevice::new(vaddr);
        device.init_graphics().unwrap();
    }

    #[cfg(target_arch = "riscv64")]
    #[test_case]
    fn test_virtio_gpu_framebuffer_operations() {
        let vaddr = map_gpu();
        let mut device = VirtioGpuDevice::new(vaddr);

        // Initialize the device
        device.init_graphics().unwrap();

        // Get framebuffer configuration
        let config = device.get_framebuffer_config().unwrap();
        assert_ne!(config.width, 0);
        assert_ne!(config.height, 0);
        assert_eq!(config.format, PixelFormat::BGRA8888);
        assert_eq!(config.stride, config.width * 4);
        assert_eq!(
            config.size(),
            config.width as usize * config.height as usize * 4
        );

        // Get framebuffer address
        let fb_addr = device.get_framebuffer_address().unwrap();
        assert_ne!(fb_addr, 0);

        // Write some test pattern to framebuffer
        unsafe {
            let fb_ptr = crate::vm::addr::phys_to_virt(fb_addr) as *mut u32;
            let pixel_count = (config.width * config.height) as usize;

            // Fill with a gradient pattern
            for y in 0..config.height {
                for x in 0..config.width {
                    let pixel_index = (y * config.width + x) as usize;
                    if pixel_index < pixel_count {
                        // Create a simple gradient: red increasing with x, blue with y
                        let red = if config.width > 1 {
                            (x * 255) / (config.width - 1)
                        } else {
                            0
                        };
                        let blue = if config.height > 1 {
                            (y * 255) / (config.height - 1)
                        } else {
                            0
                        };
                        let green = 0x80; // Fixed green component
                        let alpha = 0xFF; // Fully opaque

                        // BGRA format: Blue | Green | Red | Alpha
                        let pixel = (alpha << 24) | (red << 16) | (green << 8) | blue;
                        *fb_ptr.add(pixel_index) = pixel;
                    }
                }
            }
        }

        // Present the entire framebuffer
        device
            .present_current_framebuffer_region(DisplayRegion::full(&config))
            .unwrap();

        // Verify some pixels were written correctly
        unsafe {
            let fb_ptr = crate::vm::addr::phys_to_virt(fb_addr) as *mut u32;

            // Check top-left corner (should be mostly blue)
            let top_left = *fb_ptr;
            assert_eq!((top_left >> 24) & 0xFF, 0xFF); // Alpha
            assert_eq!((top_left >> 16) & 0xFF, 0x00); // Red (should be 0 at x=0)
            assert_eq!((top_left >> 8) & 0xFF, 0x80); // Green
            assert_eq!(top_left & 0xFF, 0x00); // Blue (should be 0 at y=0)

            // Check bottom-right corner
            let bottom_right_index =
                ((config.height - 1) * config.width + (config.width - 1)) as usize;
            let bottom_right = *fb_ptr.add(bottom_right_index);
            assert_eq!((bottom_right >> 24) & 0xFF, 0xFF); // Alpha
            assert_eq!((bottom_right >> 16) & 0xFF, 0xFF); // Red (should be max at x=width-1)
            assert_eq!((bottom_right >> 8) & 0xFF, 0x80); // Green
            assert_eq!(bottom_right & 0xFF, 0xFF); // Blue (should be max at y=height-1)
        }
    }

    #[cfg(target_arch = "riscv64")]
    #[test_case]
    fn test_virtio_gpu_pixel_drawing() {
        let vaddr = map_gpu();
        let mut device = VirtioGpuDevice::new(vaddr);
        device.init_graphics().unwrap();

        let config = device.get_framebuffer_config().unwrap();
        let fb_addr = device.get_framebuffer_address().unwrap();

        // Helper function to set a pixel
        let set_pixel = |x: u32, y: u32, color: u32| {
            if x < config.width && y < config.height {
                unsafe {
                    let fb_ptr = crate::vm::addr::phys_to_virt(fb_addr) as *mut u32;
                    let pixel_index = (y * config.width + x) as usize;
                    *fb_ptr.add(pixel_index) = color;
                }
            }
        };

        // Draw a simple test pattern
        // Red horizontal line at y=100
        for x in 0..config.width {
            set_pixel(x, 100, 0xFF0000FF); // Red in BGRA format
        }

        // Green vertical line at x=200
        for y in 0..config.height {
            set_pixel(200, y, 0xFF00FF00); // Green in BGRA format
        }

        // Blue diagonal line
        let min_dim = config.width.min(config.height);
        for i in 0..min_dim {
            set_pixel(i, i, 0xFFFF0000); // Blue in BGRA format
        }

        // Present the changes
        device
            .present_current_framebuffer_region(DisplayRegion::full(&config))
            .unwrap();

        // Verify some of the drawn pixels
        unsafe {
            let fb_ptr = crate::vm::addr::phys_to_virt(fb_addr) as *mut u32;

            // Check red line
            let red_pixel_index = (100 * config.width + 50) as usize;
            let red_pixel = *fb_ptr.add(red_pixel_index);
            assert_eq!(red_pixel, 0xFF0000FF);

            // Check green line
            let green_pixel_index = (50 * config.width + 200) as usize;
            let green_pixel = *fb_ptr.add(green_pixel_index);
            assert_eq!(green_pixel, 0xFF00FF00);

            // Check blue diagonal
            let blue_pixel_index = (100 * config.width + 100) as usize;
            let blue_pixel = *fb_ptr.add(blue_pixel_index);
            assert_eq!(blue_pixel, 0xFFFF0000);
        }
    }

    #[cfg(target_arch = "riscv64")]
    #[test_case]
    fn test_virtio_gpu_rectangle_drawing() {
        let vaddr = map_gpu();
        let mut device = VirtioGpuDevice::new(vaddr);
        device.init_graphics().unwrap();

        let config = device.get_framebuffer_config().unwrap();
        let fb_addr = device.get_framebuffer_address().unwrap();

        // Helper function to draw a filled rectangle
        let draw_rectangle = |x: u32, y: u32, width: u32, height: u32, color: u32| {
            for dy in 0..height {
                for dx in 0..width {
                    let px = x + dx;
                    let py = y + dy;
                    if px < config.width && py < config.height {
                        unsafe {
                            let fb_ptr = crate::vm::addr::phys_to_virt(fb_addr) as *mut u32;
                            let pixel_index = (py * config.width + px) as usize;
                            *fb_ptr.add(pixel_index) = color;
                        }
                    }
                }
            }
        };

        // Clear framebuffer with black
        unsafe {
            let fb_ptr = crate::vm::addr::phys_to_virt(fb_addr) as *mut u32;
            let pixel_count = (config.width * config.height) as usize;
            for i in 0..pixel_count {
                *fb_ptr.add(i) = 0xFF000000; // Black with full alpha
            }
        }

        // Draw some rectangles
        draw_rectangle(50, 50, 100, 75, 0xFF0000FF); // Red rectangle
        draw_rectangle(200, 100, 150, 100, 0xFF00FF00); // Green rectangle
        draw_rectangle(400, 200, 80, 120, 0xFFFF0000); // Blue rectangle

        // Present changes
        device
            .present_current_framebuffer_region(DisplayRegion::full(&config))
            .unwrap();

        // Verify the rectangles were drawn correctly
        unsafe {
            let fb_ptr = crate::vm::addr::phys_to_virt(fb_addr) as *mut u32;

            // Check red rectangle center
            let red_center_index = ((50 + 37) * config.width + (50 + 50)) as usize;
            let red_pixel = *fb_ptr.add(red_center_index);
            assert_eq!(red_pixel, 0xFF0000FF);

            // Check green rectangle center
            let green_center_index = ((100 + 50) * config.width + (200 + 75)) as usize;
            let green_pixel = *fb_ptr.add(green_center_index);
            assert_eq!(green_pixel, 0xFF00FF00);

            // Check blue rectangle center
            let blue_center_index = ((200 + 60) * config.width + (400 + 40)) as usize;
            let blue_pixel = *fb_ptr.add(blue_center_index);
            assert_eq!(blue_pixel, 0xFFFF0000);

            // Check that area outside rectangles is still black
            let background_index = (10 * config.width + 10) as usize;
            let background_pixel = *fb_ptr.add(background_index);
            assert_eq!(background_pixel, 0xFF000000);
        }
    }

    #[cfg(target_arch = "riscv64")]
    #[test_case]
    fn test_virtio_gpu_border_drawing() {
        let vaddr = map_gpu();
        let mut device = VirtioGpuDevice::new(vaddr);
        device.init_graphics().unwrap();

        let config = device.get_framebuffer_config().unwrap();
        let fb_addr = device.get_framebuffer_address().unwrap();

        // Helper function to draw a rectangle border
        let draw_border = |x: u32, y: u32, width: u32, height: u32, color: u32| {
            // Top and bottom edges
            for dx in 0..width {
                let px = x + dx;
                if px < config.width {
                    // Top edge
                    if y < config.height {
                        unsafe {
                            let fb_ptr = crate::vm::addr::phys_to_virt(fb_addr) as *mut u32;
                            let pixel_index = (y * config.width + px) as usize;
                            *fb_ptr.add(pixel_index) = color;
                        }
                    }
                    // Bottom edge
                    let bottom_y = y + height - 1;
                    if bottom_y < config.height {
                        unsafe {
                            let fb_ptr = crate::vm::addr::phys_to_virt(fb_addr) as *mut u32;
                            let pixel_index = (bottom_y * config.width + px) as usize;
                            *fb_ptr.add(pixel_index) = color;
                        }
                    }
                }
            }

            // Left and right edges
            for dy in 0..height {
                let py = y + dy;
                if py < config.height {
                    // Left edge
                    if x < config.width {
                        unsafe {
                            let fb_ptr = crate::vm::addr::phys_to_virt(fb_addr) as *mut u32;
                            let pixel_index = (py * config.width + x) as usize;
                            *fb_ptr.add(pixel_index) = color;
                        }
                    }
                    // Right edge
                    let right_x = x + width - 1;
                    if right_x < config.width {
                        unsafe {
                            let fb_ptr = crate::vm::addr::phys_to_virt(fb_addr) as *mut u32;
                            let pixel_index = (py * config.width + right_x) as usize;
                            *fb_ptr.add(pixel_index) = color;
                        }
                    }
                }
            }
        };

        // Clear framebuffer
        unsafe {
            let fb_ptr = crate::vm::addr::phys_to_virt(fb_addr) as *mut u32;
            let pixel_count = (config.width * config.height) as usize;
            for i in 0..pixel_count {
                *fb_ptr.add(i) = 0xFF000000; // Black
            }
        }

        // Draw nested borders
        draw_border(10, 10, 200, 150, 0xFF0000FF); // Red outer border
        draw_border(20, 20, 180, 130, 0xFF00FF00); // Green middle border
        draw_border(30, 30, 160, 110, 0xFFFF0000); // Blue inner border

        device
            .present_current_framebuffer_region(DisplayRegion::full(&config))
            .unwrap();

        // Verify borders
        unsafe {
            let fb_ptr = crate::vm::addr::phys_to_virt(fb_addr) as *mut u32;

            // Check red border corners
            let top_left_red = *fb_ptr.add((10 * config.width + 10) as usize);
            assert_eq!(top_left_red, 0xFF0000FF);

            let top_right_red = *fb_ptr.add((10 * config.width + 209) as usize);
            assert_eq!(top_right_red, 0xFF0000FF);

            // Check green border
            let green_border = *fb_ptr.add((20 * config.width + 20) as usize);
            assert_eq!(green_border, 0xFF00FF00);

            // Check blue border
            let blue_border = *fb_ptr.add((30 * config.width + 30) as usize);
            assert_eq!(blue_border, 0xFFFF0000);

            // Check inside area is still black
            let inside = *fb_ptr.add((50 * config.width + 50) as usize);
            assert_eq!(inside, 0xFF000000);
        }
    }

    #[cfg(target_arch = "riscv64")]
    #[test_case]
    fn test_virtio_gpu_pixel_format_verification() {
        let vaddr = map_gpu();
        let mut device = VirtioGpuDevice::new(vaddr);
        device.init_graphics().unwrap();

        let config = device.get_framebuffer_config().unwrap();
        let fb_addr = device.get_framebuffer_address().unwrap();

        // Test various pixel format interpretations
        unsafe {
            let fb_ptr = crate::vm::addr::phys_to_virt(fb_addr) as *mut u32;

            // Test pure colors in BGRA format
            let test_colors = [
                (0xFF0000FF, "red"),   // Red in BGRA: A=FF, R=00, G=00, B=FF
                (0xFF00FF00, "green"), // Green in BGRA: A=FF, R=00, G=FF, B=00
                (0xFFFF0000, "blue"),  // Blue in BGRA: A=FF, R=FF, G=00, B=00
                (0xFFFFFFFF, "white"), // White in BGRA: A=FF, R=FF, G=FF, B=FF
                (0xFF000000, "black"), // Black in BGRA: A=FF, R=00, G=00, B=00
                (0xFF808080, "gray"),  // Gray in BGRA: A=FF, R=80, G=80, B=80
            ];

            // Write test pattern
            for (i, (color, _name)) in test_colors.iter().enumerate() {
                let x = (i as u32 * 100) % config.width;
                let y = (i as u32 * 100) / config.width;
                if y < config.height {
                    let pixel_index = (y * config.width + x) as usize;
                    *fb_ptr.add(pixel_index) = *color;
                }
            }

            device
                .present_current_framebuffer_region(DisplayRegion::full(&config))
                .unwrap();

            // Verify the colors were written correctly
            for (i, (expected_color, _name)) in test_colors.iter().enumerate() {
                let x = (i as u32 * 100) % config.width;
                let y = (i as u32 * 100) / config.width;
                if y < config.height {
                    let pixel_index = (y * config.width + x) as usize;
                    let actual_color = *fb_ptr.add(pixel_index);
                    assert_eq!(actual_color, *expected_color);
                }
            }
        }

        // Test partial transparency (though VirtIO GPU might not support it fully)
        unsafe {
            let fb_ptr = crate::vm::addr::phys_to_virt(fb_addr) as *mut u32;
            let semi_transparent_red = 0x800000FF; // 50% transparent red
            let pixel_index = (100 * config.width + 100) as usize;
            *fb_ptr.add(pixel_index) = semi_transparent_red;

            device
                .present_current_framebuffer_region(DisplayRegion::new(100, 100, 1, 1))
                .unwrap();

            let written_pixel = *fb_ptr.add(pixel_index);
            assert_eq!(written_pixel, semi_transparent_red);
        }
    }

    #[cfg(target_arch = "riscv64")]
    #[test_case]
    fn test_virtio_gpu_command_flow_verification() {
        let vaddr = map_gpu();
        let mut device = VirtioGpuDevice::new(vaddr);

        // Test device initialization and command flow
        crate::early_println!("[Test] Starting VirtIO GPU command flow verification");
        device.init_graphics().unwrap();

        let config = device.get_framebuffer_config().unwrap();
        let fb_addr = device.get_framebuffer_address().unwrap();

        crate::early_println!(
            "[Test] Framebuffer initialized at {:#x}, config: {}x{}",
            fb_addr,
            config.width,
            config.height
        );

        // Write a test pattern and verify the present path
        unsafe {
            let fb_ptr = crate::vm::addr::phys_to_virt(fb_addr) as *mut u32;

            // Write a simple checkerboard pattern
            for y in 0..config.height.min(100) {
                for x in 0..config.width.min(100) {
                    let pixel_index = (y * config.width + x) as usize;
                    let color = if (x / 10 + y / 10) % 2 == 0 {
                        0xFFFF0000 // Blue squares
                    } else {
                        0xFF00FF00 // Green squares
                    };
                    *fb_ptr.add(pixel_index) = color;
                }
            }
        }

        crate::early_println!("[Test] Written checkerboard pattern to framebuffer");

        // Test presenting different regions
        device
            .present_current_framebuffer_region(DisplayRegion::new(0, 0, 50, 50))
            .unwrap();
        device
            .present_current_framebuffer_region(DisplayRegion::new(50, 50, 50, 50))
            .unwrap();
        device
            .present_current_framebuffer_region(DisplayRegion::full(&config))
            .unwrap();

        crate::early_println!("[Test] VirtIO GPU command flow verification completed");
    }

    #[cfg(target_arch = "riscv64")]
    #[test_case]
    fn test_virtio_gpu_resource_management() {
        let vaddr = map_gpu();
        let mut device = VirtioGpuDevice::new(vaddr);
        device.init_graphics().unwrap();

        // Test that resource IDs are managed correctly
        let config = device.get_framebuffer_config().unwrap();
        let fb_addr = device.get_framebuffer_address().unwrap();

        crate::early_println!("[Test] Testing VirtIO GPU resource management");
        crate::early_println!("[Test] Primary framebuffer resource should be ID 1");

        // The framebuffer should be associated with resource ID 1
        // (as set up in setup_framebuffer)

        // Write some data and present to verify resource association
        unsafe {
            let fb_ptr = crate::vm::addr::phys_to_virt(fb_addr) as *mut u32;
            // Write a diagonal line pattern
            for i in 0..config.width.min(config.height).min(500) {
                let pixel_index = (i * config.width + i) as usize;
                *fb_ptr.add(pixel_index) = 0xFFFFFF00; // Yellow diagonal
            }
        }

        // Present the diagonal region
        device
            .present_current_framebuffer_region(DisplayRegion::new(
                0,
                0,
                config.width.min(500),
                config.height.min(500),
            ))
            .unwrap();

        crate::early_println!("[Test] Resource management test completed");
    }
}
