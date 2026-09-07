//! Graphics device interface
//!
//! This module defines the interface for graphics devices in the kernel.
//! It provides abstractions for framebuffer operations and graphics device management.

use alloc::vec::Vec;
use core::any::Any;

use self::output::{DisplayOutput, DisplayRegion};
use alloc::sync::Arc;

use super::{Device, DeviceType, manager::DeviceManager};
use crate::object::capability::selectable::Selectable;
use crate::object::capability::{ControlOps, MemoryMappingOps};

pub mod display_device;
pub mod framebuffer_device;
pub mod manager;
pub mod output;

#[cfg(test)]
mod tests;

/// Get the first available graphics device
///
/// This is a convenience function to get the first graphics device registered in the system.
/// Returns None if no graphics devices are available.
pub fn get_graphics_device() -> Option<Arc<dyn Device>> {
    let manager = DeviceManager::get_manager();
    if let Some(device_id) = manager.get_first_device_by_type(DeviceType::Graphics) {
        return manager.get_device(device_id);
    }
    None
}

/// Pixel format for framebuffer
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    /// 32-bit RGBA (8 bits per channel)
    RGBA8888,
    /// 32-bit BGRA (8 bits per channel)  
    BGRA8888,
    XRGB8888,
    XBGR8888,
    XRGB2101010,
    /// 24-bit RGB (8 bits per channel)
    RGB888,
    /// 16-bit RGB (5-6-5 bits)
    RGB565,
    ARGB1555,
    XRGB1555,
}

impl PixelFormat {
    /// Get bytes per pixel for this format
    pub fn bytes_per_pixel(&self) -> usize {
        match self {
            PixelFormat::RGBA8888
            | PixelFormat::BGRA8888
            | PixelFormat::XRGB8888
            | PixelFormat::XBGR8888
            | PixelFormat::XRGB2101010 => 4,
            PixelFormat::RGB888 => 3,
            PixelFormat::RGB565 | PixelFormat::ARGB1555 | PixelFormat::XRGB1555 => 2,
        }
    }
}

/// Framebuffer configuration
#[derive(Debug, Clone)]
pub struct FramebufferConfig {
    /// Width in pixels
    pub width: u32,
    /// Height in pixels
    pub height: u32,
    /// Pixel format
    pub format: PixelFormat,
    /// Stride (bytes per row)
    pub stride: u32,
}

impl FramebufferConfig {
    /// Create a new framebuffer configuration
    pub fn new(width: u32, height: u32, format: PixelFormat) -> Self {
        let stride = width * format.bytes_per_pixel() as u32;
        Self {
            width,
            height,
            format,
            stride,
        }
    }

    /// Get the total size of the framebuffer in bytes
    pub fn size(&self) -> usize {
        (self.stride * self.height) as usize
    }
}

/// GPU resource that can be presented through the display pipeline.
#[derive(Debug, Clone)]
pub struct GpuDisplayResource {
    resource_id: u32,
    width: u32,
    height: u32,
    backend_cookie: u64,
    linear_backing: Option<GpuLinearDisplayBacking>,
}

/// Producer guarantees attached to one GPU-image presentation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GpuPresentOptions {
    swapchain_buffer: bool,
}

impl GpuPresentOptions {
    /// Describe a buffer that participates in a presentation swapchain.
    ///
    /// The producer promises not to write this image again after a successful
    /// present until a different swapchain image has also been presented
    /// successfully. A display driver may therefore retain and scan out the
    /// image directly instead of copying it into a display-owned buffer.
    pub const fn swapchain_buffer() -> Self {
        Self {
            swapchain_buffer: true,
        }
    }

    /// Return whether the producer supplied the swapchain lifetime guarantee.
    pub const fn is_swapchain_buffer(self) -> bool {
        self.swapchain_buffer
    }
}

/// Linear framebuffer backing exported by a GPU image for cross-device scanout.
#[derive(Clone)]
pub struct GpuLinearDisplayBacking {
    physical_addr: usize,
    physical_segments: Arc<[GpuBackingSegment]>,
    allocation_size: u64,
    stride: u32,
    format: PixelFormat,
    // A display controller may continue fetching after the synchronous
    // present call returns. Keep the producer's allocation alive until the
    // controller replaces this scanout resource.
    _owner: Arc<dyn GpuDisplayBackingOwner>,
}

/// One physically contiguous extent of a logically linear GPU allocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuBackingSegment {
    physical_addr: usize,
    length: usize,
}

