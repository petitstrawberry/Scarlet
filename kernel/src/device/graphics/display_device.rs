//! Modern display character device.
//!
//! This endpoint represents a scanout/display surface for display-system
//! clients that explicitly present damage regions. Legacy framebuffer
//! compatibility remains available through `/dev/fbX`.

extern crate alloc;

use crate::sync::IrqRwSpinLock;
use alloc::{collections::BTreeMap, sync::Arc, vec, vec::Vec};
use core::any::Any;

use super::{
    FramebufferConfig, GpuPresentOptions, PixelFormat, manager::FramebufferResource,
    output::DisplayRegion,
};
use crate::device::{Device, DeviceType, char::CharDevice, manager::DeviceManager};
use crate::library::std::usercopy::copy_from_user;
use crate::object::capability::selectable::{ReadyInterest, SelectWaitOutcome, Selectable};
use crate::object::capability::{ControlOps, MemoryMappingOps};
use crate::vm::addr::phys_to_virt;

/// Display character device control commands.
///
/// These are Scarlet display controls, not Linux framebuffer compatibility
/// ioctls. `/dev/fbX` owns the `FBIO_*` namespace.
pub mod display_commands {
    /// Get display surface information.
    pub const DISPLAY_GET_INFO: u32 = 0x5000;
    /// Present the whole display surface.
    pub const DISPLAY_PRESENT: u32 = 0x5001;
    /// Present a display surface region.
    pub const DISPLAY_PRESENT_REGION: u32 = 0x5002;
    /// Get direct scanout swapchain information.
    pub const DISPLAY_GET_SWAPCHAIN: u32 = 0x5003;
    /// Present one direct scanout buffer.
    pub const DISPLAY_PRESENT_BUFFER: u32 = 0x5004;
    /// Present one GPU image capability through this display device.
    pub const DISPLAY_PRESENT_IMAGE: u32 = 0x5005;
}

/// 32-bit RGBA pixel layout.
pub const DISPLAY_PIXEL_FORMAT_RGBA8888: u32 = 1;
/// 32-bit BGRA pixel layout.
pub const DISPLAY_PIXEL_FORMAT_BGRA8888: u32 = 2;
/// 32-bit XRGB pixel layout.
pub const DISPLAY_PIXEL_FORMAT_XRGB8888: u32 = 3;
/// 32-bit XBGR pixel layout.
pub const DISPLAY_PIXEL_FORMAT_XBGR8888: u32 = 4;
/// 32-bit XRGB2101010 pixel layout.
pub const DISPLAY_PIXEL_FORMAT_XRGB2101010: u32 = 5;
/// 24-bit RGB pixel layout.
pub const DISPLAY_PIXEL_FORMAT_RGB888: u32 = 6;
/// 16-bit RGB565 pixel layout.
pub const DISPLAY_PIXEL_FORMAT_RGB565: u32 = 7;
/// 16-bit ARGB1555 pixel layout.
pub const DISPLAY_PIXEL_FORMAT_ARGB1555: u32 = 8;
/// 16-bit XRGB1555 pixel layout.
pub const DISPLAY_PIXEL_FORMAT_XRGB1555: u32 = 9;

/// Maximum number of damage rectangles carried by one present request.
pub const DISPLAY_MAX_DAMAGE_RECTS: usize = 32;

/// Display surface information.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct DisplayInfo {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Bytes per row.
    pub stride: u32,
    /// Pixel format, one of `DISPLAY_PIXEL_FORMAT_*`.
    pub format: u32,
    /// Page-aligned size of the mappable display backing store.
    pub buffer_len: u32,
    /// Opaque identifier for the current mappable backing store.
    ///
    /// This value changes when the display surface's mapped backing changes,
    /// even if `buffer_len` remains the same.
    pub backing_id: usize,
}

/// Region argument for DISPLAY_PRESENT_REGION.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct DisplayPresentRegion {
    /// Left edge in pixels.
    pub x: u32,
    /// Top edge in pixels.
    pub y: u32,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

