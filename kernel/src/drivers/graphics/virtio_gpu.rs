//! # VirtIO GPU Device Driver
//!
//! This module provides a driver for VirtIO GPU devices, implementing the
//! GraphicsDevice trait for integration with the kernel's graphics subsystem.
//!
//! The driver supports basic framebuffer operations and display management
//! according to the VirtIO GPU specification.

use crate::sync::{IrqRwSpinLock, IrqSpinLock};
use alloc::{
    boxed::Box,
    sync::{Arc, Weak},
    vec::Vec,
};

use crate::{
    device::{
        Device, DeviceType,
        gpu::{
            GPU_DIALECT_INFO_BYTES, GPU_EXECUTION_SUPPORT_DEPTH, GPU_EXECUTION_SUPPORT_IMAGE_MIPS,
            GPU_EXECUTION_SUPPORT_IMAGE_READBACK, GPU_EXECUTION_SUPPORT_IMAGE_UPLOAD,
            GPU_EXECUTION_SUPPORT_MEMORY, GPU_EXECUTION_SUPPORT_PRESENTATION,
            GPU_EXECUTION_SUPPORT_QUEUE, GPU_EXECUTION_SUPPORT_TIMELINE,
            GPU_IMAGE_FORMAT_BGRA8_UNORM, GPU_IMAGE_FORMAT_DEPTH32_FLOAT,
            GPU_IMAGE_USAGE_DEPTH_STENCIL_ATTACHMENT, GPU_IMAGE_USAGE_PRESENTABLE,
            GPU_IMAGE_USAGE_RENDER_TARGET, GPU_IMAGE_USAGE_SAMPLED, GPU_IMAGE_USAGE_TRANSFER_DST,
            GPU_IMAGE_USAGE_TRANSFER_SRC, GpuBackend, GpuBackendBuffer, GpuBackendBufferInfo,
            GpuBackendContext, GpuBackendContextInfo, GpuBackendDialectDescriptor,
            GpuBackendDialectInfo, GpuBackendEnqueueError, GpuBackendImage, GpuBackendImageInfo,
            GpuBackendInfo, GpuBackendQueue, GpuBackendQueueInfo, GpuBackendSubmitError,
            GpuBufferCreateInfo, GpuDeviceInfo, GpuDeviceState, GpuImageBackingInfo,
            GpuImageCreateInfo, GpuImageUploadInfo, GpuSubmission,
        },
        graphics::{
            FramebufferConfig, GpuDisplayResource, GpuPresentOptions, GraphicsDevice, PixelFormat,
            PresentationOwners, PresentationWait, output::DisplayRegion,
        },
    },
    drivers::virtio::{device::VirtioDevice, pci::VirtioPciTransport, queue::VirtQueue},
    mem::page::ContiguousPages,
    object::capability::{ControlOps, MemoryMappingOps, Selectable},
    vm::addr::virt_to_phys,
};
use core::ptr;
use core::sync::atomic::{AtomicBool, Ordering};

mod asynchronous;
mod control;
mod core_lock;
mod synchronous;

use core_lock::CoreLock;

use control::{ControlEnqueueError, ControlQueue, ControlRequest};

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
const VIRTIO_GPU_CONFIG_EVENTS_READ_OFFSET: usize = 0;
const VIRTIO_GPU_CONFIG_EVENTS_CLEAR_OFFSET: usize = 4;
const VIRTIO_GPU_EVENT_DISPLAY: u32 = 1;
const VIRTIO_GPU_CONFIG_NUM_CAPSETS_OFFSET: usize = 12;
const VIRTIO_GPU_CONTROL_QUEUE_SIZE: usize = 64;
const VIRTIO_GPU_CURSOR_QUEUE_SIZE: usize = 16;
// A bounded batch can contain several 2 MiB VirGL submissions. QEMU may spend
// seconds compiling shaders or uploading them before returning the last fence.
const VIRTIO_GPU_CONTROL_TIMEOUT_NS: u64 = 10_000_000_000;
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
// VirGL protocol values differ from Gallium PIPE_BIND_* at this bit.
const VIRGL_BIND_CUSTOM: u32 = 1 << 17;
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
    paddr: u64,
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
    // VirGL interprets bind=0 as buffer storage and rejects PIPE_TEXTURE_2D.
    // Transfer-only images still need a physical texture binding. This does
    // not add SAMPLED authority to the generic image's advertised usage.
    if bind == 0 {
        bind = PIPE_BIND_SAMPLER_VIEW;
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

struct VirtioGpuQueues {
    control: ControlQueue,
    cursor: VirtQueue<'static>,
}

impl VirtioGpuQueues {
    fn new() -> Self {
        let mut cursor = VirtQueue::new(VIRTIO_GPU_CURSOR_QUEUE_SIZE);
        cursor.init();
        Self {
            control: ControlQueue::new(),
            cursor,
        }
    }

    fn queue(&self, index: usize) -> &VirtQueue<'static> {
        match index {
            0 => &self.control.ring,
            1 => &self.cursor,
            _ => panic!("Invalid VirtIO GPU queue index"),
        }
    }
}

/// Mutable submission and presentation state, held only for bounded admission.
pub struct VirtioGpuDeviceCore {
    control: Arc<VirtioGpuControl>,
    async_submissions: asynchronous::AsyncSubmissions,
    async_enabled: bool,
    next_present_id: u64,
    presentations: Vec<PresentOperation>,
}

impl core::ops::Deref for VirtioGpuDeviceCore {
    type Target = VirtioGpuControl;
    fn deref(&self) -> &Self::Target {
        &self.control
    }
}

/// Shared transport. Synchronous callers never retain the submission-state
/// guard while waiting for a host response. Each field has its own short guard.
pub struct VirtioGpuControl {
    base_addr: usize,
    pci_transport: Option<VirtioPciTransport>,
    interrupt_state: Arc<asynchronous::InterruptState>,
    virtqueues: IrqSpinLock<VirtioGpuQueues>,
    synchronous: synchronous::Operations,
    display_info: IrqRwSpinLock<Option<VirtioGpuRespDisplayInfo>>,
    display_update_pending: AtomicBool,
    framebuffer_addr: IrqRwSpinLock<Option<u64>>,
    framebuffer_alloc: IrqRwSpinLock<Option<ContiguousPages>>,
    retired_framebuffer_allocs: IrqSpinLock<Vec<ContiguousPages>>,
    resource_id: IrqSpinLock<u32>,
    acceleration_context_id: IrqSpinLock<u32>,
    acceleration_fence_id: IrqSpinLock<u64>,
    current_resource_id: IrqRwSpinLock<Option<u32>>,
    scanout_resource_id: IrqRwSpinLock<Option<u32>>,
    negotiated_features: IrqRwSpinLock<u64>,
    transport_ready: IrqRwSpinLock<bool>,
    initialized: IrqSpinLock<bool>,
    // Track resources and their associated memory
    resources: IrqSpinLock<alloc::collections::BTreeMap<u32, (u64, usize)>>, // resource_id -> (addr, size)
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
        Self {
            control: Arc::new(VirtioGpuControl::new_with_transport(
                base_addr,
                pci_transport,
            )),
            async_submissions: asynchronous::AsyncSubmissions::default(),
            async_enabled: false,
            next_present_id: 0,
            presentations: Vec::new(),
        }
    }
}