impl GpuBackingSegment {
    /// Describe one stable physical extent.
    ///
    /// # Arguments
    ///
    /// * `physical_addr` - Page-aligned physical base address.
    /// * `length` - Non-zero extent length in bytes.
    pub const fn new(physical_addr: usize, length: usize) -> Self {
        Self {
            physical_addr,
            length,
        }
    }

    /// Return the physical base address of this extent.
    pub const fn physical_addr(self) -> usize {
        self.physical_addr
    }

    /// Return the extent length in bytes.
    pub const fn length(self) -> usize {
        self.length
    }
}

/// Opaque lifetime owner for cross-device GPU scanout memory.
///
/// Display drivers retain this object while hardware may still fetch from the
/// corresponding physical allocation. It intentionally exposes no device- or
/// architecture-specific operations.
pub trait GpuDisplayBackingOwner: Send + Sync {}

impl<T: Send + Sync> GpuDisplayBackingOwner for T {}

impl core::fmt::Debug for GpuLinearDisplayBacking {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("GpuLinearDisplayBacking")
            .field("physical_addr", &self.physical_addr)
            .field("physical_segments", &self.physical_segments)
            .field("allocation_size", &self.allocation_size)
            .field("stride", &self.stride)
            .field("format", &self.format)
            .finish_non_exhaustive()
    }
}

impl PartialEq for GpuLinearDisplayBacking {
    fn eq(&self, other: &Self) -> bool {
        self.physical_addr == other.physical_addr
            && self.physical_segments == other.physical_segments
            && self.allocation_size == other.allocation_size
            && self.stride == other.stride
            && self.format == other.format
    }
}

impl Eq for GpuLinearDisplayBacking {}

impl GpuLinearDisplayBacking {
    /// Return the first physical byte of the linear image.
    ///
    /// # Returns
    ///
    /// Stable physical address retained by the presenting GPU image.
    pub const fn physical_addr(&self) -> usize {
        self.physical_addr
    }

    /// Return the ordered physical extents forming the linear allocation.
    pub fn physical_segments(&self) -> &[GpuBackingSegment] {
        &self.physical_segments
    }

    /// Return the allocated byte length of the backing.
    ///
    /// # Returns
    ///
    /// Page-rounded allocation size in bytes.
    pub const fn allocation_size(&self) -> u64 {
        self.allocation_size
    }

    /// Return the number of bytes between adjacent rows.
    ///
    /// # Returns
    ///
    /// Linear row stride in bytes.
    pub const fn stride(&self) -> u32 {
        self.stride
    }

    /// Return the framebuffer pixel format.
    ///
    /// # Returns
    ///
    /// Pixel format consumed by the display engine.
    pub const fn format(&self) -> PixelFormat {
        self.format
    }
}

impl GpuDisplayResource {
    /// Create a displayable GPU resource descriptor.
    ///
    /// # Arguments
    ///
    /// * `resource_id` - Backend resource identifier.
    /// * `width` - Resource width in pixels.
    /// * `height` - Resource height in pixels.
    /// * `backend_cookie` - Internal identity of the owning graphics backend.
    ///
    /// # Returns
    ///
    /// A GPU display resource descriptor.
    pub(crate) const fn new(
        resource_id: u32,
        width: u32,
        height: u32,
        backend_cookie: u64,
    ) -> Self {
        Self {
            resource_id,
            width,
            height,
            backend_cookie,
            linear_backing: None,
        }
    }