/// Direct scanout swapchain information.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct DisplaySwapchainInfo {
    /// Number of scanout buffers.
    pub buffer_count: u32,
    /// Bytes in each mappable buffer.
    pub buffer_len: u32,
    /// Scanout buffer currently displayed by the hardware.
    pub front_buffer: u32,
    /// Reserved for ABI alignment and future flags.
    pub reserved: u32,
    /// mmap offset of the first direct scanout buffer.
    pub first_buffer_offset: usize,
}

/// Argument for `DISPLAY_PRESENT_BUFFER`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct DisplayPresentBuffer {
    /// Direct scanout buffer index.
    pub index: u32,
    /// Reserved for future fence flags.
    pub flags: u32,
    /// Number of valid entries in `damage`.
    ///
    /// Zero means the complete buffer was modified.
    pub damage_count: u32,
    /// Reserved for ABI alignment.
    pub reserved: u32,
    /// User pointer to `damage_count` [`DisplayPresentRegion`] entries.
    pub damage_ptr: usize,
}

/// Argument for `DISPLAY_PRESENT_IMAGE`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct DisplayPresentImage {
    /// GPU image capability handle owned by the current task.
    pub image_handle: u32,
    /// `DISPLAY_PRESENT_IMAGE_FLAG_*` values.
    pub flags: u32,
    /// Left edge in pixels when not presenting the full image.
    pub x: u32,
    /// Top edge in pixels when not presenting the full image.
    pub y: u32,
    /// Region width in pixels when not presenting the full image.
    pub width: u32,
    /// Region height in pixels when not presenting the full image.
    pub height: u32,
}

/// Present the full GPU image and require zero region fields.
pub const DISPLAY_PRESENT_IMAGE_FLAG_FULL_FRAME: u32 = 1 << 0;
/// The producer will not rewrite this image until another image is presented.
pub const DISPLAY_PRESENT_IMAGE_FLAG_SWAPCHAIN_BUFFER: u32 = 1 << 1;
/// All currently defined GPU image presentation flags.
pub const DISPLAY_PRESENT_IMAGE_FLAGS_VALID: u32 =
    DISPLAY_PRESENT_IMAGE_FLAG_FULL_FRAME | DISPLAY_PRESENT_IMAGE_FLAG_SWAPCHAIN_BUFFER;

#[derive(Debug, Clone)]
struct DisplayBackingInfo {
    config: FramebufferConfig,
    physical_addr: usize,
    size: usize,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
struct DisplayMapping {
    vaddr: usize,
    length: usize,
}

/// Modern display device backed by a graphics scanout resource.
///
/// The device deliberately has a separate `/dev/displayX` namespace from
/// `/dev/fbX`, even though the current CPU-composited backing store is still a
/// mappable framebuffer. Presentation is expected to happen through explicit
/// display controls rather than framebuffer compatibility ioctls.
pub struct DisplayCharDevice {
    /// The graphics resource this display surface presents.
    fb_resource: Arc<FramebufferResource>,
    /// Track mappings for testing and diagnostics.
    mappings: IrqRwSpinLock<BTreeMap<usize, DisplayMapping>>,
    #[cfg(test)]
    device_manager_addr: Option<usize>,
}

impl DisplayCharDevice {
    /// Create a new display character device.
    ///
    /// # Arguments
    ///
    /// * `fb_resource` - The graphics resource backing this display surface.
    ///
    /// # Returns
    ///
    /// A new DisplayCharDevice instance.
    pub fn new(fb_resource: Arc<FramebufferResource>) -> Self {
        Self {
            fb_resource,
            mappings: IrqRwSpinLock::new(BTreeMap::new()),
            #[cfg(test)]
            device_manager_addr: None,
        }
    }