impl VirtioGpuControl {
    fn new_with_transport(base_addr: usize, pci_transport: Option<VirtioPciTransport>) -> Self {
        let mut device = Self {
            base_addr,
            pci_transport,
            interrupt_state: Arc::new(asynchronous::InterruptState::new(base_addr, pci_transport)),
            virtqueues: IrqSpinLock::new(VirtioGpuQueues::new()),
            synchronous: synchronous::Operations::new(),
            display_info: IrqRwSpinLock::new(None),
            display_update_pending: AtomicBool::new(false),
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
            initialized: IrqSpinLock::new(false),
            resources: IrqSpinLock::new(alloc::collections::BTreeMap::new()),
        };

        // init may install a ring even if later negotiation fails. Without a
        // reset proof its storage must outlive the device's configuration.
        device.virtqueues.lock().control.mark_device_owned();

        // Initialize the VirtIO device - this will set up the queues with the device
        match device.init() {
            Ok(features) => {
                device.virtqueues.lock().control.set_event_idx(
                    features & (1u64 << crate::drivers::virtio::features::VIRTIO_RING_F_EVENT_IDX)
                        != 0,
                );
                *device.negotiated_features.write() = features;
                *device.transport_ready.write() = true;
            }
            Err(_) => {
                crate::println!("[Virtio GPU] Warning: Failed to initialize VirtIO device");
            }
        }

        // crate::println!("[Virtio GPU] Device created and initialized at {:#x}", base_addr);
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
        // The negotiated legacy VirGL fence callback carries only 32 bits.
        // Never wrap/truncate into a fence that could certify unrelated work.
        if current > u64::from(u32::MAX) {
            return Err("VirtIO GPU acceleration fence IDs are exhausted");
        }
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

    /// Set up a new resource without retiring unrelated GPU work. Callers
    /// create a fresh object, attach its initial backing, or add a context
    /// attachment. Destruction, transfers and backing replacement retain the
    /// execution barrier in send_execution_command/send_execution_bytes.
    fn send_resource_setup<T>(&self, command: &T) -> Result<(), &'static str> {
        // SAFETY: callers pass initialized repr(C) VirtIO commands.
        let bytes = unsafe {
            core::slice::from_raw_parts(command as *const T as *const u8, core::mem::size_of::<T>())
        };
        self.send_resource_setup_bytes(bytes)
    }

    fn send_resource_setup_bytes(&self, bytes: &[u8]) -> Result<(), &'static str> {
        let mut response = [0; core::mem::size_of::<VirtioGpuCtrlHdr>()];
        self.send_control_request(bytes, &mut response)?;
        // SAFETY: the completed request supplied a complete, possibly unaligned header.
        let response =
            unsafe { core::ptr::read_unaligned(response.as_ptr() as *const VirtioGpuCtrlHdr) };
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
        // Legacy operations remain synchronous and ordered after all preceding
        // control requests. Reaping only returns transport ownership; execution
        // owners validate fences and release GPU resources outside driver locks.
        self.synchronous
            .execute(self, cmd_buffer, resp_buffer, true)
    }

    fn send_control_request(
        &self,
        cmd_buffer: &[u8],
        resp_buffer: &mut [u8],
    ) -> Result<(), &'static str> {
        self.synchronous
            .execute(self, cmd_buffer, resp_buffer, false)
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
        // Preserve the original 12-byte prefix. Optional words expose the host
        // anisotropy limit (IEEE f32 bits), then kernel transfer capabilities.
        let mut opaque_info = [0u8; 20];
        opaque_info[..8].copy_from_slice(&negotiated_features.to_le_bytes());
        let acceleration_usable = self.acceleration_usable();
        let dialect_count = if acceleration_usable {
            self.read_config_u32(VIRTIO_GPU_CONFIG_NUM_CAPSETS_OFFSET)
        } else {
            0
        };
        opaque_info[8..12].copy_from_slice(&dialect_count.to_le_bytes());
        if acceleration_usable {
            let (anisotropy, float_color) = self.virgl_color_metadata(dialect_count);
            opaque_info[12..16].copy_from_slice(&anisotropy.to_le_bytes());
            // Flag 0: D32 transfers. Flag 1: RGBA16F/RGBA32F sampled render targets.
            opaque_info[16..20]
                .copy_from_slice(&(1u32 | (u32::from(float_color) << 1)).to_le_bytes());
        }

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
                            | GPU_EXECUTION_SUPPORT_IMAGE_MIPS
                            | crate::device::gpu::GPU_EXECUTION_SUPPORT_TEXTURE_ARRAYS
                            | crate::device::gpu::GPU_EXECUTION_SUPPORT_DEPTH_SAMPLING
                            | crate::device::gpu::GPU_EXECUTION_SUPPORT_NATIVE_NARROW_COLOR
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

    fn virgl_color_metadata(&self, count: u32) -> (u32, bool) {
        // Stable wire offset in virgl_caps_v2, not a native Rust/C layout.
        // Only transport opaque host metadata; filtering policy stays in userspace.
        const END: usize = 764;
        for index in 0..count.min(32) {
            let Ok(info) = self.get_backend_dialect_info(index) else {
                continue;
            };
            if info.id != 2 || info.max_size < END as u32 {
                continue;
            }
            let mut caps = [0u8; END];
            if self
                .read_backend_dialect(info.id, info.max_version, &mut caps)
                .is_ok()
            {
                let bits = u32::from_le_bytes(caps[760..END].try_into().unwrap());
                // Positive, finite f32; compare bits without kernel FP operations.
                let anisotropy = if (0x3f80_0000..0x7f80_0000).contains(&bits) {
                    bits
                } else {
                    0
                };
                let supported = |mask: usize, format: usize| {
                    let offset = mask + format / 32 * 4;
                    u32::from_le_bytes(caps[offset..offset + 4].try_into().unwrap())
                        & (1 << (format % 32))
                        != 0
                };
                // Stable v1 sampler/render masks, each sixteen u32 words.
                let float_color = [31, 94]
                    .into_iter()
                    .all(|f| supported(4, f) && supported(68, f));
                return (anisotropy, float_color);
            }
        }
        (0, false)
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
        self.send_resource_setup(&cmd)
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
        self.send_resource_setup(&cmd)?;
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
        self.send_resource_setup(&cmd)
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

    /// Attach initial backing memory to a newly created, unused resource.
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

        self.send_resource_setup_bytes(&cmd_buffer)
    }

    /// Attach one contiguous backing memory range to a resource.
    fn attach_backing_to_resource(
        &self,
        resource_id: u32,
        addr: u64,
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
        if !self.display_update_needed() {
            return Ok(());
        }
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
                crate::println!(
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

        self.display_update_pending.store(false, Ordering::Release);
        Ok(())
    }

    fn display_update_needed(&self) -> bool {
        let events = self.read_config_u32(VIRTIO_GPU_CONFIG_EVENTS_READ_OFFSET);
        if events & VIRTIO_GPU_EVENT_DISPLAY != 0 {
            // Clear before querying: a resize that arrives during the query
            // remains set for the next snapshot. Retain a local retry bit if
            // GET_DISPLAY_INFO or framebuffer replacement fails after the ack.
            self.display_update_pending.store(true, Ordering::Release);
            self.write_config(
                VIRTIO_GPU_CONFIG_EVENTS_CLEAR_OFFSET,
                VIRTIO_GPU_EVENT_DISPLAY,
            );
            crate::arch::io_mb();
        }
        self.display_update_pending.load(Ordering::Acquire)
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

    fn get_framebuffer_address(&self) -> Result<u64, &'static str> {
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
}

impl VirtioDevice for VirtioGpuControl {
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
        virtqueues.queue(queue_idx).get_queue_size()
    }

    fn get_queue_desc_addr(&self, queue_idx: usize) -> Option<u64> {
        if queue_idx >= self.get_virtqueue_count() {
            return None;
        }

        let virtqueues = self.virtqueues.lock();
        Some(virt_to_phys(virtqueues.queue(queue_idx).desc.as_ptr() as usize) as u64)
    }

    fn get_queue_driver_addr(&self, queue_idx: usize) -> Option<u64> {
        if queue_idx >= self.get_virtqueue_count() {
            return None;
        }

        let virtqueues = self.virtqueues.lock();
        Some(virt_to_phys(virtqueues.queue(queue_idx).avail.flags as *const u16 as usize) as u64)
    }

    fn get_queue_device_addr(&self, queue_idx: usize) -> Option<u64> {
        if queue_idx >= self.get_virtqueue_count() {
            return None;
        }

        let virtqueues = self.virtqueues.lock();
        Some(virt_to_phys(virtqueues.queue(queue_idx).used.flags as *const u16 as usize) as u64)
    }

    fn get_supported_features(&self, device_features: u64) -> u64 {
        let mut supported = (1u64 << VIRTIO_GPU_F_VIRGL)
            | (1u64 << VIRTIO_GPU_F_EDID)
            | (1u64 << VIRTIO_GPU_F_CONTEXT_INIT)
            | (1u64 << crate::drivers::virtio::features::VIRTIO_RING_F_EVENT_IDX);
        if self.pci_transport().is_some() {
            supported |= 1u64 << crate::drivers::virtio::features::VIRTIO_F_VERSION_1;
        }
        device_features & supported
    }
}