    /// Create a displayable linear GPU image descriptor.
    ///
    /// This constructor is the generic boundary used when a GPU producer and
    /// display controller are separate devices. It carries no GPU register or
    /// command-stream details.
    ///
    /// # Arguments
    ///
    /// * `physical_addr` - Stable physical address of pixel `(0, 0)`.
    /// * `allocation_size` - Allocated backing size in bytes.
    /// * `width` - Image width in pixels.
    /// * `height` - Image height in pixels.
    /// * `stride` - Number of bytes between adjacent rows.
    /// * `format` - Linear framebuffer pixel format.
    /// * `owner` - Strong lifetime owner retained while display hardware may fetch.
    ///
    /// # Returns
    ///
    /// A validated cross-device display descriptor, or an error for an invalid
    /// or undersized layout.
    pub fn new_linear(
        physical_addr: usize,
        allocation_size: u64,
        width: u32,
        height: u32,
        stride: u32,
        format: PixelFormat,
        owner: Arc<dyn GpuDisplayBackingOwner>,
    ) -> Result<Self, &'static str> {
        Self::new_linear_segments(
            Arc::from([GpuBackingSegment::new(
                physical_addr,
                usize::try_from(allocation_size)
                    .map_err(|_| "GPU display allocation does not fit kernel address size")?,
            )]),
            allocation_size,
            width,
            height,
            stride,
            format,
            owner,
        )
    }

    /// Create a displayable linear image backed by ordered physical extents.
    ///
    /// The extents are logically concatenated in slice order. The retained
    /// owner keeps every extent stable until display hardware releases it.
    pub fn new_linear_segments(
        physical_segments: Arc<[GpuBackingSegment]>,
        allocation_size: u64,
        width: u32,
        height: u32,
        stride: u32,
        format: PixelFormat,
        owner: Arc<dyn GpuDisplayBackingOwner>,
    ) -> Result<Self, &'static str> {
        let row_bytes = u64::from(width)
            .checked_mul(format.bytes_per_pixel() as u64)
            .ok_or("GPU display row size overflows")?;
        let required = u64::from(stride)
            .checked_mul(u64::from(height))
            .ok_or("GPU display backing size overflows")?;
        let mut segment_bytes = 0usize;
        for segment in physical_segments.iter().copied() {
            if segment.physical_addr() == 0
                || segment.length() == 0
                || segment
                    .physical_addr()
                    .checked_add(segment.length() - 1)
                    .is_none()
            {
                return Err("GPU linear display segment is invalid");
            }
            segment_bytes = segment_bytes
                .checked_add(segment.length())
                .ok_or("GPU linear display segment size overflows")?;
        }
        let allocation_size_usize = usize::try_from(allocation_size)
            .map_err(|_| "GPU display backing size does not fit the kernel address size")?;
        let physical_addr = physical_segments
            .first()
            .map(|segment| segment.physical_addr())
            .unwrap_or(0);
        if width == 0
            || height == 0
            || u64::from(stride) < row_bytes
            || allocation_size < required
            || segment_bytes < allocation_size_usize
        {
            return Err("GPU linear display backing is invalid");
        }
        Ok(Self {
            resource_id: 0,
            width,
            height,
            backend_cookie: 0,
            linear_backing: Some(GpuLinearDisplayBacking {
                physical_addr,
                physical_segments,
                allocation_size,
                stride,
                format,
                _owner: owner,
            }),
        })
    }

    /// Return the backend resource identifier.
    pub(crate) const fn resource_id(&self) -> u32 {
        self.resource_id
    }

    /// Return the resource width in pixels.
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Return the resource height in pixels.
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Return the opaque identity of the graphics backend that owns this resource.
    pub(crate) const fn backend_cookie(&self) -> u64 {
        self.backend_cookie
    }

    /// Return linear physical backing when this is a cross-device resource.
    ///
    /// # Returns
    ///
    /// Linear backing metadata, or `None` for backend-private resources such as
    /// VirtIO resource identifiers.
    pub fn linear_backing(&self) -> Option<GpuLinearDisplayBacking> {
        self.linear_backing.clone()
    }

    /// Get the full resource region.
    ///
    /// # Returns
    ///
    /// Region covering the whole resource.
    pub const fn full_region(&self) -> DisplayRegion {
        DisplayRegion::new(0, 0, self.width, self.height)
    }
}

/// Graphics device interface
///
/// This trait defines the interface for graphics devices.
/// It provides methods for framebuffer management and display operations.
pub trait GraphicsDevice: Device {
    /// Return whether this device is a firmware-provided boot framebuffer.
    ///
    /// Boot framebuffers remain available as a fallback until a native display
    /// driver successfully takes over the display pipeline.
    ///
    /// # Returns
    ///
    /// `true` for firmware boot framebuffers that may be retired by a native
    /// display driver, otherwise `false`.
    fn is_boot_framebuffer(&self) -> bool {
        false
    }

    /// Get the device display name
    fn get_display_name(&self) -> &'static str;