    /// Create a new display character device with an explicit DeviceManager.
    ///
    /// # Arguments
    ///
    /// * `fb_resource` - The graphics resource backing this display surface.
    /// * `device_manager` - Device manager used for tests.
    ///
    /// # Returns
    ///
    /// A new DisplayCharDevice instance.
    #[cfg(test)]
    pub fn new_with_device_manager(
        fb_resource: Arc<FramebufferResource>,
        device_manager: &DeviceManager,
    ) -> Self {
        Self {
            fb_resource,
            mappings: IrqRwSpinLock::new(BTreeMap::new()),
            device_manager_addr: Some(device_manager as *const DeviceManager as usize),
        }
    }

    #[cfg(test)]
    fn device_manager(&self) -> &DeviceManager {
        match self.device_manager_addr {
            Some(device_manager_addr) => unsafe { &*(device_manager_addr as *const DeviceManager) },
            None => DeviceManager::get_manager(),
        }
    }

    #[cfg(not(test))]
    fn device_manager(&self) -> &DeviceManager {
        DeviceManager::get_manager()
    }

    fn page_aligned_size(size: usize) -> Result<usize, &'static str> {
        size.checked_add(crate::environment::PAGE_SIZE - 1)
            .map(|size| size & !(crate::environment::PAGE_SIZE - 1))
            .ok_or("Display backing size overflow")
    }

    fn current_backing_info(&self) -> Result<DisplayBackingInfo, &'static str> {
        if let Some(device) = self
            .device_manager()
            .get_device(self.fb_resource.source_device_id)
        {
            if let Some(graphics_device) = device.as_graphics_device() {
                let (config, physical_addr) = graphics_device.get_framebuffer_info()?;
                let size = Self::page_aligned_size(config.size())?;
                return Ok(DisplayBackingInfo {
                    config,
                    physical_addr,
                    size,
                });
            }
        }