pub struct VirtioGpuDevice {
    core: Arc<CoreLock<VirtioGpuDeviceCore>>,
    interrupt_state: Arc<asynchronous::InterruptState>,
}

/// DMA and admission ownership must survive forced task exit without relying
/// on Rust destructors on a suspended syscall stack.
struct PresentOperation {
    id: u64,
    owner: Weak<crate::task::Task>,
    awaiting_admission: bool,
    caller_released: bool,
    target: Option<PresentTarget>,
    _owners: Option<PresentationOwners>,
    barrier: Option<Arc<ControlRequest>>,
    requests: Vec<Arc<ControlRequest>>,
}

fn dead_presentations(owners: &[(u64, Weak<crate::task::Task>)]) -> Vec<u64> {
    owners
        .iter()
        .filter_map(|(id, owner)| {
            let live = owner.upgrade().is_some_and(|task| {
                !matches!(
                    task.get_state(),
                    crate::task::TaskState::Zombie | crate::task::TaskState::Terminated
                )
            });
            (!live).then_some(*id)
        })
        .collect()
}

enum PresentTarget {
    Gpu {
        resource: GpuDisplayResource,
        region: DisplayRegion,
        producer_complete: bool,
    },
    Framebuffer {
        config: FramebufferConfig,
        physical_addr: u64,
        region: DisplayRegion,
    },
}

impl PresentTarget {
    fn needs_idle_barrier(&self) -> bool {
        !matches!(
            self,
            Self::Gpu {
                producer_complete: true,
                ..
            }
        )
    }
    fn request_count(&self) -> usize {
        match self {
            Self::Gpu { .. } => 2,
            Self::Framebuffer { .. } => 3,
        }
    }
}

fn remove_presentations(
    operations: &mut Vec<PresentOperation>,
    released: &[u64],
) -> Vec<PresentOperation> {
    let mut retired = Vec::new();
    let mut index = 0;
    while index < operations.len() {
        let operation = &mut operations[index];
        if released.contains(&operation.id) {
            operation.caller_released = true;
            operation.awaiting_admission = false;
        }
        // The caller's image/device leases cannot retire before the published
        // commands return. A transport failure needs reset proof, so preserve
        // those owners along with the quarantined ControlQueue allocations.
        if operation.caller_released
            && operation
                .requests
                .iter()
                .all(|request| matches!(request.status(), control::ControlStatus::Complete(_)))
        {
            retired.push(operations.swap_remove(index));
        } else {
            index += 1;
        }
    }
    retired
}

fn presentation_progress_needed(operations: &[PresentOperation]) -> bool {
    operations.iter().any(|operation| {
        !operation.caller_released
            || operation
                .requests
                .iter()
                .any(|request| matches!(request.status(), control::ControlStatus::Pending))
    })
}

fn prune_abandoned_presentations(core: &Arc<CoreLock<VirtioGpuDeviceCore>>) -> bool {
    let owners: Vec<_> = {
        let Some(core) = core.try_lock() else {
            return false;
        };
        core.presentations
            .iter()
            .map(|operation| (operation.id, operation.owner.clone()))
            .collect()
    };
    // Task destruction can release GPU handles; never upgrade/drop Task Arcs
    // under the core IRQ guard. Weak identity also prevents task-id reuse.
    let dead = dead_presentations(&owners);
    let retired = {
        let Some(mut core) = core.try_lock() else {
            return false;
        };
        remove_presentations(&mut core.presentations, &dead)
    };
    // Image/device destructors may re-enter this GPU. Move out, unlock, drop.
    drop(retired);
    true
}

struct PresentTicket {
    core: Weak<CoreLock<VirtioGpuDeviceCore>>,
    id: u64,
    armed: bool,
}

