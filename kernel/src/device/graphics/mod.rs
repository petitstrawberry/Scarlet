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
pub mod drm_device;
pub mod buffer;

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
/// This trait defines the minimal OS-independent interface for graphics devices.
/// It provides methods for fbdev-level framebuffer management and display operations.
/// 
/// ## Design Philosophy
/// 
/// The GraphicsDevice trait is designed to be minimal and OS-independent, providing
/// only the essential operations needed for basic framebuffer access. More advanced
/// features (like page flipping, 3D rendering, etc.) are provided through separate
/// capability traits that devices can optionally implement.
/// 
/// ## Dynamic Framebuffer Address
/// 
/// Modern GPUs (both dGPU and iGPU) manage framebuffer memory as device-controlled
/// buffers (in VRAM or system RAM). The OS must treat the "current scanout buffer
/// address" as a variable value that can change. The `get_framebuffer_address()`
/// method returns the current active framebuffer address, which the driver manages.
/// From the CPU's perspective, this is always just a memory access - whether it's
/// VRAM or system RAM is abstracted by the driver.
/// 
/// ## Capability-Based Extension
/// 
/// Additional capabilities should be exposed through separate traits like
/// `PageFlipCapable`, `RenderDevice`, etc. This allows:
/// - Devices to implement only the features they support
/// - OS code to detect and use advanced features when available
/// - Fallback implementations for devices without native support
pub trait GraphicsDevice: Device {
    /// Get the device display name
    fn get_display_name(&self) -> &'static str;

    /// Get framebuffer configuration
    /// 
    /// Returns the current framebuffer configuration including resolution,
    /// pixel format, and stride. This configuration may be updated by the
    /// device driver when the display mode changes.
    fn get_framebuffer_config(&self) -> Result<FramebufferConfig, &'static str>;
    
    /// Get current framebuffer memory address
    /// 
    /// Returns the physical address of the current active framebuffer.
    /// This address may change when the device performs operations like
    /// page flipping. Callers should query this address whenever they
    /// need to access the framebuffer, rather than caching it.
    /// 
    /// The returned address can be mapped to virtual memory for CPU access.
    fn get_framebuffer_address(&self) -> Result<usize, &'static str>;

    /// Flush framebuffer region to display
    /// 
    /// Ensures that any pending writes to the framebuffer memory are
    /// visible on the display. This may involve:
    /// - Flushing CPU caches
    /// - Notifying the display controller of changes
    /// - Triggering a display update cycle
    /// 
    /// # Arguments
    /// 
    /// * `x` - X coordinate of the region to flush
    /// * `y` - Y coordinate of the region to flush
    /// * `width` - Width of the region to flush
    /// * `height` - Height of the region to flush
    fn flush_framebuffer(&self, x: u32, y: u32, width: u32, height: u32) -> Result<(), &'static str>;
    
    /// Initialize the graphics device (idempotent)
    /// 
    /// Performs any necessary initialization for the graphics device.
    /// This method is idempotent - calling it multiple times should
    /// be safe and should not cause issues.
    fn init_graphics(&self) -> Result<(), &'static str>;
}

/// Page flip capability for graphics devices
/// 
/// This trait represents the ability to perform hardware-accelerated page flipping,
/// where the display controller can be instructed to switch the scanout source from
/// one framebuffer to another without copying data.
/// 
/// ## Page Flipping vs. Memcpy
/// 
/// Devices that don't implement this trait can still support page flip semantics
/// through a fallback mechanism that copies from a back buffer to the front buffer
/// and then flushes. However, native page flipping is more efficient as it:
/// - Avoids memory copy operations
/// - Provides tear-free updates
/// - Can be synchronized with vertical blank (vsync)
/// 
/// ## Usage
/// 
/// ```rust,ignore
/// // Check if device supports page flipping via downcasting
/// use core::any::Any;
/// 
/// if let Some(any_device) = device.as_any().downcast_ref::<SpecificDeviceType>() {
///     if let Some(page_flip_device) = any_device as &dyn PageFlipCapable {
///         // Use hardware page flip
///         page_flip_device.page_flip(buffer_id)?;
///     }
/// } else {
///     // Fallback to memcpy + flush
///     let config = device.get_framebuffer_config()?;
///     // ... copy buffer to framebuffer ...
///     device.flush_framebuffer(0, 0, config.width, config.height)?;
/// }
/// ```
/// 
/// Note: A future enhancement may add an `as_page_flip_capable()` helper method
/// to the Device trait for more convenient capability detection.
pub trait PageFlipCapable: GraphicsDevice {
    /// Perform a page flip operation
    /// 
    /// Switches the display controller's scanout source to the specified buffer.
    /// This operation should ideally be synchronized with vertical blank to avoid
    /// tearing.
    /// 
    /// # Arguments
    /// 
    /// * `buffer_id` - The ID of the buffer to flip to (device-specific)
    /// 
    /// # Returns
    /// 
    /// Result indicating success or failure
    fn page_flip(&self, buffer_id: u32) -> Result<(), &'static str>;
    
    /// Create a new buffer for page flipping
    /// 
    /// Allocates a new buffer that can be used as a flip target.
    /// Returns a device-specific buffer ID.
    /// 
    /// # Arguments
    /// 
    /// * `width` - Width of the buffer in pixels
    /// * `height` - Height of the buffer in pixels
    /// * `format` - Pixel format for the buffer
    /// 
    /// # Returns
    /// 
    /// Result containing the buffer ID or an error
    fn create_flip_buffer(&self, width: u32, height: u32, format: PixelFormat) -> Result<u32, &'static str>;
    
    /// Destroy a buffer created for page flipping
    /// 
    /// # Arguments
    /// 
    /// * `buffer_id` - The ID of the buffer to destroy
    fn destroy_flip_buffer(&self, buffer_id: u32) -> Result<(), &'static str>;
    
    /// Get the physical address of a flip buffer
    /// 
    /// Returns the physical address where the specified buffer's memory
    /// can be accessed. This can be used to map the buffer for CPU access.
    /// 
    /// # Arguments
    /// 
    /// * `buffer_id` - The ID of the buffer
    /// 
    /// # Returns
    /// 
    /// Result containing the physical address or an error
    fn get_flip_buffer_address(&self, buffer_id: u32) -> Result<usize, &'static str>;
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

impl Selectable for GenericGraphicsDevice {}

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
