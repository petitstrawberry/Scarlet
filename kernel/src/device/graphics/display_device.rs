//! Modern display character device.
//!
//! This endpoint represents a scanout/display surface for display-system
//! clients that explicitly present damage regions. Legacy framebuffer
//! compatibility remains available through `/dev/fbX`.

extern crate alloc;

use crate::sync::IrqGuard;
use alloc::{sync::Arc, vec, vec::Vec};
use core::any::Any;

use super::{
    FramebufferConfig, GpuPresentOptions, PixelFormat, PresentationOwners,
    manager::FramebufferResource,
    output::{ConnectorKind, DisplayRegion},
};
use crate::device::{Device, DeviceType, char::CharDevice, manager::DeviceManager};
use crate::library::std::usercopy::{copy_from_user, copy_to_user};
use crate::object::capability::memory_mapping::{
    MappingError, MappingRequest, MemoryMappingInfo, PreparedMapping,
};
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
    /// Return the display backlight level as an integer percentage in `0..=100`.
    ///
    /// This command ignores its argument and returns the percentage directly
    /// as the successful `control` return value.
    pub const DISPLAY_GET_BRIGHTNESS: u32 = 0x5006;
    /// Set the display backlight level from an integer percentage in `0..=100`.
    ///
    /// The scalar `control` argument is the requested percentage. Values
    /// outside the inclusive `0..=100` range are rejected.
    pub const DISPLAY_SET_BRIGHTNESS: u32 = 0x5007;
    /// Describe the output this endpoint presents and its connector.
    ///
    /// The argument points to a [`super::DisplayOutputInfo`] whose `size` the
    /// caller sets to the bytes it provides. See that type for the contract.
    pub const DISPLAY_GET_OUTPUT_INFO: u32 = 0x5008;
    pub const DISPLAY_GET_MODE: u32 = 0x5009;
    pub const DISPLAY_REQUEST_MODE: u32 = 0x500a;
}

/// Indexed mode query. The caller sets `index`; the kernel fills `count` and
/// the mode (zero when index is past the last entry).
#[repr(C)]
#[derive(Default, Clone, Copy)]
struct DisplayModeQuery {
    index: u32,
    count: u32,
    mode: super::output::DisplayMode,
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
    // Explicitly initialize the existing ABI padding before copying to userspace.
    reserved: u32,
    /// Opaque identifier for the current mappable backing store.
    ///
    /// This value changes when the display surface's mapped backing changes,
    /// even if `buffer_len` remains the same.
    pub backing_id: u64,
}

const _: () = {
    assert!(core::mem::size_of::<DisplayInfo>() == 32);
    assert!(core::mem::offset_of!(DisplayInfo, backing_id) == 24);
};

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

/// Layout version of [`DisplayOutputInfo`] written by this kernel.
pub const DISPLAY_OUTPUT_INFO_VERSION: u32 = 3;
/// A monitor is attached to the connector and enabled.
pub const DISPLAY_OUTPUT_FLAG_CONNECTED: u32 = 1 << 0;
/// This endpoint is `/dev/display0`, the primary output.
pub const DISPLAY_OUTPUT_FLAG_PRIMARY: u32 = 1 << 1;
/// Connector kind is unknown.
pub const DISPLAY_CONNECTOR_KIND_UNKNOWN: u32 = 0;
/// Built-in panel.
pub const DISPLAY_CONNECTOR_KIND_INTERNAL: u32 = 1;
/// External monitor port.
pub const DISPLAY_CONNECTOR_KIND_EXTERNAL: u32 = 2;
/// Output of a virtual display device.
pub const DISPLAY_CONNECTOR_KIND_VIRTUAL: u32 = 3;
/// Bytes reserved for the NUL-padded connector name.
pub const DISPLAY_CONNECTOR_NAME_LEN: usize = 32;
/// The original record prefix remains accepted by newer kernels.
pub const DISPLAY_OUTPUT_INFO_V1_SIZE: usize = 80;
/// Stable firmware/bus identity; an empty key means persistence is unavailable.
pub const DISPLAY_DEVICE_KEY_LEN: usize = 96;
pub const DISPLAY_OUTPUT_INFO_V2_SIZE: usize = 176;
pub const DISPLAY_MONITOR_KEY_LEN: usize = 96;
pub const DISPLAY_MONITOR_NAME_LEN: usize = 64;
pub const DISPLAY_OUTPUT_FLAG_MONITOR_SERIAL: u32 = 1 << 2;

/// Argument for `DISPLAY_GET_OUTPUT_INFO`.
///
/// Contract:
/// * The caller sets `size` to the number of bytes it provides. Requests
///   smaller than the version 1 record are rejected.
/// * The kernel writes `min(size, size_of::<DisplayOutputInfo>())` bytes and
///   stores that count in `size`, and the layout version in `version`. Later
///   versions only append fields.
/// * All fields are fixed width so the record is identical on 32- and 64-bit
///   targets. Unknown values are 0.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct DisplayOutputInfo {
    /// In: bytes provided by the caller. Out: bytes written.
    pub size: u32,
    /// Out: [`DISPLAY_OUTPUT_INFO_VERSION`].
    pub version: u32,
    /// `DISPLAY_OUTPUT_FLAG_*` values.
    pub flags: u32,
    /// Connector this endpoint presents, within its graphics device.
    pub connector_index: u32,
    /// Connectors the graphics device exposes, whether connected or not.
    pub connector_count: u32,
    /// `DISPLAY_CONNECTOR_KIND_*` value.
    pub kind: u32,
    /// Current mode width in pixels.
    pub width: u32,
    /// Current mode height in pixels.
    pub height: u32,
    /// Refresh rate in millihertz.
    pub refresh_mhz: u32,
    /// Physical width in millimetres.
    pub physical_width_mm: u32,
    /// Physical height in millimetres.
    pub physical_height_mm: u32,
    /// Reserved; written as 0.
    pub reserved: u32,
    /// NUL-padded connector name.
    pub name: [u8; DISPLAY_CONNECTOR_NAME_LEN],
    /// Version 2: NUL-padded stable device identity, independent of enumeration.
    pub device_key: [u8; DISPLAY_DEVICE_KEY_LEN],
    /// Version 3: EDID display identity, independent of its connector.
    pub monitor_key: [u8; DISPLAY_MONITOR_KEY_LEN],
    pub monitor_name: [u8; DISPLAY_MONITOR_NAME_LEN],
}