impl Drop for PresentTicket {
    fn drop(&mut self) {
        if self.armed {
            VirtioGpuDevice::release_present_operation(&self.core, self.id);
        }
    }
}

impl PresentationWait for PresentTicket {
    fn wait(mut self: Box<Self>) -> Result<(), &'static str> {
        let core = self.core.clone();
        let id = self.id;
        self.armed = false;
        drop(self);
        let result = VirtioGpuDevice::run_present_operation(&core, id);
        VirtioGpuDevice::release_present_operation(&core, id);
        result
    }
}

fn prepare_present_target(
    core: &VirtioGpuDeviceCore,
    target: &PresentTarget,
) -> Result<Vec<Arc<ControlRequest>>, &'static str> {
    match target {
        PresentTarget::Gpu {
            resource, region, ..
        } => {
            if resource.backend_cookie() != core.backend_cookie() {
                return Err("GPU display resource belongs to a different VirtIO GPU device");
            }
            if resource.resource_id() == 0 || resource.width() == 0 || resource.height() == 0 {
                return Err("GPU display resource is invalid");
            }
            let Some(rect) = VirtioGpuControl::clamp_region_to_rect(
                *region,
                resource.width(),
                resource.height(),
            ) else {
                return Ok(Vec::new());
            };
            presentation_requests(
                resource.resource_id(),
                resource.width(),
                resource.height(),
                rect,
                false,
            )
        }
        PresentTarget::Framebuffer {
            config,
            physical_addr,
            region,
        } => {
            let current_config = core.get_framebuffer_config()?;
            if core.get_framebuffer_address()? != *physical_addr {
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
            let display = core.display_info.read();
            let primary = display.as_ref().ok_or("Device not initialized")?.pmodes[0];
            if primary.enabled == 0 {
                return Err("Primary display not enabled");
            }
            let Some(rect) =
                VirtioGpuControl::clamp_region_to_rect(*region, primary.r.width, primary.r.height)
            else {
                return Ok(Vec::new());
            };
            let resource_id = core
                .current_resource_id
                .read()
                .ok_or("No framebuffer resource found")?;
            presentation_requests(resource_id, primary.r.width, primary.r.height, rect, true)
        }
    }
}

fn presentation_requests(
    resource_id: u32,
    width: u32,
    height: u32,
    rect: VirtioGpuRect,
    transfer: bool,
) -> Result<Vec<Arc<ControlRequest>>, &'static str> {
    fn header(hdr_type: u32) -> VirtioGpuCtrlHdr {
        VirtioGpuCtrlHdr {
            hdr_type,
            flags: 0,
            fence_id: 0,
            ctx_id: 0,
            padding: 0,
        }
    }
    fn request<T>(command: &T) -> Result<Arc<ControlRequest>, &'static str> {
        // SAFETY: callers supply initialized repr(C) VirtIO commands. New
        // copies these bytes into owned DMA before the borrow ends.
        let bytes = unsafe {
            core::slice::from_raw_parts(command as *const T as *const u8, core::mem::size_of::<T>())
        };
        ControlRequest::new(bytes, core::mem::size_of::<VirtioGpuCtrlHdr>())
    }
    let mut requests = Vec::with_capacity(if transfer { 3 } else { 2 });
    requests.push(request(&VirtioGpuSetScanout {
        hdr: header(VIRTIO_GPU_CMD_SET_SCANOUT),
        r: VirtioGpuRect {
            x: 0,
            y: 0,
            width,
            height,
        },
        scanout_id: 0,
        resource_id,
    })?);
    if transfer {
        requests.push(request(&VirtioGpuTransferToHost2d {
            hdr: header(VIRTIO_GPU_CMD_TRANSFER_TO_HOST_2D),
            r: rect,
            offset: (u64::from(rect.y) * u64::from(width) + u64::from(rect.x)) * 4,
            resource_id,
            padding: 0,
        })?);
    }
    requests.push(request(&VirtioGpuResourceFlush {
        hdr: header(VIRTIO_GPU_CMD_RESOURCE_FLUSH),
        r: rect,
        resource_id,
        padding: 0,
    })?);
    Ok(requests)
}

