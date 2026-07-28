//! Kernel-owned GPU child capability objects.

use alloc::{string::String, sync::Arc};

use super::connection::{read_user_value, write_user_value};
use super::{
    GPU_ABI_VERSION, GPU_BUFFER_QUERY_INFO, GPU_IMAGE_FORMAT_BGRA8_UNORM, GPU_IMAGE_QUERY_INFO,
    GPU_IMAGE_USAGE_VALID, GPU_RESULT_INVALID_ABI, GPU_TIMELINE_CREATE_POINT, GPU_TIMELINE_FAIL,
    GPU_TIMELINE_QUERY, GPU_TIMELINE_SIGNAL, GpuBackend, GpuBackendImage, GpuBufferInfo,
    GpuImageCreateInfo, GpuImageInfo, GpuTimelineCreatePoint, GpuTimelineFail, GpuTimelineInfo,
    GpuTimelineSignal,
};
use crate::environment::PAGE_SIZE;
use crate::mem::page::{Page, allocate_raw_pages, free_raw_pages};
use crate::object::capability::ControlOps;
use crate::object::capability::memory_mapping::{
    AccessKind, AccessOp, MemoryMappingInfo, MemoryMappingOps, ResolveFaultError,
    ResolveFaultResult,
};
use crate::object::capability::selectable::{
    ReadyInterest, ReadySet, SelectWaitOutcome, Selectable,
};
use crate::sched::scheduler::current_task_id;
use crate::sync::{IrqSpinLock, waker::Waker};
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
/// `true` only for a non-empty BGRA8 render-target and presentable image.
pub(crate) const fn image_create_is_valid(create: GpuImageCreateInfo) -> bool {
    create.format == GPU_IMAGE_FORMAT_BGRA8_UNORM
        && create.usage == GPU_IMAGE_USAGE_VALID
        && create.width != 0
        && create.height != 0
}

/// Kernel-owned backend image capability.
pub struct GpuImage {
    backend_image: Arc<dyn GpuBackendImage>,
}

impl GpuImage {
    /// Adopt a real backend image after validating its immutable metadata.
    ///
    /// # Arguments
    ///
    /// * `backend_image` - Real backend image to own.
    /// * `create` - Generic descriptor used to create the image.
    ///
    /// # Returns
    ///
    /// A capability retaining the backend image, or an error when the backend
    /// returned metadata inconsistent with the validated request.
    pub fn new(
        backend_image: Arc<dyn GpuBackendImage>,
        create: GpuImageCreateInfo,
    ) -> Result<Self, &'static str> {
        if !image_create_is_valid(create) {
            return Err("GPU image descriptor is invalid");
        }
        let info = backend_image.query_info();
        if info.format != create.format
            || info.usage != create.usage
            || info.width != create.width
            || info.height != create.height
            || info.command_resource_token == 0
            || info.allocation_size == 0
        {
            return Err("GPU backend image metadata is invalid");
        }
        Ok(Self { backend_image })
    }

    /// Return immutable backend-neutral image information.
    ///
    /// # Returns
    ///
    /// The image format, usage, extent, command token, and allocation size.
    pub fn query_info(&self) -> super::GpuBackendImageInfo {
        self.backend_image.query_info()
    }

    /// Clone the backend image for a context lifetime reference.
    ///
    /// # Returns
    ///
    /// A strong reference to the real backend image.
    pub(crate) fn backend_image(&self) -> Arc<dyn GpuBackendImage> {
        Arc::clone(&self.backend_image)
    }

    /// Return this image's display descriptor when it is presentable.
    ///
    /// # Returns
    ///
    /// An internal descriptor accepted by the graphics display bridge.
    pub(crate) fn display_resource(&self) -> Option<crate::device::graphics::GpuDisplayResource> {
        self.backend_image.display_resource()
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

/// Kernel-owned, fixed-size GPU buffer.
pub struct GpuBuffer {
    backend: Arc<dyn GpuBackend>,
    paddr: usize,
    allocation_size: usize,
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
        Ok(Self {
            backend,
            paddr: virt_to_phys(pages as usize),
            allocation_size,
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
    pub const fn allocation_size(&self) -> usize {
        self.allocation_size
    }

    fn query_info(&self, info: &mut GpuBufferInfo) {
        info.clear_response();
        if info.abi_version != GPU_ABI_VERSION {
            info.result = GPU_RESULT_INVALID_ABI;
            return;
        }
        info.flags = self.flags;
        info.size_bytes = self.allocation_size as u64;
        info.cpu_visible = u32::from(self.cpu_visible());
        let _ = &self.backend;
    }

    fn handle_query_info(&self, arg: usize) -> Result<i32, &'static str> {
        let mut info = read_user_value(arg)?;
        self.query_info(&mut info);
        write_user_value(arg, &info)?;
        Ok(0)
    }
}

impl Drop for GpuBuffer {
    fn drop(&mut self) {
        let page_count = self.allocation_size / PAGE_SIZE;
        if page_count != 0 {
            let pages = phys_to_virt(self.paddr) as *mut Page;
            free_raw_pages(pages, page_count);
        }
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
        if end > self.allocation_size {
            return Err("GPU buffer mapping range exceeds allocation");
        }
        let paddr = self
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
        if offset >= self.allocation_size {
            return Err(ResolveFaultError::Unmapped);
        }
        Ok(ResolveFaultResult {
            paddr_page_base: self
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
