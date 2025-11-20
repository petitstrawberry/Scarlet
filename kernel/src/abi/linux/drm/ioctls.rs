//! # DRM ioctl Implementations
//!
//! This module implements the DRM ioctl handlers that bridge between
//! Linux DRM API and Scarlet's GraphicsDevice abstraction.

use super::types::*;
use crate::device::graphics::{GraphicsDevice, PixelFormat};
use crate::device::manager::DeviceManager;
use alloc::vec::Vec;

/// DRM ioctl command numbers
pub mod commands {
    /// DRM_IOCTL_VERSION - Get driver version
    pub const DRM_IOCTL_VERSION: u32 = 0xC0406400;
    /// DRM_IOCTL_MODE_GETRESOURCES - Get mode resources
    pub const DRM_IOCTL_MODE_GETRESOURCES: u32 = 0xC04064A0;
    /// DRM_IOCTL_MODE_GETCRTC - Get CRTC configuration
    pub const DRM_IOCTL_MODE_GETCRTC: u32 = 0xC06864A1;
    /// DRM_IOCTL_MODE_SETCRTC - Set CRTC configuration
    pub const DRM_IOCTL_MODE_SETCRTC: u32 = 0xC06864A2;
    /// DRM_IOCTL_MODE_CREATE_DUMB - Create dumb buffer
    pub const DRM_IOCTL_MODE_CREATE_DUMB: u32 = 0xC02064B2;
    /// DRM_IOCTL_MODE_MAP_DUMB - Map dumb buffer
    pub const DRM_IOCTL_MODE_MAP_DUMB: u32 = 0xC01064B3;
    /// DRM_IOCTL_MODE_DESTROY_DUMB - Destroy dumb buffer
    pub const DRM_IOCTL_MODE_DESTROY_DUMB: u32 = 0xC00464B4;
    /// DRM_IOCTL_MODE_PAGE_FLIP - Perform page flip
    pub const DRM_IOCTL_MODE_PAGE_FLIP: u32 = 0xC01064B0;
    /// DRM_IOCTL_MODE_GETCONNECTOR - Get connector information
    pub const DRM_IOCTL_MODE_GETCONNECTOR: u32 = 0xC05064A7;
    /// DRM_IOCTL_MODE_GETENCODER - Get encoder information
    pub const DRM_IOCTL_MODE_GETENCODER: u32 = 0xC01464A6;
}

/// DRM device context
/// 
/// This structure maintains the state for a DRM device, including
/// buffers created for page flipping and mappings.
pub struct DrmDeviceContext {
    /// Device ID in the DeviceManager
    pub device_id: usize,
    /// Created dumb buffers (handle -> (address, size))
    pub dumb_buffers: Vec<(u32, usize, usize)>,
    /// Next handle to allocate
    pub next_handle: u32,
}

impl DrmDeviceContext {
    /// Create a new DRM device context
    pub fn new(device_id: usize) -> Self {
        Self {
            device_id,
            dumb_buffers: Vec::new(),
            next_handle: 1,
        }
    }
    
    /// Allocate a new handle
    fn allocate_handle(&mut self) -> u32 {
        let handle = self.next_handle;
        if self.next_handle == u32::MAX {
            panic!("DRM handle space exhausted: cannot allocate more handles");
        }
        self.next_handle += 1;
        handle
    }
    
    /// Store a dumb buffer
    fn store_buffer(&mut self, handle: u32, address: usize, size: usize) {
        self.dumb_buffers.push((handle, address, size));
    }
    
    /// Get buffer by handle
    fn get_buffer(&self, handle: u32) -> Option<(usize, usize)> {
        self.dumb_buffers.iter()
            .find(|(h, _, _)| *h == handle)
            .map(|(_, addr, size)| (*addr, *size))
    }
    
    /// Remove buffer by handle
    fn remove_buffer(&mut self, handle: u32) -> Option<(usize, usize)> {
        if let Some(pos) = self.dumb_buffers.iter().position(|(h, _, _)| *h == handle) {
            let (_, addr, size) = self.dumb_buffers.remove(pos);
            Some((addr, size))
        } else {
            None
        }
    }
}