impl VirtioGpuDevice {
    fn begin_present(
        &self,
        target: PresentTarget,
        owners: Option<PresentationOwners>,
    ) -> Result<Box<dyn PresentationWait>, &'static str> {
        let task_id = crate::task::mytask()
            .ok_or("VirtIO GPU presentation requires a schedulable task")?
            .get_id();
        let owner = crate::sched::scheduler::get_task_by_id(task_id)
            .map(|task| Arc::downgrade(&task))
            .ok_or("VirtIO GPU presentation task is not registered")?;
        if !asynchronous::register(&self.core) {
            return Err("VirtIO GPU presentation completion worker is unavailable");
        }
        let id = {
            let mut core = self.core.lock();
            let id = core
                .next_present_id
                .checked_add(1)
                .ok_or("VirtIO GPU presentation identity exhausted")?;
            core.presentations
                .try_reserve(1)
                .map_err(|_| "VirtIO GPU presentation ownership allocation failed")?;
            core.next_present_id = id;
            core.presentations.push(PresentOperation {
                id,
                owner,
                awaiting_admission: true,
                caller_released: false,
                target: Some(target),
                _owners: owners,
                barrier: None,
                requests: Vec::new(),
            });
            // Publish ownership and wake progress before releasing the guard.
            // Even an immediate forced exit is observed with an idle queue.
            asynchronous::wake_worker();
            id
        };
        Ok(Box::new(PresentTicket {
            core: Arc::downgrade(&self.core),
            id,
            armed: true,
        }))
    }

    fn release_present_operation(core: &Weak<CoreLock<VirtioGpuDeviceCore>>, id: u64) {
        let retired = {
            let Some(core) = core.upgrade() else {
                return;
            };
            let mut core = core.lock();
            remove_presentations(&mut core.presentations, &[id])
        };
        drop(retired);
        asynchronous::wake_worker();
    }

    fn run_present_operation(
        weak_core: &Weak<CoreLock<VirtioGpuDeviceCore>>,
        id: u64,
    ) -> Result<(), &'static str> {
        let request_count = loop {
            let pending = {
                let core = weak_core
                    .upgrade()
                    .ok_or("VirtIO GPU presentation device was abandoned")?;
                let mut core = core.lock();
                let pending = {
                    let queues = core.virtqueues.lock();
                    queues.control.check()?;
                    let target = core
                        .presentations
                        .iter()
                        .find(|operation| operation.id == id)
                        .and_then(|operation| operation.target.as_ref())
                        .ok_or("VirtIO GPU presentation ownership was abandoned")?;
                    if target.needs_idle_barrier()
                        || !queues.control.can_enqueue_batch(target.request_count())
                    {
                        queues.control.pending_request()
                    } else {
                        None
                    }
                };
                if let Some(pending) = pending {
                    let weak = Arc::downgrade(&pending);
                    let operation = core
                        .presentations
                        .iter_mut()
                        .find(|operation| operation.id == id)
                        .ok_or("VirtIO GPU presentation ownership was abandoned")?;
                    operation.barrier = Some(pending);
                    Some(weak)
                } else {
                    // Legacy callers require idle validation. Completed-image
                    // callers need only batch capacity; unrelated contexts keep
                    // running. The guard makes either admission check atomic.
                    let target = core
                        .presentations
                        .iter()
                        .find(|operation| operation.id == id)
                        .and_then(|operation| operation.target.as_ref())
                        .ok_or("VirtIO GPU presentation ownership was abandoned")?;
                    let requests = prepare_present_target(&core, target)?;
                    if requests.is_empty() {
                        return Ok(());
                    }
                    {
                        let mut queues = core.virtqueues.lock();
                        queues
                            .control
                            .enqueue_batch(&requests, crate::timer::get_time_ns())
                            .map_err(|error| match error {
                                ControlEnqueueError::Busy => {
                                    "VirtIO GPU presentation queue is full"
                                }
                                ControlEnqueueError::Failed(error) => error,
                            })?;
                    }
                    let count = requests.len();
                    let operation = core
                        .presentations
                        .iter_mut()
                        .find(|operation| operation.id == id)
                        .ok_or("VirtIO GPU presentation ownership was abandoned")?;
                    operation.requests = requests;
                    operation.barrier = None;
                    operation.awaiting_admission = false;
                    // Concurrent presentations can return in a different order.
                    // Do not publish a stale cached scanout from a resumed waiter.
                    *core.scanout_resource_id.write() = None;
                    core.notify(0);
                    asynchronous::wake_worker();
                    break count;
                }
            };
            // Only a weak receipt crosses the scheduling boundary. The core
            // registry owns the barrier until normal or forced-exit cleanup.
            ControlRequest::wait_owned(
                pending
                    .expect("pending barrier request")
                    .upgrade()
                    .ok_or("VirtIO GPU presentation barrier was abandoned")?,
            )?;
        };
        for index in 0..request_count {
            let weak = {
                let core = weak_core
                    .upgrade()
                    .ok_or("VirtIO GPU presentation device was abandoned")?;
                let core = core.lock();
                let operation = core
                    .presentations
                    .iter()
                    .find(|operation| operation.id == id)
                    .ok_or("VirtIO GPU presentation ownership was abandoned")?;
                Arc::downgrade(&operation.requests[index])
            };
            ControlRequest::wait_owned(
                weak.upgrade()
                    .ok_or("VirtIO GPU presentation receipt was abandoned")?,
            )?;
            let mut bytes = [0; core::mem::size_of::<VirtioGpuCtrlHdr>()];
            {
                let core = weak_core
                    .upgrade()
                    .ok_or("VirtIO GPU presentation device was abandoned")?;
                let core = core.lock();
                let operation = core
                    .presentations
                    .iter()
                    .find(|operation| operation.id == id)
                    .ok_or("VirtIO GPU presentation ownership was abandoned")?;
                operation.requests[index].copy_response(&mut bytes)?;
            }
            // SAFETY: the returned owned response includes a complete header.
            let response =
                unsafe { core::ptr::read_unaligned(bytes.as_ptr() as *const VirtioGpuCtrlHdr) };
            validate_execution_response(response, None)?;
        }
        Ok(())
    }

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
        let core = VirtioGpuDeviceCore::new(base_addr);
        Self {
            interrupt_state: Arc::clone(&core.interrupt_state),
            core: Arc::new(CoreLock::new(core)),
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
        let core = VirtioGpuDeviceCore::new_pci(transport);
        Self {
            interrupt_state: Arc::clone(&core.interrupt_state),
            core: Arc::new(CoreLock::new(core)),
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

    fn on_mapped(&self, _vaddr: usize, _paddr: u64, _length: usize, _offset: usize) {
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
    core: Arc<CoreLock<VirtioGpuDeviceCore>>,
    control: Arc<VirtioGpuControl>,
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
        // Register independent progress before advertising async capability.
        let enabled = asynchronous::register(&device.core);
        device.core.lock().async_enabled = enabled;
        Self {
            core: Arc::clone(&device.core),
            control: Arc::clone(&device.core.lock().control),
        }
    }

    fn dialect_token(info: VirtioGpuBackendDialectInfo) -> u64 {
        (u64::from(info.id) << 32) | u64::from(info.max_version)
    }

    fn query_dialect_info(
        core: &VirtioGpuControl,
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
    core: Arc<CoreLock<VirtioGpuDeviceCore>>,
    control: Arc<VirtioGpuControl>,
    context_id: u32,
    info: GpuBackendContextInfo,
    attached_resources: IrqSpinLock<Vec<u32>>,
}

/// Real VirtIO image resource using generic kernel-owned backing when required.
struct VirtioGpuBackendImage {
    _core: Arc<CoreLock<VirtioGpuDeviceCore>>,
    control: Arc<VirtioGpuControl>,
    resource_id: u32,
    create: GpuImageCreateInfo,
    allocation_size: u64,
    has_attached_backing: bool,
    backend_cookie: u64,
}

/// Real VirtIO buffer resource backed by a generic GPU buffer allocation.
struct VirtioGpuBackendBuffer {
    _core: Arc<CoreLock<VirtioGpuDeviceCore>>,
    control: Arc<VirtioGpuControl>,
    resource_id: u32,
    allocation_size: u64,
    backend_cookie: u64,
}

impl Drop for VirtioGpuBackendImage {
    fn drop(&mut self) {
        let core = self.control.as_ref();
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
        let core = self.control.as_ref();
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
        let core = self.control.as_ref();
        let attached_resources = core::mem::take(&mut *self.attached_resources.lock());
        for resource_id in attached_resources {
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
        let core = self.control.as_ref();
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
        let core = self.control.as_ref();
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
            control: Arc::clone(&self.control),
            context_id: self.context_id,
            async_enabled: self.core.lock().async_enabled,
            submit_pool: Arc::new(asynchronous::RequestPool::new()),
        }))
    }

    fn attach_image(&self, image: &dyn GpuBackendImage) -> Result<u64, &'static str> {
        let info = image.query_info();
        let resource_id = u32::try_from(info.command_resource_token)
            .map_err(|_| "VirtIO GPU image token does not fit a resource ID")?;
        let core = self.control.as_ref();
        if resource_id == 0 || image.backend_cookie() != core.backend_cookie() {
            return Err("GPU image belongs to a different VirtIO GPU device");
        }
        if self
            .attached_resources
            .lock()
            .iter()
            .any(|&attached_resource_id| attached_resource_id == resource_id)
        {
            return Err("VirtIO GPU image is already attached to this context");
        }
        core.attach_acceleration_resource(self.context_id, resource_id)?;
        let mut attached_resources = self.attached_resources.lock();
        if attached_resources.try_reserve(1).is_err() {
            drop(attached_resources);
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
        let core = self.control.as_ref();
        if resource_id == 0 || image.backend_cookie() != core.backend_cookie() {
            return Err("GPU image belongs to a different VirtIO GPU device");
        }
        if !self.attached_resources.lock().contains(&resource_id) {
            return Err("VirtIO GPU image is not attached to this context");
        }
        core.detach_acceleration_resource(self.context_id, resource_id)?;
        let mut attached_resources = self.attached_resources.lock();
        if let Some(index) = attached_resources.iter().position(|&id| id == resource_id) {
            attached_resources.swap_remove(index);
        }
        Ok(())
    }

    fn upload_image_bgra(
        &self,
        image: &dyn GpuBackendImage,
        upload: GpuImageUploadInfo,
    ) -> Result<(), &'static str> {
        self.transfer_image_bgra(image, upload)
    }

    fn upload_image(
        &self,
        image: &dyn GpuBackendImage,
        upload: GpuImageUploadInfo,
    ) -> Result<(), &'static str> {
        self.transfer_image_bgra(image, upload)
    }

    fn readback_image(
        &self,
        image: &dyn GpuBackendImage,
        readback: GpuImageUploadInfo,
    ) -> Result<(), &'static str> {
        self.transfer_image_from_host_bgra(image, readback)
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
        if resource_id == 0 || buffer.backend_cookie() != self.control.backend_cookie() {
            return Err("GPU buffer belongs to a different VirtIO GPU device");
        }
        let core = self.control.as_ref();
        core.attach_acceleration_resource(self.context_id, resource_id)?;
        let mut attached_resources = self.attached_resources.lock();
        if attached_resources.try_reserve(1).is_err() {
            drop(attached_resources);
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
        let core = self.control.as_ref();
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
    core: Arc<CoreLock<VirtioGpuDeviceCore>>,
    control: Arc<VirtioGpuControl>,
    context_id: u32,
    async_enabled: bool,
    submit_pool: Arc<asynchronous::RequestPool>,
}

