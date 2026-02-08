//! Graphics device interface
//!
//! This module defines the interface for graphics devices in the kernel.
//! It provides abstractions for framebuffer operations and graphics device management.

use alloc::{boxed::Box, vec::Vec};
use core::any::Any;
use spin::Mutex;

use alloc::sync::Arc;

use super::{Device, DeviceType, manager::DeviceManager};
use crate::object::capability::selectable::Selectable;
use crate::object::capability::{ControlOps, MemoryMappingOps};

pub mod framebuffer_device;
pub mod manager;

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
    /// 24-bit RGB (8 bits per channel)
    RGB888,
    /// 16-bit RGB (5-6-5 bits)
    RGB565,
}

impl PixelFormat {
    /// Get bytes per pixel for this format
    pub fn bytes_per_pixel(&self) -> usize {
        match self {
            PixelFormat::RGBA8888 | PixelFormat::BGRA8888 => 4,
            PixelFormat::RGB888 => 3,
            PixelFormat::RGB565 => 2,
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

/// Graphics operation requests
#[derive(Debug)]
pub enum GraphicsRequest {
    /// Get framebuffer configuration
    GetFramebufferConfig,
    /// Map framebuffer memory
    MapFramebuffer,
    /// Flush framebuffer changes to display
    FlushFramebuffer {
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    },
}

/// Result of graphics operations
#[derive(Debug)]
pub struct GraphicsResult {
    pub request: Box<GraphicsRequest>,
    pub result: Result<GraphicsResponse, &'static str>,
}

/// Response from graphics operations
#[derive(Debug)]
pub enum GraphicsResponse {
    /// Framebuffer configuration
    FramebufferConfig(FramebufferConfig),
    /// Framebuffer memory address
    FramebufferAddress(usize),
    /// Operation completed successfully
    Success,
}

/// Graphics device interface
///
/// This trait defines the minimal interface for graphics devices.
/// It provides fbdev-equivalent basic operations for framebuffer management.
/// OS-specific features (like DRM) are provided through separate capability traits.
pub trait GraphicsDevice: Device {
    /// Get the device display name
    fn get_display_name(&self) -> &'static str;

    /// Get current framebuffer configuration
    /// This returns the current scanout buffer configuration which may change
    /// after page flips or mode changes.
    fn get_framebuffer_config(&self) -> Result<FramebufferConfig, &'static str>;

    /// Get current framebuffer memory address
    /// Returns the physical address of the current front buffer.
    /// This address may change after a page flip operation.
    fn get_framebuffer_address(&self) -> Result<usize, &'static str>;

    /// Flush framebuffer region to display
    /// Ensures that writes to the framebuffer are visible to the display controller.
    fn flush_framebuffer(
        &self,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    ) -> Result<(), &'static str>;

    /// Initialize the graphics device (idempotent)
    fn init_graphics(&self) -> Result<(), &'static str>;
}

/// Capability trait for devices that support page flipping
///
/// Page flipping allows changing the displayed buffer without tearing.
/// Devices that implement this trait can switch between front and back buffers.
pub trait PageFlipCapable: GraphicsDevice {
    /// Check if page flip is currently possible
    fn can_page_flip(&self) -> bool;

    /// Queue a page flip to display the specified buffer
    ///
    /// # Arguments
    /// * `fb_id` - Identifier of the framebuffer to display
    /// * `crtc_id` - CRTC identifier (display controller)
    ///
    /// # Returns
    /// Ok if the flip was queued successfully. The actual flip happens asynchronously
    /// during the next vblank period.
    fn page_flip(&self, fb_id: u32, crtc_id: u32) -> Result<(), &'static str>;

    /// Get the current back buffer address for rendering
    /// Returns None if there's no back buffer available.
    fn get_back_buffer_address(&self) -> Option<usize>;

    /// Swap front and back buffers (synchronous operation)
    /// This is a simpler alternative to page_flip for devices that don't
    /// support async flipping.
    fn swap_buffers(&self) -> Result<(), &'static str>;
}

/// Capability trait for devices that support dumb buffers
///
/// Dumb buffers are simple CPU-accessible buffers that can be used as
/// framebuffers. They are typically used for basic display without 3D acceleration.
pub trait DumbBufferCapable: GraphicsDevice {
    /// Create a dumb buffer with specified dimensions
    ///
    /// # Arguments
    /// * `width` - Width in pixels
    /// * `height` - Height in pixels
    /// * `bpp` - Bits per pixel
    ///
    /// # Returns
    /// A handle to the created dumb buffer
    fn create_dumb_buffer(
        &self,
        width: u32,
        height: u32,
        bpp: u32,
    ) -> Result<DumbBufferHandle, &'static str>;

    /// Destroy a previously created dumb buffer
    fn destroy_dumb_buffer(&self, handle: DumbBufferHandle) -> Result<(), &'static str>;

    /// Map a dumb buffer for CPU access
    /// Returns the kernel virtual address for accessing the buffer.
    fn map_dumb_buffer(&self, handle: DumbBufferHandle) -> Result<usize, &'static str>;

    /// Unmap a dumb buffer
    fn unmap_dumb_buffer(&self, handle: DumbBufferHandle) -> Result<(), &'static str>;
}

/// Handle to a dumb buffer
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DumbBufferHandle {
    pub id: u32,
    pub width: u32,
    pub height: u32,
    pub pitch: u32,
    pub size: u64,
}

/// A generic implementation of a graphics device
pub struct GenericGraphicsDevice {
    display_name: &'static str,
    config: Option<FramebufferConfig>,
    framebuffer_addr: Option<usize>,
    request_queue: Mutex<Vec<Box<GraphicsRequest>>>,
}

impl GenericGraphicsDevice {
    pub fn new(display_name: &'static str) -> Self {
        Self {
            display_name,
            config: None,
            framebuffer_addr: None,
            request_queue: Mutex::new(Vec::new()),
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
    ) -> Result<(usize, usize, bool), &'static str> {
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

    fn flush_framebuffer(
        &self,
        _x: u32,
        _y: u32,
        _width: u32,
        _height: u32,
    ) -> Result<(), &'static str> {
        // Generic implementation - no-op
        Ok(())
    }

    fn init_graphics(&self) -> Result<(), &'static str> {
        // Generic implementation - no-op
        Ok(())
    }
}