    /// Return the current display backlight level as a percentage.
    ///
    /// Implementations that expose a physical display backlight must return a
    /// value in the inclusive `0..=100` range. Devices without a controllable
    /// backlight retain backward-compatible behavior by returning an error.
    ///
    /// # Returns
    ///
    /// The current brightness percentage, or an error when this graphics
    /// device does not provide display-brightness control.
    fn get_brightness_percent(&self) -> Result<u8, &'static str> {
        Err("Display brightness control is not supported")
    }

    /// Set the display backlight level as a percentage.
    ///
    /// Implementations must reject values outside the inclusive `0..=100`
    /// range. Devices without a controllable backlight retain
    /// backward-compatible behavior by returning an error.
    ///
    /// # Arguments
    ///
    /// * `percent` - Requested display brightness in the inclusive `0..=100`
    ///   range, where `0` disables the backlight and `100` requests full
    ///   brightness.
    ///
    /// # Returns
    ///
    /// Success after applying the requested level, or an error when the level
    /// is invalid or this graphics device does not provide
    /// display-brightness control.
    fn set_brightness_percent(&self, _percent: u8) -> Result<(), &'static str> {
        Err("Display brightness control is not supported")
    }

    /// Get framebuffer configuration
    fn get_framebuffer_config(&self) -> Result<FramebufferConfig, &'static str>;

    /// Get framebuffer memory address
    fn get_framebuffer_address(&self) -> Result<usize, &'static str>;

    /// Get framebuffer configuration and memory address as one snapshot.
    ///
    /// # Returns
    ///
    /// Current framebuffer configuration and physical memory address, or an error
    /// if the framebuffer is not initialized.
    fn get_framebuffer_info(&self) -> Result<(FramebufferConfig, usize), &'static str> {
        let config = self.get_framebuffer_config()?;
        let physical_addr = self.get_framebuffer_address()?;
        Ok((config, physical_addr))
    }

    /// Present a framebuffer backing store to the display pipeline.
    ///
    /// # Arguments
    ///
    /// * `config` - Framebuffer configuration for the backing store.
    /// * `physical_addr` - Physical address of the framebuffer backing store.
    /// * `region` - Updated display region. Use `DisplayRegion::full(config)`
    ///   for a full-frame present.
    ///
    /// # Returns
    ///
    /// Success or an error describing why presentation failed.
    fn present_framebuffer_region(
        &self,
        config: &FramebufferConfig,
        physical_addr: usize,
        region: DisplayRegion,
    ) -> Result<(), &'static str>;

    /// Present the device's current framebuffer to the display pipeline.
    ///
    /// # Arguments
    ///
    /// * `region` - Updated display region. Use `DisplayRegion::full(config)`
    ///   for a full-frame present.
    ///
    /// # Returns
    ///
    /// Success or an error describing why presentation failed.
    fn present_current_framebuffer_region(
        &self,
        region: DisplayRegion,
    ) -> Result<(), &'static str> {
        let (config, physical_addr) = self.get_framebuffer_info()?;
        self.present_framebuffer_region(&config, physical_addr, region)
    }

    /// Return the number of CPU-mappable scanout buffers available for direct presentation.
    fn scanout_buffer_count(&self) -> usize {
        0
    }

    /// Return the scanout buffer currently displayed by the hardware.
    ///
    /// # Returns
    ///
    /// The front buffer index, or `None` when direct scanout is unsupported.
    fn front_scanout_buffer(&self) -> Option<usize> {
        None
    }

    /// Return configuration and physical address for one direct scanout buffer.
    ///
    /// # Arguments
    ///
    /// * `index` - Scanout buffer index.
    ///
    /// # Returns
    ///
    /// Buffer configuration and physical address, or an error when unsupported.
    fn get_scanout_buffer_info(
        &self,
        _index: usize,
    ) -> Result<(FramebufferConfig, usize), &'static str> {
        Err("Direct scanout buffers are not supported")
    }

    /// Present one direct scanout buffer atomically.
    ///
    /// # Arguments
    ///
    /// * `index` - Scanout buffer index previously exposed for drawing.
    ///
    /// # Returns
    ///
    /// Success after the hardware reports completion and the previous front
    /// buffer is safe to acquire for drawing.
    fn present_scanout_buffer(&self, _index: usize) -> Result<(), &'static str> {
        Err("Direct scanout buffers are not supported")
    }

    /// Present one direct scanout buffer with the regions modified by the producer.
    ///
    /// An empty region list means the complete buffer was modified.
    ///
    /// # Arguments
    ///
    /// * `index` - Scanout buffer index previously exposed for drawing.
    /// * `regions` - Modified framebuffer regions, or an empty slice for a full update.
    ///
    /// # Returns
    ///
    /// Success after the hardware reports completion and the previous front
    /// buffer is safe to acquire for drawing.
    fn present_scanout_buffer_regions(
        &self,
        index: usize,
        _regions: &[DisplayRegion],
    ) -> Result<(), &'static str> {
        self.present_scanout_buffer(index)
    }

    /// Present a GPU resource through the display pipeline.
    ///
    /// This is the display-side boundary for accelerated producers. GPU
    /// command submission updates the resource; this method selects the
    /// resource for scanout and presents the requested region.
    ///
    /// # Arguments
    ///
    /// * `resource` - GPU resource to present.
    /// * `region` - Updated resource region.
    ///
    /// # Returns
    ///
    /// Success or an error describing why presentation failed.
    fn present_gpu_resource_region(
        &self,
        _resource: GpuDisplayResource,
        _region: DisplayRegion,
    ) -> Result<(), &'static str> {
        Err("GPU resource presentation is not supported")
    }

    /// Present a GPU resource with explicit producer lifetime guarantees.
    ///
    /// Drivers that do not support direct cross-device scanout may ignore the
    /// options and use their normal presentation path. The default preserves
    /// the existing GPU presentation behavior.
    fn present_gpu_resource_region_with_options(
        &self,
        resource: GpuDisplayResource,
        region: DisplayRegion,
        _options: GpuPresentOptions,
    ) -> Result<(), &'static str> {
        self.present_gpu_resource_region(resource, region)
    }

    /// Present a whole GPU resource through the display pipeline.
    ///
    /// # Arguments
    ///
    /// * `resource` - GPU resource to present.
    ///
    /// # Returns
    ///
    /// Success or an error describing why presentation failed.
    fn present_gpu_resource(&self, resource: GpuDisplayResource) -> Result<(), &'static str> {
        let region = resource.full_region();
        self.present_gpu_resource_region(resource, region)
    }

    /// Initialize the graphics device (idempotent)
    fn init_graphics(&self) -> Result<(), &'static str>;

    /// Get display outputs provided by this device.
    ///
    /// Default returns empty — devices that don't support multi-output
    /// don't need to override this.
    fn get_outputs(&self) -> Vec<&dyn DisplayOutput> {
        Vec::new()
    }
}