/// Present the full GPU image and require zero region fields.
pub const DISPLAY_PRESENT_IMAGE_FLAG_FULL_FRAME: u32 = 1 << 0;
/// The producer will not rewrite this image until another image is presented.
pub const DISPLAY_PRESENT_IMAGE_FLAG_SWAPCHAIN_BUFFER: u32 = 1 << 1;
/// All writes to this image have completed successfully before this request.
pub const DISPLAY_PRESENT_IMAGE_FLAG_PRODUCER_COMPLETE: u32 = 1 << 2;
/// All currently defined GPU image presentation flags.
pub const DISPLAY_PRESENT_IMAGE_FLAGS_VALID: u32 = DISPLAY_PRESENT_IMAGE_FLAG_FULL_FRAME
    | DISPLAY_PRESENT_IMAGE_FLAG_SWAPCHAIN_BUFFER
    | DISPLAY_PRESENT_IMAGE_FLAG_PRODUCER_COMPLETE;

#[derive(Debug, Clone)]
struct DisplayBackingInfo {
    config: FramebufferConfig,
    physical_addr: u64,
    size: usize,
}

/// Retains the exact scanout allocation and byte range admitted for one VMA.
struct DisplayMapping {
    _device: Arc<dyn Device>,
    info: MemoryMappingInfo,
    length: usize,
}

impl MemoryMappingOps for DisplayMapping {
    fn stable_physical_extent(&self, page: usize, count: usize) -> Option<u64> {
        use crate::environment::PAGE_SIZE;
        let offset = page.checked_mul(PAGE_SIZE)?;
        let length = count.checked_mul(PAGE_SIZE)?;
        self.get_mapping_info(offset, length)
            .ok()
            .map(|info| info.paddr)
    }