        Ok(DisplayBackingInfo {
            config: self.fb_resource.config.clone(),
            physical_addr: self.fb_resource.physical_addr,
            size: self.fb_resource.size,
        })
    }

    fn display_format(format: PixelFormat) -> u32 {
        match format {
            PixelFormat::RGBA8888 => DISPLAY_PIXEL_FORMAT_RGBA8888,
            PixelFormat::BGRA8888 => DISPLAY_PIXEL_FORMAT_BGRA8888,
            PixelFormat::XRGB8888 => DISPLAY_PIXEL_FORMAT_XRGB8888,
            PixelFormat::XBGR8888 => DISPLAY_PIXEL_FORMAT_XBGR8888,
            PixelFormat::XRGB2101010 => DISPLAY_PIXEL_FORMAT_XRGB2101010,
            PixelFormat::RGB888 => DISPLAY_PIXEL_FORMAT_RGB888,
            PixelFormat::RGB565 => DISPLAY_PIXEL_FORMAT_RGB565,
            PixelFormat::ARGB1555 => DISPLAY_PIXEL_FORMAT_ARGB1555,
            PixelFormat::XRGB1555 => DISPLAY_PIXEL_FORMAT_XRGB1555,
        }
    }

    fn current_display_info(&self) -> Result<DisplayInfo, &'static str> {
        let backing = self.current_backing_info()?;
        Ok(DisplayInfo {
            width: backing.config.width,
            height: backing.config.height,
            stride: backing.config.stride,
            format: Self::display_format(backing.config.format),
            buffer_len: backing.size as u32,
            backing_id: backing.physical_addr,
        })
    }

    fn translated_ptr(arg: usize) -> Result<usize, &'static str> {
        if arg == 0 {
            return Err("Invalid argument pointer");
        }

        if let Some(current_task) = crate::task::mytask() {
            current_task
                .vm_manager
                .translate_to_kva(arg)
                .ok_or("Invalid user pointer - not mapped")
        } else {
            Ok(arg)
        }
    }

    fn handle_get_info(&self, arg: usize) -> Result<i32, &'static str> {
        let target_ptr = Self::translated_ptr(arg)?;
        let info = self.current_display_info()?;

        // SAFETY: target_ptr is either a translated user pointer or a kernel
        // pointer supplied by in-kernel tests.
        unsafe {
            core::ptr::write(target_ptr as *mut DisplayInfo, info);
        }

        Ok(0)
    }

    fn handle_present(&self) -> Result<i32, &'static str> {
        let backing = self.current_backing_info()?;
        if backing.physical_addr == 0 {
            return Err("Invalid display backing address");
        }

        self.present_region(DisplayRegion::full(&backing.config))?;
        Ok(0)
    }

    fn handle_present_region(&self, arg: usize) -> Result<i32, &'static str> {
        let target_ptr = Self::translated_ptr(arg)?;

        // SAFETY: target_ptr is either a translated user pointer or a kernel
        // pointer supplied by in-kernel tests.
        let region = unsafe { core::ptr::read(target_ptr as *const DisplayPresentRegion) };

        self.present_region(DisplayRegion::new(
            region.x,
            region.y,
            region.width,
            region.height,
        ))?;
        Ok(0)
    }

    fn handle_get_swapchain(&self, arg: usize) -> Result<i32, &'static str> {
        let target_ptr = Self::translated_ptr(arg)?;
        let device = self
            .device_manager()
            .get_device(self.fb_resource.source_device_id)
            .ok_or("Display source device not found")?;
        let graphics = device
            .as_graphics_device()
            .ok_or("Display source device is not graphics-capable")?;
        let count = graphics.scanout_buffer_count();
        if count == 0 {
            return Err("Display swapchain is not supported");
        }
        let (config, _) = graphics.get_scanout_buffer_info(0)?;
        let front_buffer = graphics
            .front_scanout_buffer()
            .ok_or("Display front scanout buffer is unavailable")?;
        if front_buffer >= count {
            return Err("Display front scanout buffer is invalid");
        }
        let buffer_len = Self::page_aligned_size(config.size())?;
        let buffer_count = u32::try_from(count).map_err(|_| "Display scanout count exceeds ABI")?;
        let buffer_len_u32 =
            u32::try_from(buffer_len).map_err(|_| "Display scanout size exceeds ABI")?;
        let front_buffer =
            u32::try_from(front_buffer).map_err(|_| "Display front scanout exceeds ABI")?;
        let info = DisplaySwapchainInfo {
            buffer_count,
            buffer_len: buffer_len_u32,
            front_buffer,
            reserved: 0,
            first_buffer_offset: buffer_len,
        };
        // SAFETY: target_ptr is a translated caller-provided output pointer.
        unsafe { core::ptr::write(target_ptr as *mut DisplaySwapchainInfo, info) };
        Ok(0)
    }

    fn handle_present_buffer(&self, arg: usize) -> Result<i32, &'static str> {
        if arg == 0 {
            return Err("Invalid display present pointer");
        }
        let request = if let Some(task) = crate::task::mytask() {
            let mut request = core::mem::MaybeUninit::<DisplayPresentBuffer>::uninit();
            // SAFETY: the byte slice covers the complete uninitialized request,
            // which copy_from_user initializes before assume_init.
            let bytes = unsafe {
                core::slice::from_raw_parts_mut(
                    request.as_mut_ptr() as *mut u8,
                    core::mem::size_of::<DisplayPresentBuffer>(),
                )
            };
            copy_from_user(&task, arg, bytes)
                .map_err(|_| "Failed to copy display present from user")?;
            // SAFETY: copy_from_user initialized every byte of the request.
            unsafe { request.assume_init() }
        } else {
            // SAFETY: in-kernel callers provide a valid request pointer.
            unsafe { core::ptr::read(arg as *const DisplayPresentBuffer) }
        };
        let damage_count = request.damage_count as usize;
        if damage_count > DISPLAY_MAX_DAMAGE_RECTS {
            return Err("Display present has too many damage rectangles");
        }
        let device = self
            .device_manager()
            .get_device(self.fb_resource.source_device_id)
            .ok_or("Display source device not found")?;
        let graphics = device
            .as_graphics_device()
            .ok_or("Display source device is not graphics-capable")?;
        if damage_count != 0 && request.damage_ptr == 0 {
            return Err("Display present has a null damage pointer");
        }
        let damage_bytes_len = damage_count
            .checked_mul(core::mem::size_of::<DisplayPresentRegion>())
            .ok_or("Display damage size overflow")?;
        let mut damage_bytes = vec![0u8; damage_bytes_len];
        if damage_count != 0 {
            let task = crate::task::mytask().ok_or("Display present has no current task")?;
            copy_from_user(&task, request.damage_ptr, &mut damage_bytes)
                .map_err(|_| "Failed to copy display damage from user")?;
        }

        let mut regions = Vec::with_capacity(damage_count);
        for bytes in damage_bytes.chunks_exact(core::mem::size_of::<DisplayPresentRegion>()) {
            // SAFETY: each chunk has the exact structure size and read_unaligned
            // does not require the byte vector to provide structure alignment.
            let damage =
                unsafe { core::ptr::read_unaligned(bytes.as_ptr() as *const DisplayPresentRegion) };
            regions.push(DisplayRegion::new(
                damage.x,
                damage.y,
                damage.width,
                damage.height,
            ));
        }
        graphics.present_scanout_buffer_regions(request.index as usize, &regions)?;
        Ok(0)
    }

    fn handle_present_image(&self, arg: usize) -> Result<i32, &'static str> {
        if arg == 0 {
            return Err("Invalid display image present pointer");
        }
        let task = crate::task::mytask().ok_or("Display image present has no current task")?;
        let mut request = core::mem::MaybeUninit::<DisplayPresentImage>::uninit();
        // SAFETY: the byte slice covers the complete uninitialized request,
        // which copy_from_user initializes before assume_init.
        let bytes = unsafe {
            core::slice::from_raw_parts_mut(
                request.as_mut_ptr() as *mut u8,
                core::mem::size_of::<DisplayPresentImage>(),
            )
        };
        copy_from_user(&task, arg, bytes)
            .map_err(|_| "Failed to copy display image present from user")?;
        // SAFETY: copy_from_user initialized every byte of the request.
        let request = unsafe { request.assume_init() };
        if request.image_handle == 0 || request.flags & !DISPLAY_PRESENT_IMAGE_FLAGS_VALID != 0 {
            return Err("Display image present request is invalid");
        }
        let full_frame = request.flags & DISPLAY_PRESENT_IMAGE_FLAG_FULL_FRAME != 0;
        if full_frame
            && (request.x != 0 || request.y != 0 || request.width != 0 || request.height != 0)
        {
            return Err("Full-frame display image present has region fields");
        }
        if !full_frame && (request.width == 0 || request.height == 0) {
            return Err("Display image present region is empty");
        }
        let image_owner = task
            .handle_table
            .get_arc_clone(request.image_handle)
            .ok_or("Display image handle is invalid")?;
        let image = image_owner
            .as_gpu()
            .and_then(crate::device::gpu::GpuObject::as_image)
            .ok_or("Display image handle is not a GPU image")?;
        let resource = image
            .display_resource()
            .ok_or("GPU image is not presentable")?;
        let region = if full_frame {
            resource.full_region()
        } else {
            if request.x >= resource.width() || request.y >= resource.height() {
                return Err("Display image region lies outside the image");
            }
            DisplayRegion::new(
                request.x,
                request.y,
                request.width.min(resource.width() - request.x),
                request.height.min(resource.height() - request.y),
            )
        };
        let device = self
            .device_manager()
            .get_device(self.fb_resource.source_device_id)
            .ok_or("Display source device not found")?;
        let graphics = device
            .as_graphics_device()
            .ok_or("Display source device is not graphics-capable")?;
        // `image_owner` remains live until this synchronous presentation returns.
        let options = if request.flags & DISPLAY_PRESENT_IMAGE_FLAG_SWAPCHAIN_BUFFER != 0 {
            GpuPresentOptions::swapchain_buffer()
        } else {
            GpuPresentOptions::default()
        };
        graphics.present_gpu_resource_region_with_options(resource, region, options)?;
        Ok(0)
    }

    fn present_region(&self, region: DisplayRegion) -> Result<(), &'static str> {
        let device = self
            .device_manager()
            .get_device(self.fb_resource.source_device_id)
            .ok_or("Display source device not found")?;
        let graphics_device = device
            .as_graphics_device()
            .ok_or("Display source device is not graphics-capable")?;
        let (config, physical_addr) = graphics_device.get_framebuffer_info()?;
        if physical_addr == 0 {
            return Err("Invalid display backing address");
        }

        let region = DisplayRegion::new(
            region.x.min(config.width),
            region.y.min(config.height),
            region.width.min(config.width.saturating_sub(region.x)),
            region.height.min(config.height.saturating_sub(region.y)),
        );
        graphics_device.present_current_framebuffer_region(region)
    }
}