/// A generic implementation of a graphics device
pub struct GenericGraphicsDevice {
    display_name: &'static str,
    config: Option<FramebufferConfig>,
    framebuffer_addr: Option<usize>,
    boot_framebuffer: bool,
}

impl GenericGraphicsDevice {
    pub fn new(display_name: &'static str) -> Self {
        Self {
            display_name,
            config: None,
            framebuffer_addr: None,
            boot_framebuffer: false,
        }
    }

    /// Set framebuffer configuration
    pub fn set_framebuffer_config(&mut self, config: FramebufferConfig) {
        self.config = Some(config);
    }

    /// Set framebuffer address
    pub fn set_framebuffer_address(&mut self, addr: usize) {
        self.framebuffer_addr = Some(addr);
    }

    /// Mark this generic device as a firmware boot framebuffer.
    ///
    /// # Arguments
    ///
    /// * `boot_framebuffer` - Whether native display takeover may retire it.
    pub fn set_boot_framebuffer(&mut self, boot_framebuffer: bool) {
        self.boot_framebuffer = boot_framebuffer;
    }
}

impl Device for GenericGraphicsDevice {
    fn device_type(&self) -> super::DeviceType {
        super::DeviceType::Graphics
    }

    fn name(&self) -> &'static str {
        self.display_name
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn as_graphics_device(&self) -> Option<&dyn GraphicsDevice> {
        Some(self)
    }
}

impl ControlOps for GenericGraphicsDevice {
    // Generic graphics devices don't support control operations by default
    fn control(&self, _command: u32, _arg: usize) -> Result<i32, &'static str> {
        Err("Control operations not supported")
    }
}

impl MemoryMappingOps for GenericGraphicsDevice {
    fn get_mapping_info(
        &self,
        _offset: usize,
        _length: usize,
    ) -> Result<crate::object::capability::MemoryMappingInfo, &'static str> {
        Err("Memory mapping not supported by this graphics device")
    }

    fn on_mapped(&self, _vaddr: usize, _paddr: usize, _length: usize, _offset: usize) {
        // Generic graphics devices don't support memory mapping
    }

    fn on_unmapped(&self, _vaddr: usize, _length: usize) {
        // Generic graphics devices don't support memory mapping
    }

    fn supports_mmap(&self) -> bool {
        false
    }
}

impl Selectable for GenericGraphicsDevice {
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

impl GraphicsDevice for GenericGraphicsDevice {
    fn is_boot_framebuffer(&self) -> bool {
        self.boot_framebuffer
    }

    fn get_display_name(&self) -> &'static str {
        self.display_name
    }

    fn get_framebuffer_config(&self) -> Result<FramebufferConfig, &'static str> {
        self.config.clone().ok_or("Framebuffer not configured")
    }

    fn get_framebuffer_address(&self) -> Result<usize, &'static str> {
        self.framebuffer_addr.ok_or("Framebuffer address not set")
    }

    fn present_framebuffer_region(
        &self,
        _config: &FramebufferConfig,
        _physical_addr: usize,
        _region: DisplayRegion,
    ) -> Result<(), &'static str> {
        Ok(())
    }

    fn init_graphics(&self) -> Result<(), &'static str> {
        // Generic implementation - no-op
        Ok(())
    }
}