    fn get_mapping_info(
        &self,
        offset: usize,
        length: usize,
    ) -> Result<MemoryMappingInfo, &'static str> {
        if offset
            .checked_add(length)
            .is_none_or(|end| end > self.length)
        {
            return Err("Display mapping range exceeded");
        }
        Ok(MemoryMappingInfo {
            paddr: self
                .info
                .paddr
                .checked_add(offset as u64)
                .ok_or("Display backing physical address overflow")?,
            ..self.info
        })
    }

    fn mmap_owner_name(&self) -> alloc::string::String {
        alloc::string::String::from("display")
    }
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

    fn snapshot_backing(&self) -> Result<(DisplayBackingInfo, Arc<dyn Device>), &'static str> {
        let connector = self.fb_resource.connector_index;
        let source = self.source_graphics_device()?;
        let source_graphics = source
            .as_graphics_device()
            .ok_or("Invalid graphics device")?;
        let exact_backing = source_graphics.connector_mapping_device(connector)?;
        let (config, physical_addr, size) = if let Some(backing) = exact_backing.as_ref() {
            let graphics = backing
                .as_graphics_device()
                .ok_or("Invalid display backing device")?;
            let (config, address) = graphics.get_framebuffer_info()?;
            let size = graphics.framebuffer_mapping_size(&config)?;
            (config, address, size)
        } else {
            let (config, address) = source_graphics.connector_framebuffer_info(connector)?;
            let size = source_graphics.framebuffer_mapping_size(&config)?;
            (config, address, size)
        };
        Ok((
            DisplayBackingInfo {
                config,
                physical_addr,
                size,
            },
            exact_backing.unwrap_or(source),
        ))
    }

    fn snapshot_mapping(
        &self,
        offset: usize,
        length: usize,
        allow_shorter: bool,
    ) -> Result<DisplayMapping, &'static str> {
        use crate::environment::PAGE_SIZE;

        if !offset.is_multiple_of(PAGE_SIZE) || !length.is_multiple_of(PAGE_SIZE) {
            return Err("Display mmap range must be page-aligned");
        }
        let (
            DisplayBackingInfo {
                physical_addr,
                size,
                ..
            },
            device,
        ) = self.snapshot_backing()?;
        let graphics = device
            .as_graphics_device()
            .ok_or("Invalid display backing device")?;
        let connector = self.fb_resource.connector_index;
        if physical_addr == 0 || size == 0 {
            return Err("Invalid display backing configuration");
        }
        if !physical_addr.is_multiple_of(PAGE_SIZE as u64) {
            return Err("Display backing physical address must be page-aligned");
        }

        let (base, buffer_offset) = if offset < size {
            (physical_addr, offset)
        } else if connector == 0 {
            let relative = offset - size;
            let (_, address) = graphics.get_scanout_buffer_info(relative / size)?;
            (address, relative % size)
        } else {
            return Err("Secondary display mmap exceeds framebuffer");
        };
        let available = size - buffer_offset;
        let length = if allow_shorter {
            length.min(available)
        } else {
            length
        };
        if length > available {
            return Err("Requested length exceeds available display backing size");
        }
        let paddr = base
            .checked_add(buffer_offset as u64)
            .ok_or("Display backing physical address overflow")?;
        let info = MemoryMappingInfo::new(paddr, 0x3, true)
            .with_memory_attribute(graphics.framebuffer_memory_attribute());
        Ok(DisplayMapping {
            _device: device,
            info,
            length,
        })
    }

    fn current_backing_info(&self) -> Result<DisplayBackingInfo, &'static str> {
        if let Some(device) = self
            .device_manager()
            .get_device(self.fb_resource.source_device_id)
        {
            if let Some(graphics_device) = device.as_graphics_device() {
                let (config, physical_addr) =
                    graphics_device.connector_framebuffer_info(self.fb_resource.connector_index)?;
                let size = graphics_device.framebuffer_mapping_size(&config)?;
                return Ok(DisplayBackingInfo {
                    config,
                    physical_addr,
                    size,
                });
            }
        }

        if self.fb_resource.connector_index != 0 {
            // Secondary connectors have no static backing to fall back to.
            return Err("Display source device not found");
        }
        Ok(DisplayBackingInfo {
            config: self.fb_resource.config.clone(),
            physical_addr: self.fb_resource.physical_addr,
            size: self.fb_resource.size,
        })
    }

    /// Resolve the graphics device that owns this display surface.
    fn source_graphics_device(&self) -> Result<Arc<dyn Device>, &'static str> {
        let device = self
            .device_manager()
            .get_device(self.fb_resource.source_device_id)
            .ok_or("Display source device not found")?;
        if device.as_graphics_device().is_none() {
            return Err("Display source device is not graphics-capable");
        }
        Ok(device)
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
            reserved: 0,
            backing_id: backing.physical_addr,
        })
    }

    fn handle_get_info(&self, arg: usize) -> Result<i32, &'static str> {
        let info = self.current_display_info()?;
        // SAFETY: DisplayInfo contains only initialized integer fields and no padding.
        Self::write_argument(arg, unsafe {
            core::slice::from_raw_parts(
                (&info as *const DisplayInfo).cast::<u8>(),
                core::mem::size_of::<DisplayInfo>(),
            )
        })?;
        Ok(0)
    }

    fn handle_get_brightness(&self) -> Result<i32, &'static str> {
        let _transaction = crate::task::device_transaction()?;
        let device = self.source_graphics_device()?;
        let brightness = device
            .as_graphics_device()
            .ok_or("Display source device is not graphics-capable")?
            .connector_brightness_percent(self.fb_resource.connector_index)?;
        if brightness > 100 {
            return Err("Display driver returned an invalid brightness percentage");
        }
        Ok(i32::from(brightness))
    }

    fn handle_set_brightness(&self, arg: usize) -> Result<i32, &'static str> {
        let _transaction = crate::task::device_transaction()?;
        if arg > 100 {
            return Err("Display brightness must be in the range 0..=100");
        }

        let device = self.source_graphics_device()?;
        device
            .as_graphics_device()
            .ok_or("Display source device is not graphics-capable")?
            .set_connector_brightness_percent(self.fb_resource.connector_index, arg as u8)?;
        Ok(0)
    }

    fn connector_kind_code(kind: ConnectorKind) -> u32 {
        match kind {
            ConnectorKind::Unknown => DISPLAY_CONNECTOR_KIND_UNKNOWN,
            ConnectorKind::Internal => DISPLAY_CONNECTOR_KIND_INTERNAL,
            ConnectorKind::External => DISPLAY_CONNECTOR_KIND_EXTERNAL,
            ConnectorKind::Virtual => DISPLAY_CONNECTOR_KIND_VIRTUAL,
        }
    }

    /// Build the output description for this endpoint.
    fn current_output_info(&self) -> Result<DisplayOutputInfo, &'static str> {
        let device = self.source_graphics_device()?;
        let connectors = device
            .as_graphics_device()
            .ok_or("Display source device is not graphics-capable")?
            .connectors();
        let connector_index = self.fb_resource.connector_index;
        let mut info = DisplayOutputInfo {
            size: core::mem::size_of::<DisplayOutputInfo>() as u32,
            version: DISPLAY_OUTPUT_INFO_VERSION,
            flags: 0,
            connector_index,
            connector_count: connectors.len() as u32,
            kind: DISPLAY_CONNECTOR_KIND_UNKNOWN,
            width: 0,
            height: 0,
            refresh_mhz: 0,
            physical_width_mm: 0,
            physical_height_mm: 0,
            reserved: 0,
            name: [0; DISPLAY_CONNECTOR_NAME_LEN],
            device_key: [0; DISPLAY_DEVICE_KEY_LEN],
            monitor_key: [0; DISPLAY_MONITOR_KEY_LEN],
            monitor_name: [0; DISPLAY_MONITOR_NAME_LEN],
        };
        if let Some(key) = device
            .as_graphics_device()
            .and_then(|graphics| graphics.display_device_key())
        {
            // Never truncate an identity: different long keys must not alias.
            if !key.is_empty()
                && key.len() < DISPLAY_DEVICE_KEY_LEN
                && !key.chars().any(char::is_control)
            {
                info.device_key[..key.len()].copy_from_slice(key.as_bytes());
            }
        }
        if let Some(identity) = device
            .as_graphics_device()
            .and_then(|graphics| graphics.connector_monitor_identity(connector_index))
        {
            if !identity.key.is_empty() && identity.key.len() < DISPLAY_MONITOR_KEY_LEN {
                info.monitor_key[..identity.key.len()].copy_from_slice(identity.key.as_bytes());
                if identity.has_serial {
                    info.flags |= DISPLAY_OUTPUT_FLAG_MONITOR_SERIAL;
                }
            }
            let mut len = identity.name.len().min(DISPLAY_MONITOR_NAME_LEN - 1);
            while !identity.name.is_char_boundary(len) {
                len -= 1;
            }
            info.monitor_name[..len].copy_from_slice(&identity.name.as_bytes()[..len]);
        }
        if self.fb_resource.logical_name == "fb0" {
            info.flags |= DISPLAY_OUTPUT_FLAG_PRIMARY;
        }
        if let Some(connector) = connectors.get(connector_index as usize) {
            if connector.connected {
                info.flags |= DISPLAY_OUTPUT_FLAG_CONNECTED;
            }
            info.kind = Self::connector_kind_code(connector.kind);
            info.width = connector.width;
            info.height = connector.height;
            info.refresh_mhz = connector.refresh_mhz;
            if let Some((width_mm, height_mm)) = connector.physical_size_mm {
                info.physical_width_mm = width_mm;
                info.physical_height_mm = height_mm;
            }
            // Keep a terminating NUL so C callers can read it as a string,
            // and cut on a character boundary so the prefix stays UTF-8.
            let mut len = connector.name.len().min(DISPLAY_CONNECTOR_NAME_LEN - 1);
            while !connector.name.is_char_boundary(len) {
                len -= 1;
            }
            info.name[..len].copy_from_slice(&connector.name.as_bytes()[..len]);
        }
        Ok(info)
    }

    /// Copy bytes from the caller, or from a kernel pointer in kernel tests.
    fn read_argument(arg: usize, dst: &mut [u8]) -> Result<(), &'static str> {
        if arg == 0 {
            return Err("Invalid argument pointer");
        }
        match crate::task::mytask() {
            Some(task) => copy_from_user(&task, arg, dst).map_err(|_| "Invalid user pointer"),
            None => {
                // SAFETY: without a current task, in-kernel callers pass a
                // kernel pointer to at least `dst.len()` readable bytes.
                unsafe {
                    core::ptr::copy_nonoverlapping(arg as *const u8, dst.as_mut_ptr(), dst.len())
                };
                Ok(())
            }
        }
    }

    /// Copy bytes to the caller, or to a kernel pointer in kernel tests.
    fn write_argument(arg: usize, src: &[u8]) -> Result<(), &'static str> {
        if arg == 0 {
            return Err("Invalid argument pointer");
        }
        match crate::task::mytask() {
            Some(task) => copy_to_user(&task, arg, src).map_err(|_| "Invalid user pointer"),
            None => {
                // SAFETY: without a current task, in-kernel callers pass a
                // kernel pointer to at least `src.len()` writable bytes.
                unsafe { core::ptr::copy_nonoverlapping(src.as_ptr(), arg as *mut u8, src.len()) };
                Ok(())
            }
        }
    }

    fn handle_get_output_info(&self, arg: usize) -> Result<i32, &'static str> {
        let mut size_bytes = [0u8; 4];
        Self::read_argument(arg, &mut size_bytes)?;
        let requested = u32::from_ne_bytes(size_bytes) as usize;
        let record_len = core::mem::size_of::<DisplayOutputInfo>();
        if requested < DISPLAY_OUTPUT_INFO_V1_SIZE {
            return Err("Display output info buffer is too small");
        }

        let mut info = self.current_output_info()?;
        let written = requested.min(record_len);
        info.size = written as u32;
        // SAFETY: DisplayOutputInfo is repr(C) with only integer fields and no
        // padding, so viewing it as bytes is sound.
        let bytes = unsafe {
            core::slice::from_raw_parts(
                (&info as *const DisplayOutputInfo).cast::<u8>(),
                record_len,
            )
        };
        Self::write_argument(arg, &bytes[..written])?;
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
        let mut region = DisplayPresentRegion::default();
        // SAFETY: DisplayPresentRegion has no padding and every integer bit
        // pattern in the supplied bytes is valid.
        Self::read_argument(arg, unsafe {
            core::slice::from_raw_parts_mut(
                (&mut region as *mut DisplayPresentRegion).cast::<u8>(),
                core::mem::size_of::<DisplayPresentRegion>(),
            )
        })?;

        self.present_region(DisplayRegion::new(
            region.x,
            region.y,
            region.width,
            region.height,
        ))?;
        Ok(0)
    }

    fn handle_get_swapchain(&self, arg: usize) -> Result<i32, &'static str> {
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
        let buffer_len = graphics.framebuffer_mapping_size(&config)?;
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
        // SAFETY: Four u32 fields followed by a usize have no padding on
        // supported targets, and each field has been initialized.
        Self::write_argument(arg, unsafe {
            core::slice::from_raw_parts(
                (&info as *const DisplaySwapchainInfo).cast::<u8>(),
                core::mem::size_of::<DisplaySwapchainInfo>(),
            )
        })?;
        Ok(0)
    }

    fn handle_present_buffer(&self, arg: usize) -> Result<i32, &'static str> {
        let _transaction = crate::task::device_transaction()?;
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
        let (completion, legacy) = {
            // No scheduling boundary may split transfer of these owning refs.
            let _handoff = IrqGuard::new();
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
            // The backend operation retains the image until hardware returns it.
            let options = if request.flags & DISPLAY_PRESENT_IMAGE_FLAG_SWAPCHAIN_BUFFER != 0 {
                GpuPresentOptions::swapchain_buffer()
            } else {
                GpuPresentOptions::default()
            };
            let options = if request.flags & DISPLAY_PRESENT_IMAGE_FLAG_PRODUCER_COMPLETE != 0 {
                options.with_producer_complete()
            } else {
                options
            };
            let completion = graphics.begin_connector_gpu_resource_present(
                self.fb_resource.connector_index,
                resource.clone(),
                region,
                options,
                PresentationOwners::new((image_owner.clone(), Arc::clone(&device))),
            )?;
            if let Some(completion) = completion {
                // The core operation now owns the image and device. The ticket
                // retains only a weak backend identity and cannot pin their pages.
                drop((image_owner, device, resource));
                (Some(completion), None)
            } else {
                (None, Some((image_owner, device, resource, region, options)))
            }
        };
        if let Some(completion) = completion {
            completion.wait()?;
        } else {
            let _transaction = crate::task::device_transaction()?;
            let (_image_owner, device, resource, region, options) =
                legacy.expect("legacy presentation");
            device
                .as_graphics_device()
                .ok_or("Display source device is not graphics-capable")?
                .present_connector_gpu_resource_region(
                    self.fb_resource.connector_index,
                    resource,
                    region,
                    options,
                )?;
        }
        Ok(0)
    }

    fn present_region(&self, region: DisplayRegion) -> Result<(), &'static str> {
        let (completion, legacy, region) = {
            // Ownership acquisition and transfer form one nonpreemptible step.
            // The existing framebuffer snapshot is synchronous; the prepared
            // presentation wait happens only after this guard is released.
            let _handoff = IrqGuard::new();
            let device = self
                .device_manager()
                .get_device(self.fb_resource.source_device_id)
                .ok_or("Display source device not found")?;
            let graphics_device = device
                .as_graphics_device()
                .ok_or("Display source device is not graphics-capable")?;
            let connector = self.fb_resource.connector_index;
            let (config, physical_addr) = graphics_device.connector_framebuffer_info(connector)?;
            if physical_addr == 0 {
                return Err("Invalid display backing address");
            }

            let region = DisplayRegion::new(
                region.x.min(config.width),
                region.y.min(config.height),
                region.width.min(config.width.saturating_sub(region.x)),
                region.height.min(config.height.saturating_sub(region.y)),
            );
            let completion = graphics_device.begin_connector_framebuffer_present(
                connector,
                config,
                physical_addr,
                region,
                PresentationOwners::new(Arc::clone(&device)),
            )?;
            if let Some(completion) = completion {
                drop(device);
                (Some(completion), None, region)
            } else {
                (None, Some(device), region)
            }
        };
        if let Some(completion) = completion {
            completion.wait()
        } else {
            let _transaction = crate::task::device_transaction()?;
            legacy
                .expect("legacy framebuffer presentation")
                .as_graphics_device()
                .ok_or("Display source device is not graphics-capable")?
                .present_connector_framebuffer_region(self.fb_resource.connector_index, region)
        }
    }
}