/// Translates a user pointer to a kernel-accessible address.
/// 
/// Returns the translated address or an error if translation fails.
fn translate_user_pointer(arg: usize) -> Result<usize, &'static str> {
    if arg == 0 {
        return Err("Invalid argument pointer");
    }
    if let Some(current_task) = crate::task::mytask() {
        current_task.vm_manager.translate_vaddr(arg)
            .ok_or("Invalid user pointer")
    } else {
        Ok(arg)
    }
}

/// Handle DRM_IOCTL_VERSION
/// 
/// Returns version information about the DRM driver.
pub fn handle_drm_version(arg: usize) -> Result<i32, &'static str> {
    let target_ptr = translate_user_pointer(arg)?;
    
    let version_ptr = target_ptr as *mut DrmVersion;
    let mut version = unsafe { core::ptr::read_unaligned(version_ptr) };
    
    // Set version numbers
    version.version_major = 1;
    version.version_minor = 0;
    version.version_patchlevel = 0;
    
    // Driver name: "scarlet"
    let name = b"scarlet\0";
    if version.name_len == 0 {
        version.name_len = name.len();
    } else if version.name != 0 {
        let name_len = version.name_len.min(name.len());
        // Copy name to user space
        if let Some(current_task) = crate::task::mytask() {
            if let Some(name_target) = current_task.vm_manager.translate_vaddr(version.name) {
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        name.as_ptr(),
                        name_target as *mut u8,
                        name_len
                    );
                }
            }
        }
    }
    
    // Date string
    let date = b"20250120\0";
    if version.date_len == 0 {
        version.date_len = date.len();
    } else if version.date != 0 {
        let date_len = version.date_len.min(date.len());
        if let Some(current_task) = crate::task::mytask() {
            if let Some(date_target) = current_task.vm_manager.translate_vaddr(version.date) {
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        date.as_ptr(),
                        date_target as *mut u8,
                        date_len
                    );
                }
            }
        }
    }
    
    // Description
    let desc = b"Scarlet DRM Driver\0";
    if version.desc_len == 0 {
        version.desc_len = desc.len();
    } else if version.desc != 0 {
        let desc_len = version.desc_len.min(desc.len());
        if let Some(current_task) = crate::task::mytask() {
            if let Some(desc_target) = current_task.vm_manager.translate_vaddr(version.desc) {
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        desc.as_ptr(),
                        desc_target as *mut u8,
                        desc_len
                    );
                }
            }
        }
    }
    
    // Write back the version structure
    unsafe { core::ptr::write(version_ptr, version); }
    
    Ok(0)
}

/// Handle DRM_IOCTL_MODE_GETRESOURCES
/// 
/// Returns the available mode resources (CRTCs, connectors, encoders).
pub fn handle_drm_get_resources(arg: usize) -> Result<i32, &'static str> {
    if arg == 0 {
        return Err("Invalid argument pointer");
    }
    
    let target_ptr = if let Some(current_task) = crate::task::mytask() {
        current_task.vm_manager.translate_vaddr(arg)
            .ok_or("Invalid user pointer")?
    } else {
        arg
    };
    
    let res_ptr = target_ptr as *mut DrmModeCardRes;
    let mut res = unsafe { core::ptr::read(res_ptr) };
    
    // For MVP, we report 1 CRTC and 1 connector
    res.count_crtcs = 1;
    res.count_connectors = 1;
    res.count_encoders = 1;
    res.count_fbs = 0;
    
    // Set min/max dimensions based on common display capabilities
    res.min_width = 320;
    res.min_height = 240;
    res.max_width = 4096;
    res.max_height = 4096;
    
    // If user provided arrays, fill them in
    if res.crtc_id_ptr != 0 && res.count_crtcs > 0 {
        if let Some(current_task) = crate::task::mytask() {
            if let Some(crtc_ptr) = current_task.vm_manager.translate_vaddr(res.crtc_id_ptr as usize) {
                unsafe {
                    *(crtc_ptr as *mut u32) = 1; // CRTC ID 1
                }
            }
        }
    }
    
    if res.connector_id_ptr != 0 && res.count_connectors > 0 {
        if let Some(current_task) = crate::task::mytask() {
            if let Some(conn_ptr) = current_task.vm_manager.translate_vaddr(res.connector_id_ptr as usize) {
                unsafe {
                    *(conn_ptr as *mut u32) = 1; // Connector ID 1
                }
            }
        }
    }
    
    if res.encoder_id_ptr != 0 && res.count_encoders > 0 {
        if let Some(current_task) = crate::task::mytask() {
            if let Some(enc_ptr) = current_task.vm_manager.translate_vaddr(res.encoder_id_ptr as usize) {
                unsafe {
                    *(enc_ptr as *mut u32) = 1; // Encoder ID 1
                }
            }
        }
    }
    
    unsafe { core::ptr::write(res_ptr, res); }
    Ok(0)
}