impl GpuBackendQueue for VirtioGpuBackendQueue {
    fn query_info(&self) -> GpuBackendQueueInfo {
        GpuBackendQueueInfo::new(VIRTIO_GPU_MAX_OPAQUE_COMMAND_SIZE)
    }

    fn submit(&self, commands: &[u8]) -> Result<(), GpuBackendSubmitError> {
        let core = self.control.as_ref();
        let fence_id = core
            .next_acceleration_fence_id()
            .map_err(GpuBackendSubmitError::Unavailable)?;
        core.submit_acceleration_commands(self.context_id, commands, Some(fence_id))
    }

    fn async_capacity(&self) -> u32 {
        if self.async_enabled {
            asynchronous::ASYNC_CAPACITY
        } else {
            0
        }
    }

    fn async_command_limit(&self) -> u32 {
        // Async requests own their DMA allocation; they do not use the legacy
        // 64 KiB staging buffer. A complete bounded IR stream can therefore be
        // admitted once, instead of racing admission between its packets.
        crate::device::gpu::GPU_MAX_OPAQUE_COMMAND_SIZE
    }

    fn enqueue(&self, submission: GpuSubmission) -> Result<(), GpuBackendEnqueueError> {
        // Private DMA preparation can grow storage only before taking any
        // device/transport lock. Steady-state requests reuse retired storage.
        let mut prepared = match asynchronous::Prepared::new(
            &self.submit_pool,
            self.context_id,
            submission.commands(),
        ) {
            Ok(prepared) => Some(prepared),
            Err(error) => return Err(GpuBackendEnqueueError::Rejected(error, submission)),
        };
        // A concurrent legacy control operation may wait for GPU execution.
        // Async admission reports pressure instead of joining that wait.
        let Some(mut guard) = self.core.try_lock() else {
            return Err(GpuBackendEnqueueError::Busy(submission));
        };
        let core = &mut *guard;
        let control = Arc::clone(&core.control);
        let Some(synchronous) = control.synchronous.try_admission() else {
            return Err(GpuBackendEnqueueError::Busy(submission));
        };
        if synchronous.blocked() {
            return Err(GpuBackendEnqueueError::Busy(submission));
        }
        if core.presentations.iter().any(|operation| {
            operation.awaiting_admission
                && operation
                    .target
                    .as_ref()
                    .is_none_or(PresentTarget::needs_idle_barrier)
        }) {
            return Err(GpuBackendEnqueueError::Busy(submission));
        }
        if !self.async_enabled {
            return Err(GpuBackendEnqueueError::Rejected(
                GpuBackendSubmitError::Unavailable("VirtIO GPU async progress is not available"),
                submission,
            ));
        }
        let fence = match core.next_acceleration_fence_id() {
            Ok(fence) => fence,
            Err(error) => {
                return Err(GpuBackendEnqueueError::Rejected(
                    GpuBackendSubmitError::Unavailable(error),
                    submission,
                ));
            }
        };
        {
            let mut queues = control.virtqueues.lock();
            core.async_submissions.enqueue(
                &mut queues.control,
                self.context_id,
                fence,
                submission,
                &mut prepared,
                crate::timer::get_time_ns(),
            )?;
        }
        core.notify(0);
        asynchronous::wake_worker();
        Ok(())
    }
}

fn virgl_image_format(format: u32) -> Result<u32, &'static str> {
    match format {
        GPU_IMAGE_FORMAT_BGRA8_UNORM => Ok(VIRTIO_GPU_FORMAT_B8G8R8A8_UNORM),
        GPU_IMAGE_FORMAT_DEPTH32_FLOAT => Ok(VIRGL_FORMAT_Z32_FLOAT),
        crate::device::gpu::GPU_IMAGE_FORMAT_R8_UNORM => Ok(64), // VIRGL_FORMAT_R8_UNORM
        crate::device::gpu::GPU_IMAGE_FORMAT_RGBA16_FLOAT => Ok(94), // VIRGL_FORMAT_R16G16B16A16_FLOAT
        crate::device::gpu::GPU_IMAGE_FORMAT_RGBA32_FLOAT => Ok(31), // VIRGL_FORMAT_R32G32B32A32_FLOAT
        crate::device::gpu::GPU_IMAGE_FORMAT_RG8_UNORM => Ok(65),    // VIRGL_FORMAT_R8G8_UNORM
        _ => Err("VirtIO GPU image format is unsupported"),
    }
}