impl Device for DisplayCharDevice {
    fn capabilities(&self) -> &'static [crate::device::DeviceCapability] {
        &[crate::device::DeviceCapability::PhysicalSeat]
    }

    fn open(self: Arc<Self>) -> Result<Arc<dyn Device>, &'static str> {
        if self.fb_resource.connector_index == 0 {
            return Ok(self);
        }
        #[cfg(not(test))]
        let opened = Self::new(self.fb_resource.clone());
        #[cfg(test)]
        let opened = Self::new_with_device_manager(self.fb_resource.clone(), self.device_manager());
        Ok(Arc::new(opened))
    }

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
        let (info, _backing) = self.snapshot_backing()?;
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
        let (info, _backing) = self.snapshot_backing()?;
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

        if self.fb_resource.connector_index != 0
            && matches!(command, DISPLAY_GET_SWAPCHAIN | DISPLAY_PRESENT_BUFFER)
        {
            // Direct-scanout swapchains are a driver feature of the primary
            // connector only. Other connectors present through the CPU
            // framebuffer or GPU images, like the primary does without one.
            return Err("Direct scanout buffers are not supported on this connector");
        }
        match command {
            DISPLAY_GET_MODE | DISPLAY_REQUEST_MODE => {
                let device = self
                    .device_manager()
                    .get_device(self.fb_resource.source_device_id)
                    .ok_or("Display source device not found")?;
                let graphics = device.as_graphics_device().ok_or("Not a graphics device")?;
                if command == DISPLAY_GET_MODE {
                    let mut query = DisplayModeQuery::default();
                    // SAFETY: repr(C) structure contains only initialized u32 fields.
                    Self::read_argument(arg, unsafe {
                        core::slice::from_raw_parts_mut(
                            &mut query as *mut _ as *mut u8,
                            core::mem::size_of::<DisplayModeQuery>(),
                        )
                    })?;
                    let modes = graphics.connector_modes(self.fb_resource.connector_index);
                    query.count = modes.len() as u32;
                    query.mode = modes.get(query.index as usize).copied().unwrap_or_default();
                    // SAFETY: all fields have been initialized; this type has no padding.
                    Self::write_argument(arg, unsafe {
                        core::slice::from_raw_parts(
                            &query as *const _ as *const u8,
                            core::mem::size_of::<DisplayModeQuery>(),
                        )
                    })?;
                    Ok(0)
                } else {
                    let mut mode = super::output::DisplayMode::default();
                    // SAFETY: mode is a repr(C) triple of initialized u32 fields.
                    Self::read_argument(arg, unsafe {
                        core::slice::from_raw_parts_mut(
                            &mut mode as *mut _ as *mut u8,
                            core::mem::size_of::<super::output::DisplayMode>(),
                        )
                    })?;
                    graphics.request_connector_mode(self.fb_resource.connector_index, mode)?;
                    Ok(0)
                }
            }
            DISPLAY_GET_INFO => self.handle_get_info(arg),
            DISPLAY_GET_BRIGHTNESS => self.handle_get_brightness(),
            DISPLAY_SET_BRIGHTNESS => self.handle_set_brightness(arg),
            DISPLAY_GET_OUTPUT_INFO => self.handle_get_output_info(arg),
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
            (DISPLAY_GET_BRIGHTNESS, "Get display brightness percentage"),
            (DISPLAY_SET_BRIGHTNESS, "Set display brightness percentage"),
            (
                DISPLAY_GET_OUTPUT_INFO,
                "Get output and connector information",
            ),
            (DISPLAY_GET_MODE, "Query display mode by index"),
            (DISPLAY_REQUEST_MODE, "Request display mode"),
            (DISPLAY_PRESENT, "Present whole display surface"),
            (DISPLAY_PRESENT_REGION, "Present display surface region"),
            (DISPLAY_GET_SWAPCHAIN, "Get direct scanout swapchain"),
            (DISPLAY_PRESENT_BUFFER, "Present direct scanout buffer"),
            (DISPLAY_PRESENT_IMAGE, "Present a GPU image capability"),
        ]
    }
}