/// Handle DRM_IOCTL_MODE_GETCRTC
/// 
/// Gets the current CRTC configuration.
pub fn handle_drm_get_crtc(arg: usize, device_id: usize) -> Result<i32, &'static str> {
    if arg == 0 {
        return Err("Invalid argument pointer");
    }
    
    let target_ptr = if let Some(current_task) = crate::task::mytask() {
        current_task.vm_manager.translate_vaddr(arg)
            .ok_or("Invalid user pointer")?
    } else {
        arg
    };
    
    let crtc_ptr = target_ptr as *mut DrmModeCrtc;
    let mut crtc = unsafe { core::ptr::read(crtc_ptr) };
    
    // Get the graphics device
    let device_manager = DeviceManager::get_manager();
    let device = device_manager.get_device(device_id)
        .ok_or("Device not found")?;
    
    let graphics_device = device.as_graphics_device()
        .ok_or("Not a graphics device")?;
    
    // Get current framebuffer configuration
    let config = graphics_device.get_framebuffer_config()?;
    
    // Fill in mode information
    crtc.mode.hdisplay = config.width as u16;
    crtc.mode.vdisplay = config.height as u16;
    crtc.mode.htotal = config.width as u16;
    crtc.mode.vtotal = config.height as u16;
    crtc.mode.vrefresh = 60; // Assume 60Hz
    crtc.mode_valid = 1;
    
    crtc.x = 0;
    crtc.y = 0;
    crtc.gamma_size = 256;
    
    unsafe { core::ptr::write(crtc_ptr, crtc); }
    Ok(0)
}

/// Handle DRM_IOCTL_MODE_CREATE_DUMB
/// 
/// Creates a dumb buffer for simple CPU access.
pub fn handle_drm_create_dumb(arg: usize, ctx: &mut DrmDeviceContext) -> Result<i32, &'static str> {
    if arg == 0 {
        return Err("Invalid argument pointer");
    }
    
    let target_ptr = if let Some(current_task) = crate::task::mytask() {
        current_task.vm_manager.translate_vaddr(arg)
            .ok_or("Invalid user pointer")?
    } else {
        arg
    };
    
    let dumb_ptr = target_ptr as *mut DrmModeCreateDumb;
    let mut dumb = unsafe { core::ptr::read(dumb_ptr) };

    // Validate user-provided dimensions
    // Reasonable maximums: width/height <= 16384, bpp in {8, 16, 24, 32}
    if dumb.width == 0 || dumb.width > 16384 {
        return Err("Invalid dumb buffer width");
    }
    if dumb.height == 0 || dumb.height > 16384 {
        return Err("Invalid dumb buffer height");
    }
    match dumb.bpp {
        8 | 16 | 24 | 32 => {},
        _ => return Err("Invalid dumb buffer bpp"),
    }

    // Calculate size with checked arithmetic
    let width_bpp = dumb.width.checked_mul(dumb.bpp)
        .ok_or("Width * bpp overflow")?;
    let pitch = ((width_bpp + 31) / 32) * 4;
    let size = pitch.checked_mul(dumb.height)
        .ok_or("Pitch * height overflow")? as usize;
    // Allocate memory for the buffer
    let pages = (size + 4095) / 4096;
    let addr = crate::mem::page::allocate_raw_pages(pages) as usize;
    
    // Check for allocation failure
    if addr == 0 {
        return Err("Failed to allocate buffer memory");
    }
    // Allocate handle and store buffer
    let handle = ctx.allocate_handle();
    ctx.store_buffer(handle, addr, size);
    
    // Fill in response
    dumb.handle = handle;
    dumb.pitch = pitch;
    dumb.size = size as u64;
    
    unsafe { core::ptr::write(dumb_ptr, dumb); }
    Ok(0)
}