fn virgl_image_layout(
    create: GpuImageCreateInfo,
) -> Result<crate::device::gpu::GpuBackendImageLayout, &'static str> {
    let mut layout = crate::device::gpu::GpuBackendImageLayout::tight_uncompressed(create)?;
    let bytes = crate::device::gpu::image_format_bytes_per_texel(create.format)
        .ok_or("VirtIO GPU image format is unsupported")?;
    let mut total = 0u64;
    for level in 0..create.mip_levels {
        let width = (create.width >> level).max(1);
        let height = (create.height >> level).max(1);
        total = total
            .checked_add(u64::from(width) * u64::from(height) * u64::from(bytes))
            .ok_or("VirtIO GPU mip allocation overflows")?;
    }
    layout.total_size = total
        .checked_mul(u64::from(create.array_layers))
        .ok_or("VirtIO GPU array allocation overflows")?;
    layout.planes[0].array_pitch =
        u32::try_from(total).map_err(|_| "VirtIO GPU mip-chain stride exceeds ABI")?;
    layout.planes[0].size = layout.total_size;
    Ok(layout)
}

impl GpuBackend for VirtioGpuBackend {
    fn plan_image(
        &self,
        create: GpuImageCreateInfo,
    ) -> Result<crate::device::gpu::GpuBackendImageLayout, &'static str> {
        virgl_image_layout(create)
    }
    fn query_info(&self) -> GpuBackendInfo {
        self.control.gpu_backend_info()
    }

    fn query_dialect(&self, index: u32) -> Result<GpuBackendDialectInfo, &'static str> {
        let core = self.control.as_ref();
        Self::query_dialect_info(&core, index)
    }

    fn create_context(
        &self,
        dialect: GpuBackendDialectDescriptor,
    ) -> Result<Arc<dyn GpuBackendContext>, &'static str> {
        let core = self.control.as_ref();
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
            control: Arc::clone(&self.control),
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
        let core = self.control.as_ref();
        core.require_virgl()?;
        let resource_id = core.create_acceleration_resource(VirtioGpuAccelerationResource3d {
            resource_id: 0,
            target: if create.cube {
                4 /* PIPE_TEXTURE_CUBE */
            } else if create.array_layers > 1 {
                7 /* PIPE_TEXTURE_2D_ARRAY */
            } else {
                PIPE_TEXTURE_2D
            },
            format: virgl_image_format(create.format)?,
            bind: virgl_image_bind(create.usage),
            width: create.width,
            height: create.height,
            depth: 1,
            array_size: create.array_layers,
            last_level: create.mip_levels - 1,
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
            _core: Arc::clone(&self.core),
            control: Arc::clone(&self.control),
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
        let core = self.control.as_ref();
        core.require_virgl()?;
        let resource_id = core.create_acceleration_resource(VirtioGpuAccelerationResource3d {
            resource_id: 0,
            target: PIPE_BUFFER,
            format: 0,
            bind: if create.flags & crate::device::gpu::GPU_BUFFER_FLAG_QUERY_RESULT != 0 {
                VIRGL_BIND_CUSTOM
            } else {
                PIPE_BIND_VERTEX_BUFFER
            },
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
            _core: Arc::clone(&self.core),
            control: Arc::clone(&self.control),
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

    fn get_framebuffer_address(&self) -> Result<u64, &'static str> {
        self.core.lock().get_framebuffer_address()
    }

    fn get_framebuffer_info(&self) -> Result<(FramebufferConfig, u64), &'static str> {
        let core = self.core.lock();
        let _ = core.poll_display_resize();
        let config = core.get_framebuffer_config()?;
        let physical_addr = core.get_framebuffer_address()?;
        Ok((config, physical_addr))
    }

    fn present_framebuffer_region(
        &self,
        config: &FramebufferConfig,
        physical_addr: u64,
        region: DisplayRegion,
    ) -> Result<(), &'static str> {
        self.begin_present(
            PresentTarget::Framebuffer {
                config: config.clone(),
                physical_addr,
                region,
            },
            None,
        )?
        .wait()
    }

    fn begin_framebuffer_present(
        &self,
        config: FramebufferConfig,
        physical_addr: u64,
        region: DisplayRegion,
        owners: PresentationOwners,
    ) -> Result<Option<Box<dyn PresentationWait>>, &'static str> {
        self.begin_present(
            PresentTarget::Framebuffer {
                config,
                physical_addr,
                region,
            },
            Some(owners),
        )
        .map(Some)
    }

    fn present_gpu_resource_region(
        &self,
        resource: GpuDisplayResource,
        region: DisplayRegion,
    ) -> Result<(), &'static str> {
        self.begin_present(
            PresentTarget::Gpu {
                resource,
                region,
                producer_complete: false,
            },
            None,
        )?
        .wait()
    }

    fn begin_gpu_resource_present(
        &self,
        resource: GpuDisplayResource,
        region: DisplayRegion,
        options: GpuPresentOptions,
        owners: PresentationOwners,
    ) -> Result<Option<Box<dyn PresentationWait>>, &'static str> {
        self.begin_present(
            PresentTarget::Gpu {
                resource,
                region,
                producer_complete: options.is_producer_complete(),
            },
            Some(owners),
        )
        .map(Some)
    }

    fn init_graphics(&self) -> Result<(), &'static str> {
        {
            let core = self.core.lock();
            let mut initialized = core.initialized.lock();
            if *initialized {
                crate::println!("[virtio-gpu] init_graphics: already initialized");
                return Ok(());
            }
            *initialized = true;
        }

        crate::println!("[virtio-gpu] init_graphics: get_display_info");

        // Get display information
        self.core.lock().get_display_info_internal()?;

        crate::println!("[virtio-gpu] init_graphics: setup_framebuffer");

        // Set up framebuffer
        self.core.lock().setup_framebuffer()?;

        crate::println!("[virtio-gpu] init_graphics: done");
        Ok(())
    }
}