impl MemoryMappingOps for DisplayCharDevice {
    fn prepare_mapping(&self, request: MappingRequest) -> Result<PreparedMapping, MappingError> {
        let maximum_permissions = self.new_mapping_permissions(request.is_shared);
        if request.permissions & !maximum_permissions != 0 {
            return Err(MappingError::PermissionDenied);
        }
        if request.private_demand {
            if !self.supports_mmap() {
                return Err(MappingError::Unsupported);
            }
            return Ok(PreparedMapping {
                length: request.length,
                info: None,
                owner: None,
                notification: None,
                maximum_permissions,
            });
        }
        let mapping = self
            .snapshot_mapping(request.offset, request.length, request.allow_shorter)
            .map_err(MappingError::InvalidRange)?;
        Ok(PreparedMapping {
            length: mapping.length,
            info: Some(mapping.info),
            owner: Some(Arc::new(mapping)),
            notification: None,
            maximum_permissions,
        })
    }

    fn get_mapping_info(
        &self,
        offset: usize,
        length: usize,
    ) -> Result<MemoryMappingInfo, &'static str> {
        self.snapshot_mapping(offset, length, false)
            .map(|mapping| mapping.info)
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

#[cfg(test)]
mod tests {
    use super::{
        DISPLAY_CONNECTOR_KIND_INTERNAL, DISPLAY_CONNECTOR_KIND_UNKNOWN,
        DISPLAY_CONNECTOR_NAME_LEN, DISPLAY_OUTPUT_FLAG_CONNECTED, DISPLAY_OUTPUT_INFO_VERSION,
        DisplayCharDevice, DisplayOutputInfo, MappingRequest, display_commands,
    };
    use crate::device::{
        Device, DeviceType,
        graphics::{
            FramebufferConfig, GenericGraphicsDevice, GraphicsDevice, PixelFormat,
            manager::FramebufferResource,
            output::{ConnectorInfo, ConnectorKind},
        },
        manager::DeviceManager,
    };
    use crate::object::capability::selectable::{SelectWaitOutcome, Selectable};
    use crate::object::capability::{ControlOps, MemoryMappingOps};
    use crate::sync::IrqRwSpinLock;
    use alloc::{string::ToString, sync::Arc};
    use core::{
        any::Any,
        sync::atomic::{AtomicU8, Ordering},
    };

    struct TestBrightnessGraphicsDevice {
        brightness: AtomicU8,
        backing: IrqRwSpinLock<Option<Arc<dyn Device>>>,
    }

    impl TestBrightnessGraphicsDevice {
        fn new(brightness: u8) -> Self {
            Self {
                brightness: AtomicU8::new(brightness),
                backing: IrqRwSpinLock::new(None),
            }
        }
    }

    impl Device for TestBrightnessGraphicsDevice {
        fn device_type(&self) -> DeviceType {
            DeviceType::Graphics
        }

