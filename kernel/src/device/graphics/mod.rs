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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuDisplayResource {
    /// Backend resource identifier.
    pub resource_id: u32,
    /// Resource width in pixels.
    pub width: u32,
    /// Resource height in pixels.
    pub height: u32,
}

impl GpuDisplayResource {
    /// Create a displayable GPU resource descriptor.
    ///
    /// # Arguments
    ///
    /// * `resource_id` - Backend resource identifier.
    /// * `width` - Resource width in pixels.
    /// * `height` - Resource height in pixels.
    ///
    /// # Returns
    ///
    /// A GPU display resource descriptor.
    pub const fn new(resource_id: u32, width: u32, height: u32) -> Self {
        Self {
            resource_id,
            width,
            height,
        }
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
    /// Get the device display name
    fn get_display_name(&self) -> &'static str;

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
    /// Success after the old front buffer is safe to reuse.
    fn present_scanout_buffer(&self, _index: usize) -> Result<(), &'static str> {
        Err("Direct scanout buffers are not supported")
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
        self.present_gpu_resource_region(resource, resource.full_region())
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
}

impl GenericGraphicsDevice {
    pub fn new(display_name: &'static str) -> Self {
        Self {
            display_name,
            config: None,
            framebuffer_addr: None,
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