// The kernel test harness runs before a schedulable task exists. Exercise the
// real presentation command batch by polling its transport, while separately
// checking that the public asynchronous API rejects that boot context.
#[cfg(test)]
impl VirtioGpuDevice {
    pub(crate) fn present_framebuffer_in_boot_test(
        &self,
        region: DisplayRegion,
    ) -> Result<(), &'static str> {
        assert!(crate::task::mytask().is_none());
        assert_eq!(
            self.present_current_framebuffer_region(region),
            Err("VirtIO GPU presentation requires a schedulable task")
        );
        let core = self.core.lock();
        assert!(core.presentations.is_empty());
        let target = PresentTarget::Framebuffer {
            config: core.get_framebuffer_config()?,
            physical_addr: core.get_framebuffer_address()?,
            region,
        };
        let requests = prepare_present_target(&core, &target)?;
        if requests.is_empty() {
            return Ok(());
        }
        core.virtqueues
            .lock()
            .control
            .enqueue_batch(&requests, crate::timer::get_time_ns())
            .map_err(|_| "Boot test presentation enqueue failed")?;
        core.notify(0);
        for request in &requests {
            loop {
                core.virtqueues
                    .lock()
                    .control
                    .reap(crate::timer::get_time_ns())?;
                match request.status() {
                    control::ControlStatus::Complete(_) => break,
                    control::ControlStatus::Failed(error) => return Err(error),
                    control::ControlStatus::Pending => core::hint::spin_loop(),
                }
            }
            let mut bytes = [0; core::mem::size_of::<VirtioGpuCtrlHdr>()];
            request.copy_response(&mut bytes)?;
            // SAFETY: the complete transport response contains this header.
            let response =
                unsafe { core::ptr::read_unaligned(bytes.as_ptr().cast::<VirtioGpuCtrlHdr>()) };
            validate_execution_response(response, None)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn native_color_mips_arrays_keep_their_physical_texel_size() {
        for (format, bytes, pipe) in [
            (crate::device::gpu::GPU_IMAGE_FORMAT_R8_UNORM, 1u64, 64),
            (crate::device::gpu::GPU_IMAGE_FORMAT_RG8_UNORM, 2, 65),
            (
                GPU_IMAGE_FORMAT_BGRA8_UNORM,
                4,
                VIRTIO_GPU_FORMAT_B8G8R8A8_UNORM,
            ),
        ] {
            let mut create = GpuImageCreateInfo::new(
                format,
                GPU_IMAGE_USAGE_RENDER_TARGET
                    | GPU_IMAGE_USAGE_SAMPLED
                    | GPU_IMAGE_USAGE_TRANSFER_SRC
                    | GPU_IMAGE_USAGE_TRANSFER_DST,
                7,
                5,
            );
            create.mip_levels = 3;
            create.array_layers = 3;
            let layout = virgl_image_layout(create).unwrap();
            assert_eq!(virgl_image_format(format).unwrap(), pipe);
            assert_eq!(layout.planes[0].bytes_per_block, bytes as u16);
            assert_eq!(layout.planes[0].row_pitch, 7 * bytes as u32);
            // Per layer: 7x5 + 3x2 + 1x1, then three array layers.
            assert_eq!(layout.planes[0].array_pitch, 42 * bytes as u32);
            assert_eq!(layout.total_size, 126 * bytes);
            assert_eq!(layout.planes[0].size, layout.total_size);
            assert!(layout.is_valid());
        }
        assert!(virgl_image_format(crate::device::gpu::GPU_IMAGE_FORMAT_NV12).is_err());
    }

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
    fn virgl_transfer_only_images_use_texture_storage() {
        for usage in [
            GPU_IMAGE_USAGE_TRANSFER_SRC,
            GPU_IMAGE_USAGE_TRANSFER_DST,
            GPU_IMAGE_USAGE_TRANSFER_SRC | GPU_IMAGE_USAGE_TRANSFER_DST,
        ] {
            assert_eq!(virgl_image_bind(usage), PIPE_BIND_SAMPLER_VIEW);
        }
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
    const VIRTIO_GPU_PADDR: u64 = 0x10002000;

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
    fn unchanged_display_snapshot_does_not_wait_for_pending_gpu_work() {
        // No hardware responds to this request. A snapshot without a display
        // event must leave it pending rather than drain it with IRQs masked.
        let mut registers = [0u32; 128];
        let device = VirtioGpuDeviceCore::new(registers.as_mut_ptr() as usize);
        let request = ControlRequest::new(
            &[0; core::mem::size_of::<VirtioGpuCtrlHdr>()],
            core::mem::size_of::<VirtioGpuCtrlHdr>(),
        )
        .unwrap();
        device
            .virtqueues
            .lock()
            .control
            .enqueue(&request, crate::timer::get_time_ns())
            .unwrap();
        device.poll_display_resize().unwrap();
        assert_eq!(request.status(), control::ControlStatus::Pending);
        assert_eq!(*device.virtqueues.lock().control.ring.avail.idx, 1);
    }

    #[test_case]
    fn display_change_ack_preserves_retry_and_unrelated_events() {
        use crate::drivers::virtio::device::Register;
        let mut registers = [0u32; 128];
        let device = VirtioGpuDeviceCore::new(registers.as_mut_ptr() as usize);
        let config = Register::DeviceConfig.offset() / 4;
        registers[config + VIRTIO_GPU_CONFIG_EVENTS_READ_OFFSET / 4] = VIRTIO_GPU_EVENT_DISPLAY | 2;
        assert!(device.display_update_needed());
        assert_eq!(
            registers[config + VIRTIO_GPU_CONFIG_EVENTS_CLEAR_OFFSET / 4],
            VIRTIO_GPU_EVENT_DISPLAY
        );
        // Emulate the device acknowledging the display event. If the query
        // fails, the driver must still retry despite the cleared device bit.
        registers[config + VIRTIO_GPU_CONFIG_EVENTS_READ_OFFSET / 4] = 2;
        assert!(device.display_update_needed());
        device
            .display_update_pending
            .store(false, Ordering::Release);
        assert!(!device.display_update_needed());
        registers[config + VIRTIO_GPU_CONFIG_EVENTS_READ_OFFSET / 4] = VIRTIO_GPU_EVENT_DISPLAY;
        assert!(device.display_update_needed());
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
            .present_framebuffer_in_boot_test(DisplayRegion::full(&config))
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
            .present_framebuffer_in_boot_test(DisplayRegion::full(&config))
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
            .present_framebuffer_in_boot_test(DisplayRegion::full(&config))
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
            .present_framebuffer_in_boot_test(DisplayRegion::full(&config))
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
                .present_framebuffer_in_boot_test(DisplayRegion::full(&config))
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
                .present_framebuffer_in_boot_test(DisplayRegion::new(100, 100, 1, 1))
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
        crate::println!("[Test] Starting VirtIO GPU command flow verification");
        device.init_graphics().unwrap();

        let config = device.get_framebuffer_config().unwrap();
        let fb_addr = device.get_framebuffer_address().unwrap();

        crate::println!(
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

        crate::println!("[Test] Written checkerboard pattern to framebuffer");

        // Test presenting different regions
        device
            .present_framebuffer_in_boot_test(DisplayRegion::new(0, 0, 50, 50))
            .unwrap();
        device
            .present_framebuffer_in_boot_test(DisplayRegion::new(50, 50, 50, 50))
            .unwrap();
        device
            .present_framebuffer_in_boot_test(DisplayRegion::full(&config))
            .unwrap();

        crate::println!("[Test] VirtIO GPU command flow verification completed");
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

        crate::println!("[Test] Testing VirtIO GPU resource management");
        crate::println!("[Test] Primary framebuffer resource should be ID 1");

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
            .present_framebuffer_in_boot_test(DisplayRegion::new(
                0,
                0,
                config.width.min(500),
                config.height.min(500),
            ))
            .unwrap();

        crate::println!("[Test] Resource management test completed");
    }
}
