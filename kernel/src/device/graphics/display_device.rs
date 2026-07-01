//! Modern display character device.
//!
//! This endpoint represents a scanout/display surface for display-system
//! clients that explicitly present damage regions. Legacy framebuffer
//! compatibility remains available through `/dev/fbX`.

extern crate alloc;

use alloc::{collections::BTreeMap, sync::Arc, vec, vec::Vec};
use core::any::Any;
use spin::RwLock;

use super::{FramebufferConfig, PixelFormat, manager::FramebufferResource, output::DisplayRegion};
use crate::device::{Device, DeviceType, char::CharDevice, manager::DeviceManager};
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
    mappings: RwLock<BTreeMap<usize, DisplayMapping>>,
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
            mappings: RwLock::new(BTreeMap::new()),
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
            mappings: RwLock::new(BTreeMap::new()),
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

    fn page_aligned_size(size: usize) -> usize {
        (size + crate::environment::PAGE_SIZE - 1) & !(crate::environment::PAGE_SIZE - 1)
    }

    fn current_backing_info(&self) -> Result<DisplayBackingInfo, &'static str> {
        if let Some(device) = self
            .device_manager()
            .get_device(self.fb_resource.source_device_id)
        {
            if let Some(graphics_device) = device.as_graphics_device() {
                let (config, physical_addr) = graphics_device.get_framebuffer_info()?;
                let size = Self::page_aligned_size(config.size());
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
            _ => Err("Unsupported display control command"),
        }
    }

    fn supported_control_commands(&self) -> Vec<(u32, &'static str)> {
        use display_commands::*;
        vec![
            (DISPLAY_GET_INFO, "Get display surface information"),
            (DISPLAY_PRESENT, "Present whole display surface"),
            (DISPLAY_PRESENT_REGION, "Present display surface region"),
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
            return Err("Offset exceeds display backing size");
        }

        let available_size = info.size - offset;
        if length > available_size {
            return Err("Requested length exceeds available display backing size");
        }

        Ok(crate::object::capability::MemoryMappingInfo::new(
            info.physical_addr + offset,
            0x3,
            true,
        )
        .with_memory_attribute(crate::vm::vmem::MemoryAttribute::NonCacheable))
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
