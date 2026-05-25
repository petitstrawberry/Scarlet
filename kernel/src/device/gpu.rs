//! GPU device interface.
//!
//! This module defines the non-framebuffer GPU interface used by accelerated
//! backends such as virtio-gpu virgl. Display scanout remains in
//! `device::graphics`; this module is for GPU contexts, resources, command
//! submission, and fences.

use alloc::vec::Vec;

use super::Device;

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

    // SAFETY: Scarlet's existing device control ABI passes userspace pointers
    // directly to device objects. The caller supplies the pointer and the kernel
    // expects the current address space to make it accessible for this access.
    Ok(unsafe { core::ptr::read(ptr as *const T) })
}

fn write_user_value<T: Copy>(ptr: usize, value: &T) -> Result<(), &'static str> {
    if ptr == 0 {
        return Err("GPU ioctl pointer is null");
    }

    // SAFETY: See `read_user_value`; this mirrors the existing framebuffer
    // control ABI style for writing small fixed-size response structures.
    unsafe {
        core::ptr::write(ptr as *mut T, *value);
    }
    Ok(())
}

/// Character device exposing a `GpuDevice` to userspace.
pub struct GpuCharDevice {
    gpu: alloc::sync::Arc<dyn GpuDevice>,
}

impl GpuCharDevice {
    /// Create a GPU character device wrapper.
    ///
    /// # Arguments
    ///
    /// * `gpu` - GPU backend to expose.
    ///
    /// # Returns
    ///
    /// A new GPU character device.
    pub fn new(gpu: alloc::sync::Arc<dyn GpuDevice>) -> Self {
        Self { gpu }
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

        // SAFETY: The userspace request supplies a writable destination buffer.
        // This follows the same direct-pointer device control convention as the
        // framebuffer control path.
        let buffer = unsafe {
            core::slice::from_raw_parts_mut(request.buffer_ptr as *mut u8, request.buffer_len)
        };
        request.bytes_written = self.gpu.read_capset(request.id, request.version, buffer)?;
        write_user_value(arg, &request)?;
        Ok(0)
    }

    fn handle_create_context(&self, arg: usize) -> Result<i32, &'static str> {
        let request: GpuContextRequest = read_user_value(arg)?;
        let name_len = request.name_len.min(GPU_CONTEXT_NAME_MAX);
        let mut name_bytes = [0u8; GPU_CONTEXT_NAME_MAX];
        if request.name_ptr != 0 && name_len != 0 {
            // SAFETY: The userspace request supplies a readable debug-name
            // buffer. We cap the copy to a small fixed-size kernel buffer.
            let source =
                unsafe { core::slice::from_raw_parts(request.name_ptr as *const u8, name_len) };
            name_bytes[..name_len].copy_from_slice(source);
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

    fn handle_submit_commands(&self, arg: usize) -> Result<i32, &'static str> {
        let request: GpuSubmitRequest = read_user_value(arg)?;
        if request.commands_ptr == 0 || request.commands_len == 0 {
            return Err("GPU command buffer is invalid");
        }

        // SAFETY: The userspace request supplies a readable command buffer. The
        // GPU backend copies/sends it synchronously before this method returns.
        let commands = unsafe {
            core::slice::from_raw_parts(request.commands_ptr as *const u8, request.commands_len)
        };
        let fence_id = if (request.flags & GPU_SUBMIT_FLAG_FENCE) != 0 {
            Some(request.fence_id)
        } else {
            None
        };
        self.gpu.submit_commands(GpuCommandSubmission {
            context_id: request.context_id,
            commands,
            fence_id,
        })?;
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
        true
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
            _ => Err("Unsupported GPU control command"),
        }
    }
}

impl crate::object::capability::MemoryMappingOps for GpuCharDevice {
    fn get_mapping_info(
        &self,
        _offset: usize,
        _length: usize,
    ) -> Result<(usize, usize, bool), &'static str> {
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