impl Device for DisplayCharDevice {
    fn device_type(&self) -> DeviceType {
        DeviceType::Char
    }

    fn name(&self) -> &'static str {
        "display"
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn as_char_device(&self) -> Option<&dyn CharDevice> {
        Some(self)
    }
}

impl CharDevice for DisplayCharDevice {
    fn read_byte(&self) -> Option<u8> {
        None
    }

    fn write_byte(&self, _byte: u8) -> Result<(), &'static str> {
        Err("write_byte is not supported - use write_at through DevFileObject instead")
    }

    fn write(&self, _buffer: &[u8]) -> Result<usize, &'static str> {
        Err("write is not supported - use write_at through DevFileObject instead")
    }

    fn can_read(&self) -> bool {
        match self.current_backing_info() {
            Ok(info) => info.physical_addr != 0 && info.size > 0,
            Err(_) => false,
        }
    }

    fn can_write(&self) -> bool {
        match self.current_backing_info() {
            Ok(info) => info.physical_addr != 0 && info.size > 0,
            Err(_) => false,
        }
    }

    fn read_at(&self, position: u64, buffer: &mut [u8]) -> Result<usize, &'static str> {
        let info = self.current_backing_info()?;
        if info.physical_addr == 0 {
            return Err("Invalid display backing address");
        }

        let logical_size = info.config.size();
        let start_pos = position as usize;
        if start_pos >= logical_size {
            return Ok(0);
        }

        let to_read = buffer.len().min(logical_size - start_pos);
        unsafe {
            let src_ptr = (phys_to_virt(info.physical_addr) as *const u8).add(start_pos);
            for i in 0..to_read {
                buffer[i] = core::ptr::read_volatile(src_ptr.add(i));
            }
        }

        Ok(to_read)
    }

    fn write_at(&self, position: u64, buffer: &[u8]) -> Result<usize, &'static str> {
        let info = self.current_backing_info()?;
        if info.physical_addr == 0 {
            return Err("Invalid display backing address");
        }

        let logical_size = info.config.size();
        let start_pos = position as usize;
        if start_pos >= logical_size {
            return Err("Position beyond display backing size");
        }

        let to_write = buffer.len().min(logical_size - start_pos);
        unsafe {
            let dst_ptr = (phys_to_virt(info.physical_addr) as *mut u8).add(start_pos);
            for i in 0..to_write {
                core::ptr::write_volatile(dst_ptr.add(i), buffer[i]);
            }
        }

        Ok(to_write)
    }

    fn can_seek(&self) -> bool {
        true
    }
}