/// Handle DRM_IOCTL_MODE_MAP_DUMB
/// 
/// Returns an offset for mmap to map the dumb buffer.
pub fn handle_drm_map_dumb(arg: usize, ctx: &DrmDeviceContext) -> Result<i32, &'static str> {
    if arg == 0 {
        return Err("Invalid argument pointer");
    }
    
    let target_ptr = if let Some(current_task) = crate::task::mytask() {
        current_task.vm_manager.translate_vaddr(arg)
            .ok_or("Invalid user pointer")?
    } else {
        arg
    };
    
    let map_ptr = target_ptr as *mut DrmModeMapDumb;
    let mut map = unsafe { core::ptr::read(map_ptr) };
    
    // Get buffer address
    let (addr, _size) = ctx.get_buffer(map.handle)
        .ok_or("Invalid buffer handle")?;
    
    // Return the address as the offset
    // In a real implementation, this would be a fake offset that the mmap
    // implementation would translate to the actual physical address
    map.offset = addr as u64;
    
    unsafe { core::ptr::write(map_ptr, map); }
    Ok(0)
}

/// Handle DRM_IOCTL_MODE_DESTROY_DUMB
/// 
/// Destroys a dumb buffer.
pub fn handle_drm_destroy_dumb(arg: usize, ctx: &mut DrmDeviceContext) -> Result<i32, &'static str> {
    if arg == 0 {
        return Err("Invalid argument pointer");
    }
    
    let target_ptr = if let Some(current_task) = crate::task::mytask() {
        current_task.vm_manager.translate_vaddr(arg)
            .ok_or("Invalid user pointer")?
    } else {
        arg
    };
    
    let destroy_ptr = target_ptr as *const DrmModeDestroyDumb;
    let destroy = unsafe { core::ptr::read(destroy_ptr) };
    
    // Remove and free the buffer
    let (addr, size) = ctx.remove_buffer(destroy.handle)
        .ok_or("Invalid buffer handle")?;
    
    // Free the memory
    let pages = (size + 4095) / 4096;
    unsafe {
        crate::mem::page::free_raw_pages(addr as *mut crate::mem::page::Page, pages);
    }
    
    Ok(0)
}

/// Handle DRM_IOCTL_MODE_PAGE_FLIP
/// 
/// Performs a page flip operation. For MVP, this copies from the specified
/// buffer to the framebuffer and flushes.
pub fn handle_drm_page_flip(arg: usize, ctx: &DrmDeviceContext) -> Result<i32, &'static str> {
    if arg == 0 {
        return Err("Invalid argument pointer");
    }
    
    let target_ptr = if let Some(current_task) = crate::task::mytask() {
        current_task.vm_manager.translate_vaddr(arg)
            .ok_or("Invalid user pointer")?
    } else {
        arg
    };
    
    let flip_ptr = target_ptr as *const DrmModePageFlip;
    let flip = unsafe { core::ptr::read(flip_ptr) };
    
    // Get the graphics device
    let device_manager = DeviceManager::get_manager();
    let device = device_manager.get_device(ctx.device_id)
        .ok_or("Device not found")?;
    
    let graphics_device = device.as_graphics_device()
        .ok_or("Not a graphics device")?;
    
    // Check if device supports hardware page flipping
    // For MVP, we always use the memcpy + flush approach
    // In the future, we can check for PageFlipCapable trait here
    
    // Get framebuffer configuration
    let config = graphics_device.get_framebuffer_config()?;
    let fb_addr = graphics_device.get_framebuffer_address()?;
    
    // Get source buffer (fb_id is treated as handle for dumb buffers)
    let (src_addr, src_size) = ctx.get_buffer(flip.fb_id)
        .ok_or("Invalid framebuffer ID")?;
    
    // Calculate copy size
    let fb_size = config.size();
    let copy_size = src_size.min(fb_size);
    
    // Copy from source buffer to framebuffer
    unsafe {
        core::ptr::copy_nonoverlapping(
            src_addr as *const u8,
            fb_addr as *mut u8,
            copy_size
        );
    }
    
    // Flush the framebuffer
    graphics_device.flush_framebuffer(0, 0, config.width, config.height)?;
    
    Ok(0)
}