        fn name(&self) -> &'static str {
            "test-brightness-graphics"
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

    impl ControlOps for TestBrightnessGraphicsDevice {}

    impl MemoryMappingOps for TestBrightnessGraphicsDevice {
        fn get_mapping_info(
            &self,
            _offset: usize,
            _length: usize,
        ) -> Result<crate::object::capability::MemoryMappingInfo, &'static str> {
            Err("Test graphics device has no mappings")
        }
    }

    impl Selectable for TestBrightnessGraphicsDevice {
        fn wait_until_ready(
            &self,
            _interest: crate::object::capability::selectable::ReadyInterest,
            _trapframe: &mut crate::arch::Trapframe,
            _timeout_ticks: Option<u64>,
            _min_wait_ticks: u64,
        ) -> SelectWaitOutcome {
            SelectWaitOutcome::Ready
        }
    }

    impl GraphicsDevice for TestBrightnessGraphicsDevice {
        fn connector_mapping_device(
            &self,
            _connector: u32,
        ) -> Result<Option<Arc<dyn Device>>, &'static str> {
            Ok(self.backing.read().clone())
        }

        fn get_display_name(&self) -> &'static str {
            "test-brightness-graphics"
        }

        fn get_brightness_percent(&self) -> Result<u8, &'static str> {
            Ok(self.brightness.load(Ordering::Relaxed))
        }

        fn set_brightness_percent(&self, percent: u8) -> Result<(), &'static str> {
            if percent > 100 {
                return Err("Display brightness must be in the range 0..=100");
            }
            self.brightness.store(percent, Ordering::Relaxed);
            Ok(())
        }

        fn get_framebuffer_config(&self) -> Result<FramebufferConfig, &'static str> {
            Ok(FramebufferConfig::new(1, 1, PixelFormat::XRGB8888))
        }

        fn get_framebuffer_address(&self) -> Result<u64, &'static str> {
            Ok(0x1000)
        }

        fn present_framebuffer_region(
            &self,
            _config: &FramebufferConfig,
            _physical_addr: u64,
            _region: crate::device::graphics::output::DisplayRegion,
        ) -> Result<(), &'static str> {
            Ok(())
        }

        fn init_graphics(&self) -> Result<(), &'static str> {
            Ok(())
        }
    }

    fn display_for_test(manager: &DeviceManager, device: Arc<dyn Device>) -> DisplayCharDevice {
        let source_device_id =
            manager.register_device_with_name("test-brightness-graphics".to_string(), device);
        let config = FramebufferConfig::new(1, 1, PixelFormat::XRGB8888);
        let resource = Arc::new(FramebufferResource::new(
            source_device_id,
            "fb-test".to_string(),
            config.clone(),
            0x1000,
            config.size(),
        ));
        DisplayCharDevice::new_with_device_manager(resource, manager)
    }

    #[test_case]
    fn display_remap_on_the_same_open_captures_the_new_backing() {
        use crate::environment::PAGE_SIZE;

        let manager = DeviceManager::new_for_test();
        let graphics = Arc::new(TestBrightnessGraphicsDevice::new(42));
        let display = Arc::new(display_for_test(&manager, graphics.clone()))
            .open()
            .unwrap();
        let make_backing = |paddr| {
            let mut backing = GenericGraphicsDevice::new("test-scanout-allocation");
            backing.set_framebuffer_config(FramebufferConfig::new(1, 1, PixelFormat::XRGB8888));
            backing.set_framebuffer_address(paddr);
            Arc::new(backing)
        };
        let first = make_backing(PAGE_SIZE as u64);
        let first_weak = Arc::downgrade(&first);
        *graphics.backing.write() = Some(first);
        let request = MappingRequest {
            offset: 0,
            length: PAGE_SIZE,
            is_shared: true,
            permissions: 3,
            private_demand: false,
            allow_shorter: false,
        };
        let old = display.prepare_mapping(request).unwrap();
        *graphics.backing.write() = Some(make_backing(2 * PAGE_SIZE as u64));
        let new = display.prepare_mapping(request).unwrap();

        assert_eq!(old.info.unwrap().paddr, PAGE_SIZE as u64);
        assert_eq!(new.info.unwrap().paddr, 2 * PAGE_SIZE as u64);
        let owner = old.owner.unwrap();
        assert_eq!(
            owner.get_mapping_info(0, PAGE_SIZE).unwrap(),
            old.info.unwrap()
        );
        assert_eq!(owner.stable_physical_extent(0, 1), Some(PAGE_SIZE as u64));
        assert_eq!(owner.stable_physical_extent(1, 1), None);
        assert!(first_weak.upgrade().is_some());
        drop(owner);
        assert!(first_weak.upgrade().is_none());
    }

    #[test_case]
    fn display_brightness_control_round_trips_valid_percentages() {
        let manager = DeviceManager::new_for_test();
        let graphics = Arc::new(TestBrightnessGraphicsDevice::new(42));
        let display = display_for_test(&manager, graphics.clone());

        assert_eq!(
            display
                .control(display_commands::DISPLAY_GET_BRIGHTNESS, 0)
                .expect("get brightness should succeed"),
            42
        );
        assert_eq!(
            display
                .control(display_commands::DISPLAY_SET_BRIGHTNESS, 100)
                .expect("set brightness should succeed"),
            0
        );
        assert_eq!(graphics.brightness.load(Ordering::Relaxed), 100);
        assert_eq!(
            display
                .control(display_commands::DISPLAY_GET_BRIGHTNESS, 0)
                .expect("get brightness after set should succeed"),
            100
        );
        assert!(
            display
                .supported_control_commands()
                .iter()
                .any(|(command, _)| *command == display_commands::DISPLAY_GET_BRIGHTNESS)
        );
        assert!(
            display
                .supported_control_commands()
                .iter()
                .any(|(command, _)| *command == display_commands::DISPLAY_SET_BRIGHTNESS)
        );
    }

    fn output_info_request(size: u32) -> DisplayOutputInfo {
        DisplayOutputInfo {
            size,
            version: 0,
            flags: 0,
            connector_index: u32::MAX,
            connector_count: 0,
            kind: u32::MAX,
            width: 0,
            height: 0,
            refresh_mhz: u32::MAX,
            physical_width_mm: u32::MAX,
            physical_height_mm: u32::MAX,
            reserved: u32::MAX,
            name: [0xff; DISPLAY_CONNECTOR_NAME_LEN],
            device_key: [0xff; super::DISPLAY_DEVICE_KEY_LEN],
            monitor_key: [0xff; super::DISPLAY_MONITOR_KEY_LEN],
            monitor_name: [0xff; super::DISPLAY_MONITOR_NAME_LEN],
        }
    }

    #[test_case]
    fn display_output_info_reports_default_single_connector() {
        let manager = DeviceManager::new_for_test();
        let graphics = Arc::new(TestBrightnessGraphicsDevice::new(42));
        let display = display_for_test(&manager, graphics);
        let mut info = output_info_request(core::mem::size_of::<DisplayOutputInfo>() as u32);

        assert_eq!(
            display
                .control(
                    display_commands::DISPLAY_GET_OUTPUT_INFO,
                    &mut info as *mut DisplayOutputInfo as usize,
                )
                .expect("output info should succeed"),
            0
        );
        assert_eq!(
            info.size as usize,
            core::mem::size_of::<DisplayOutputInfo>()
        );
        assert_eq!(info.version, DISPLAY_OUTPUT_INFO_VERSION);
        // The test resource is not fb0, so it is connected but not primary.
        assert_eq!(info.flags, DISPLAY_OUTPUT_FLAG_CONNECTED);
        assert_eq!(info.connector_index, 0);
        assert_eq!(info.connector_count, 1);
        assert_eq!(info.kind, DISPLAY_CONNECTOR_KIND_UNKNOWN);
        assert_eq!((info.width, info.height), (1, 1));
        assert_eq!(info.refresh_mhz, 0);
        assert_eq!((info.physical_width_mm, info.physical_height_mm), (0, 0));
        assert_eq!(info.reserved, 0);
        let name = b"test-brightness-graphics";
        assert_eq!(&info.name[..name.len()], name);
        assert!(info.name[name.len()..].iter().all(|byte| *byte == 0));
    }

    #[test_case]
    fn display_output_info_rejects_short_buffers_and_truncates_long_ones() {
        let manager = DeviceManager::new_for_test();
        let graphics = Arc::new(TestBrightnessGraphicsDevice::new(42));
        let display = display_for_test(&manager, graphics);
        const RECORD_LEN: usize = core::mem::size_of::<DisplayOutputInfo>();

        let mut short = output_info_request(super::DISPLAY_OUTPUT_INFO_V1_SIZE as u32 - 1);
        assert_eq!(
            display
                .control(
                    display_commands::DISPLAY_GET_OUTPUT_INFO,
                    &mut short as *mut DisplayOutputInfo as usize,
                )
                .expect_err("short request must be rejected"),
            "Display output info buffer is too small"
        );
        assert_eq!(short.version, 0);

        // A version 1 client still provides exactly 80 bytes. Preserve its
        // neighboring memory while publishing the unchanged prefix.
        let mut legacy = [0xa5u8; 96];
        legacy[..4].copy_from_slice(&(super::DISPLAY_OUTPUT_INFO_V1_SIZE as u32).to_ne_bytes());
        display
            .control(
                display_commands::DISPLAY_GET_OUTPUT_INFO,
                legacy.as_mut_ptr() as usize,
            )
            .expect("the version 1 prefix remains supported");
        assert_eq!(u32::from_ne_bytes(legacy[..4].try_into().unwrap()), 80);
        assert!(legacy[80..].iter().all(|byte| *byte == 0xa5));

        // A newer caller may pass a larger record; the kernel writes only the
        // fields it knows and leaves the tail untouched.
        let mut buffer = [0xa5u8; RECORD_LEN + 16];
        let buffer_len = buffer.len() as u32;
        buffer[..4].copy_from_slice(&buffer_len.to_ne_bytes());
        display
            .control(
                display_commands::DISPLAY_GET_OUTPUT_INFO,
                buffer.as_mut_ptr() as usize,
            )
            .expect("larger request should succeed");
        assert_eq!(
            u32::from_ne_bytes(buffer[..4].try_into().unwrap()) as usize,
            RECORD_LEN
        );
        assert_eq!(
            u32::from_ne_bytes(buffer[4..8].try_into().unwrap()),
            DISPLAY_OUTPUT_INFO_VERSION
        );
        assert!(buffer[RECORD_LEN..].iter().all(|byte| *byte == 0xa5));

        assert_eq!(
            display
                .control(display_commands::DISPLAY_GET_OUTPUT_INFO, 0)
                .expect_err("null pointer must be rejected"),
            "Invalid argument pointer"
        );
    }

    /// Graphics device reporting two connectors with a long non-ASCII name.
    struct TwoConnectorGraphicsDevice(TestBrightnessGraphicsDevice);

    impl Device for TwoConnectorGraphicsDevice {
        fn device_type(&self) -> DeviceType {
            DeviceType::Graphics
        }

        fn name(&self) -> &'static str {
            "test-two-connector-graphics"
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

    impl ControlOps for TwoConnectorGraphicsDevice {}

    impl MemoryMappingOps for TwoConnectorGraphicsDevice {
        fn get_mapping_info(
            &self,
            offset: usize,
            length: usize,
        ) -> Result<crate::object::capability::MemoryMappingInfo, &'static str> {
            self.0.get_mapping_info(offset, length)
        }
    }

    impl Selectable for TwoConnectorGraphicsDevice {
        fn wait_until_ready(
            &self,
            interest: crate::object::capability::selectable::ReadyInterest,
            trapframe: &mut crate::arch::Trapframe,
            timeout_ticks: Option<u64>,
            min_wait_ticks: u64,
        ) -> SelectWaitOutcome {
            self.0
                .wait_until_ready(interest, trapframe, timeout_ticks, min_wait_ticks)
        }
    }

    impl GraphicsDevice for TwoConnectorGraphicsDevice {
        fn get_display_name(&self) -> &'static str {
            "test-two-connector-graphics"
        }

        fn get_framebuffer_config(&self) -> Result<FramebufferConfig, &'static str> {
            self.0.get_framebuffer_config()
        }

        fn get_framebuffer_address(&self) -> Result<u64, &'static str> {
            self.0.get_framebuffer_address()
        }

        fn present_framebuffer_region(
            &self,
            config: &FramebufferConfig,
            physical_addr: u64,
            region: crate::device::graphics::output::DisplayRegion,
        ) -> Result<(), &'static str> {
            self.0
                .present_framebuffer_region(config, physical_addr, region)
        }

        fn init_graphics(&self) -> Result<(), &'static str> {
            Ok(())
        }

        fn connectors(&self) -> alloc::vec::Vec<ConnectorInfo> {
            alloc::vec![
                ConnectorInfo {
                    index: 0,
                    // 29 ASCII bytes then a 3-byte character crossing byte 31.
                    name: "internal-panel-with-long-name\u{3042}".to_string(),
                    kind: ConnectorKind::Internal,
                    connected: true,
                    width: 2560,
                    height: 1600,
                    refresh_mhz: 60_000,
                    physical_size_mm: Some((300, 190)),
                },
                ConnectorInfo {
                    index: 1,
                    name: "dp0".to_string(),
                    kind: ConnectorKind::External,
                    connected: false,
                    width: 0,
                    height: 0,
                    refresh_mhz: 0,
                    physical_size_mm: None,
                },
            ]
        }
    }

    #[test_case]
    fn display_output_info_reports_connector_zero_of_a_multi_connector_device() {
        let manager = DeviceManager::new_for_test();
        let graphics = Arc::new(TwoConnectorGraphicsDevice(
            TestBrightnessGraphicsDevice::new(50),
        ));
        let display = display_for_test(&manager, graphics);
        let mut info = output_info_request(core::mem::size_of::<DisplayOutputInfo>() as u32);
        display
            .control(
                display_commands::DISPLAY_GET_OUTPUT_INFO,
                &mut info as *mut DisplayOutputInfo as usize,
            )
            .expect("output info should succeed");

        assert_eq!(info.connector_index, 0);
        assert_eq!(info.connector_count, 2);
        assert_eq!(info.kind, DISPLAY_CONNECTOR_KIND_INTERNAL);
        assert_eq!((info.width, info.height), (2560, 1600));
        assert_eq!(info.refresh_mhz, 60_000);
        assert_eq!(
            (info.physical_width_mm, info.physical_height_mm),
            (300, 190)
        );
        // The 3-byte character would straddle the 31-byte limit, so it is
        // dropped whole and the name stays valid UTF-8 with a NUL terminator.
        let expected = b"internal-panel-with-long-name";
        assert_eq!(&info.name[..expected.len()], expected);
        assert!(info.name[expected.len()..].iter().all(|byte| *byte == 0));
    }

    #[test_case]
    fn secondary_connector_endpoint_reports_its_own_connector() {
        let manager = DeviceManager::new_for_test();
        let graphics: Arc<dyn Device> = Arc::new(TwoConnectorGraphicsDevice(
            TestBrightnessGraphicsDevice::new(50),
        ));
        let source_device_id =
            manager.register_device_with_name("test-two-connector".to_string(), graphics);
        let resource = Arc::new(FramebufferResource::new_connector(
            source_device_id,
            "fb-test-1".to_string(),
            1,
        ));
        let display = DisplayCharDevice::new_with_device_manager(resource, &manager);

        let mut info = output_info_request(core::mem::size_of::<DisplayOutputInfo>() as u32);
        display
            .control(
                display_commands::DISPLAY_GET_OUTPUT_INFO,
                &mut info as *mut DisplayOutputInfo as usize,
            )
            .expect("output info should succeed on a disconnected connector");
        assert_eq!((info.connector_index, info.connector_count), (1, 2));
        assert_eq!(info.flags, 0);
        assert_eq!(&info.name[..4], b"dp0\0");

        // The default trait has no backing for connector 1, and the
        // primary-only controls are refused rather than reaching connector 0.
        let mut display_info = super::DisplayInfo::default();
        assert!(
            display
                .control(
                    display_commands::DISPLAY_GET_INFO,
                    &mut display_info as *mut super::DisplayInfo as usize,
                )
                .is_err()
        );
        assert_eq!(
            display
                .control(display_commands::DISPLAY_GET_BRIGHTNESS, 0)
                .expect_err("the test device has no backlight on connector 1"),
            "Display brightness control is not supported"
        );
        let mut swapchain = super::DisplaySwapchainInfo::default();
        assert_eq!(
            display
                .control(
                    display_commands::DISPLAY_GET_SWAPCHAIN,
                    &mut swapchain as *mut super::DisplaySwapchainInfo as usize,
                )
                .expect_err("direct scanout is primary-only"),
            "Direct scanout buffers are not supported on this connector"
        );
    }

    #[test_case]
    fn display_output_info_record_has_fixed_layout() {
        assert_eq!(core::mem::size_of::<DisplayOutputInfo>(), 336);
        assert_eq!(core::mem::align_of::<DisplayOutputInfo>(), 4);
        assert_eq!(core::mem::offset_of!(DisplayOutputInfo, device_key), 80);
    }

    #[test_case]
    fn display_brightness_control_rejects_out_of_range_percentages() {
        let manager = DeviceManager::new_for_test();
        let graphics = Arc::new(TestBrightnessGraphicsDevice::new(37));
        let display = display_for_test(&manager, graphics.clone());

        for percent in [101, 255, 256, usize::MAX] {
            assert_eq!(
                display
                    .control(display_commands::DISPLAY_SET_BRIGHTNESS, percent)
                    .expect_err("brightness above 100 must be rejected"),
                "Display brightness must be in the range 0..=100"
            );
        }
        assert_eq!(graphics.brightness.load(Ordering::Relaxed), 37);
    }

    #[test_case]
    fn display_brightness_control_rejects_invalid_driver_readback() {
        let manager = DeviceManager::new_for_test();
        let graphics = Arc::new(TestBrightnessGraphicsDevice::new(101));
        let display = display_for_test(&manager, graphics);
        assert_eq!(
            display.control(display_commands::DISPLAY_GET_BRIGHTNESS, 0),
            Err("Display driver returned an invalid brightness percentage")
        );
    }

    #[test_case]
    fn display_brightness_control_preserves_unsupported_default() {
        let manager = DeviceManager::new_for_test();
        let mut graphics = GenericGraphicsDevice::new("test-generic-graphics");
        graphics.set_framebuffer_config(FramebufferConfig::new(1, 1, PixelFormat::XRGB8888));
        graphics.set_framebuffer_address(0x1000);
        let display = display_for_test(&manager, Arc::new(graphics));

        assert_eq!(
            display
                .control(display_commands::DISPLAY_GET_BRIGHTNESS, 0)
                .expect_err("generic graphics must preserve the unsupported default"),
            "Display brightness control is not supported"
        );
    }
}