impl ControlOps for DisplayCharDevice {
    fn control(&self, command: u32, arg: usize) -> Result<i32, &'static str> {
        use display_commands::*;

        match command {
            DISPLAY_GET_INFO => self.handle_get_info(arg),
            DISPLAY_PRESENT => self.handle_present(),
            DISPLAY_PRESENT_REGION => self.handle_present_region(arg),
            DISPLAY_GET_SWAPCHAIN => self.handle_get_swapchain(arg),
            DISPLAY_PRESENT_BUFFER => self.handle_present_buffer(arg),
            DISPLAY_PRESENT_IMAGE => self.handle_present_image(arg),
            _ => Err("Unsupported display control command"),
        }
    }

    fn supported_control_commands(&self) -> Vec<(u32, &'static str)> {
        use display_commands::*;
        vec![
            (DISPLAY_GET_INFO, "Get display surface information"),
            (DISPLAY_PRESENT, "Present whole display surface"),
            (DISPLAY_PRESENT_REGION, "Present display surface region"),
            (DISPLAY_GET_SWAPCHAIN, "Get direct scanout swapchain"),
            (DISPLAY_PRESENT_BUFFER, "Present direct scanout buffer"),
            (DISPLAY_PRESENT_IMAGE, "Present a GPU image capability"),
        ]
    }
}

impl MemoryMappingOps for DisplayCharDevice {
    fn get_mapping_info(
        &self,
        offset: usize,
        length: usize,
    ) -> Result<crate::object::capability::MemoryMappingInfo, &'static str> {
        let info = self.current_backing_info()?;

        if offset % crate::environment::PAGE_SIZE != 0 {
            return Err("Display mmap offset must be page-aligned");
        }
        if length % crate::environment::PAGE_SIZE != 0 {
            return Err("Display mmap length must be page-aligned");
        }
        if info.physical_addr == 0 || info.size == 0 {
            return Err("Invalid display backing configuration");
        }
        if info.physical_addr % crate::environment::PAGE_SIZE != 0 {
            return Err("Display backing physical address must be page-aligned");
        }
        if offset >= info.size {
            let device = self
                .device_manager()
                .get_device(self.fb_resource.source_device_id)
                .ok_or("Display source device not found")?;
            let graphics = device
                .as_graphics_device()
                .ok_or("Display source device is not graphics-capable")?;
            let relative = offset - info.size;
            let index = relative / info.size;
            let buffer_offset = relative % info.size;
            let (_, physical_addr) = graphics.get_scanout_buffer_info(index)?;
            if length > info.size - buffer_offset {
                return Err("Requested length exceeds scanout buffer size");
            }
            let mapping_paddr = physical_addr
                .checked_add(buffer_offset)
                .ok_or("Display scanout physical address overflow")?;
            return Ok(
                crate::object::capability::MemoryMappingInfo::new(mapping_paddr, 0x3, true)
                    .with_memory_attribute(crate::vm::vmem::MemoryAttribute::DeviceBurstable),
            );
        }

        let available_size = info.size - offset;
        if length > available_size {
            return Err("Requested length exceeds available display backing size");
        }

        let mapping_paddr = info
            .physical_addr
            .checked_add(offset)
            .ok_or("Display backing physical address overflow")?;
        Ok(
            crate::object::capability::MemoryMappingInfo::new(mapping_paddr, 0x3, true)
                .with_memory_attribute(crate::vm::vmem::MemoryAttribute::DeviceBurstable),
        )
    }

    fn on_mapped(&self, vaddr: usize, _paddr: usize, length: usize, _offset: usize) {
        self.mappings
            .write()
            .insert(vaddr, DisplayMapping { vaddr, length });
    }

    fn on_unmapped(&self, vaddr: usize, _length: usize) {
        self.mappings.write().remove(&vaddr);
    }

    fn supports_mmap(&self) -> bool {
        match self.current_backing_info() {
            Ok(info) => info.physical_addr != 0 && info.size > 0,
            Err(_) => false,
        }
    }

    fn mmap_owner_name(&self) -> alloc::string::String {
        alloc::string::String::from("display")
    }
}

impl Selectable for DisplayCharDevice {
    fn current_ready(
        &self,
        interest: ReadyInterest,
    ) -> crate::object::capability::selectable::ReadySet {
        let mut set = crate::object::capability::selectable::ReadySet::none();
        if interest.read {
            set.read = self.can_read();
        }
        if interest.write {
            set.write = self.can_write();
        }
        set
    }

    fn wait_until_ready(
        &self,
        _interest: ReadyInterest,
        _trapframe: &mut crate::arch::Trapframe,
        _timeout_ticks: Option<u64>,
        _min_wait_ticks: u64,
    ) -> SelectWaitOutcome {
        SelectWaitOutcome::Ready
    }
}
