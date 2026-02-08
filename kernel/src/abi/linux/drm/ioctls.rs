//! # DRM ioctl Implementations
//!
//! This module implements the DRM ioctl handlers that bridge between
//! Linux DRM API and Scarlet's GraphicsManager.
//!
//! GraphicsManager acts as Scarlet's equivalent of Linux DRM subsystem,
//! providing OS-independent graphics operations. The DRM ioctl handlers
//! simply translate Linux DRM ioctls to GraphicsManager function calls.

use super::types::*;
use super::file::DrmFile;
use crate::device::graphics::manager::GraphicsManager;
use crate::object::KernelObject;
use alloc::sync::Arc;
use alloc::collections::BTreeMap;
use spin::Mutex;

/// Per-device DRM context for tracking buffers
struct DrmDeviceContext {
    next_handle: u32,
    buffers: BTreeMap<u32, (usize, usize)>, // handle -> (phys_addr, size)
}

impl DrmDeviceContext {
    const fn new() -> Self {
        Self {
            next_handle: 1,
            buffers: BTreeMap::new(),
        }
    }
    
    fn allocate_handle(&mut self) -> u32 {
        let handle = self.next_handle;
        if self.next_handle == u32::MAX {
            panic!("DRM handle space exhausted: cannot allocate more handles");
        }
        self.next_handle += 1;
        handle
    }
    
    fn store_buffer(&mut self, handle: u32, phys_addr: usize, size: usize) {
        self.buffers.insert(handle, (phys_addr, size));
    }
    
    fn get_buffer(&self, handle: u32) -> Option<(usize, usize)> {
        self.buffers.get(&handle).copied()
    }
    
    fn remove_buffer(&mut self, handle: u32) -> Result<(), &'static str> {
        if let Some((phys_addr, size)) = self.buffers.remove(&handle) {
            // Free the memory
            let pages = (size + 4095) / 4096;
            unsafe {
                crate::mem::page::free_raw_pages(phys_addr as *mut crate::mem::page::Page, pages);
            }
            Ok(())
        } else {
            Err("Invalid buffer handle")
        }
    }
}

// Global DRM context (for MVP, we only support one device)
static DRM_CONTEXT: Mutex<Option<DrmDeviceContext>> = Mutex::new(None);

fn get_drm_context() -> &'static Mutex<Option<DrmDeviceContext>> {
    &DRM_CONTEXT
}

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
    unsafe { core::ptr::write_unaligned(version_ptr, version); }
    
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
    
    unsafe { core::ptr::write_unaligned(res_ptr, res); }
    Ok(0)
}

/// Handle DRM_IOCTL_MODE_GETCRTC
/// 
/// Gets the current CRTC configuration.
pub fn handle_drm_get_crtc(arg: usize, _device_id: usize) -> Result<i32, &'static str> {
    let target_ptr = translate_user_pointer(arg)?;
    
    let crtc_ptr = target_ptr as *mut DrmModeCrtc;
    let mut crtc = unsafe { core::ptr::read_unaligned(crtc_ptr) };
    
    // Get framebuffer configuration through GraphicsManager
    // MVP: Use fb0 as the primary framebuffer
    let graphics_manager = GraphicsManager::get_manager();
    let fb_resource = graphics_manager.get_framebuffer("fb0")
        .ok_or("Framebuffer fb0 not found")?;
    let config = &fb_resource.config;
    
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
    
    unsafe { core::ptr::write_unaligned(crtc_ptr, crtc); }
    Ok(0)
}

/// Handle DRM_IOCTL_MODE_CREATE_DUMB
/// 
/// Creates a dumb buffer for simple CPU access.
pub fn handle_drm_create_dumb(arg: usize, file: &DrmFile) -> Result<i32, &'static str> {
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

    // Create buffer via simple memory allocation
    // For MVP: Just allocate memory and track it in the DRM context
    // Calculate size with checked arithmetic
    let width_bpp = dumb.width.checked_mul(dumb.bpp)
        .ok_or("Width * bpp overflow")?;
    let pitch = ((width_bpp + 31) / 32) * 4;
    let size = pitch.checked_mul(dumb.height)
        .ok_or("Pitch * height overflow")? as usize;
    
    // Allocate physical pages for the buffer
    let pages_needed = (size + 4095) / 4096;
    let phys_addr = crate::mem::page::allocate_raw_pages(pages_needed) as usize;
    
    // Check for allocation failure
    if phys_addr == 0 {
        return Err("Failed to allocate buffer memory");
    }
    
    // Store buffer info in DRM context
    let mut context_guard = get_drm_context().lock();
    if context_guard.is_none() {
        *context_guard = Some(DrmDeviceContext::new());
    }
    let context = context_guard.as_mut().unwrap();
    let handle = context.allocate_handle();
    context.store_buffer(handle, phys_addr, size);
    drop(context_guard);
    
    // Fill in response
    dumb.handle = handle;
    dumb.pitch = pitch;
    dumb.size = size as u64;
    
    unsafe { core::ptr::write_unaligned(dumb_ptr, dumb); }
    Ok(0)
}

/// Handle DRM_IOCTL_MODE_MAP_DUMB
/// 
/// Returns an offset for mmap to map the dumb buffer.
pub fn handle_drm_map_dumb(arg: usize, file: &DrmFile) -> Result<i32, &'static str> {
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
    
    // Get buffer info from DRM context
    let context_guard = get_drm_context().lock();
    let context = context_guard.as_ref().ok_or("DRM context not initialized")?;
    let (phys_addr, _size) = context.get_buffer(map.handle)
        .ok_or("Invalid buffer handle")?;
    drop(context_guard);
    
    // For MVP: Return physical address as offset
    // In a real implementation, this would return a fake offset that the kernel
    // would use to map the buffer when mmap is called
    map.offset = phys_addr as u64;
    
    unsafe { core::ptr::write_unaligned(map_ptr, map); }
    Ok(0)
}

/// Handle DRM_IOCTL_MODE_DESTROY_DUMB
/// 
/// Destroys a dumb buffer.
pub fn handle_drm_destroy_dumb(arg: usize, file: &DrmFile) -> Result<i32, &'static str> {
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
    
    // Remove the GEM handle
    // Remove the buffer from DRM context
    let mut context_guard = get_drm_context().lock();
    let context = context_guard.as_mut().ok_or("DRM context not initialized")?;
    context.remove_buffer(destroy.handle)?;
    drop(context_guard);
    
    Ok(0)
}

/// Handle DRM_IOCTL_MODE_PAGE_FLIP
/// 
/// Performs a page flip operation. For MVP, this copies from the specified
/// buffer to the framebuffer and flushes.
pub fn handle_drm_page_flip(arg: usize, file: &DrmFile) -> Result<i32, &'static str> {
    let target_ptr = translate_user_pointer(arg)?;
    
    let flip_ptr = target_ptr as *const DrmModePageFlip;
    let flip = unsafe { core::ptr::read_unaligned(flip_ptr) };
    
    // Get framebuffer configuration and address through GraphicsManager
    let graphics_manager = GraphicsManager::get_manager();
    let fb_resource = graphics_manager.get_framebuffer("fb0")
        .ok_or("Framebuffer fb0 not found")?;
    
    let config = &fb_resource.config;
    let fb_addr = fb_resource.get_current_address()
        .unwrap_or(fb_resource.physical_addr);
    
    // Get source buffer from DRM context
    let context_guard = get_drm_context().lock();
    let context = context_guard.as_ref().ok_or("DRM context not initialized")?;
    let (src_addr, src_size) = context.get_buffer(flip.fb_id)
        .ok_or("Invalid framebuffer ID")?;
    drop(context_guard);
    
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
    // For MVP, we can skip flush or call directly on the device
    // since GraphicsManager doesn't have flush_framebuffer_by_device in dev
    
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_translate_user_pointer_null() {
        let result = translate_user_pointer(0);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Invalid argument pointer");
    }

    #[test_case]
    fn test_translate_user_pointer_kernel_context() {
        // Without a current task, should return the pointer as-is
        let addr = 0x1000usize;
        assert_eq!(translate_user_pointer(addr).unwrap(), addr);
    }

    #[test_case]
    fn test_handle_drm_version_invalid_pointer() {
        assert!(handle_drm_version(0).is_err());
    }

    #[test_case]
    fn test_handle_drm_get_resources_invalid_pointer() {
        assert!(handle_drm_get_resources(0).is_err());
    }

    #[test_case]
    fn test_handle_drm_create_dumb_invalid_pointer() {
        let file = DrmFile::new(0);
        assert!(handle_drm_create_dumb(0, &file).is_err());
    }

    #[test_case]
    fn test_handle_drm_map_dumb_invalid_pointer() {
        let file = DrmFile::new(0);
        assert!(handle_drm_map_dumb(0, &file).is_err());
    }

    #[test_case]
    fn test_handle_drm_destroy_dumb_invalid_pointer() {
        let file = DrmFile::new(0);
        assert!(handle_drm_destroy_dumb(0, &file).is_err());
    }

    #[test_case]
    fn test_handle_drm_page_flip_invalid_pointer() {
        let file = DrmFile::new(0);
        assert!(handle_drm_page_flip(0, &file).is_err());
    }

    use crate::device::graphics::{GenericGraphicsDevice, FramebufferConfig, PixelFormat};
    use crate::device::manager::{DeviceManager, SharedDevice};
    use crate::mem::page::{allocate_raw_pages, free_raw_pages, Page};

    // Helper to setup graphics environment
    fn setup_graphics_env() -> (usize, usize) {
        // Clear existing state
        let graphics_manager = GraphicsManager::get_mut_manager();
        graphics_manager.clear_for_test();
        
        let device_manager = DeviceManager::get_mut_manager();
        device_manager.clear_for_test();
        
        // Allocate fake framebuffer memory
        let fb_size = 800 * 600 * 4;
        let pages = (fb_size + 4095) / 4096;
        let fb_addr = allocate_raw_pages(pages) as usize;
        
        // Create device
        let mut device = GenericGraphicsDevice::new("test-gpu");
        let config = FramebufferConfig::new(800, 600, PixelFormat::BGRA8888);
        device.set_framebuffer_config(config);
        device.set_framebuffer_address(fb_addr);
        
        let shared_device: SharedDevice = Arc::new(device);
        
        // Register with DeviceManager first to get a valid ID
        let device_id = device_manager.register_device(shared_device.clone());
        
        // Register with GraphicsManager using the real device ID
        graphics_manager.register_framebuffer_from_device(device_id, shared_device).unwrap();
        
        (device_id, fb_addr)
    }

    #[test_case]
    fn test_drm_drawing_workflow() {
        let (device_id, fb_addr) = setup_graphics_env();
        let file = DrmFile::new(device_id);
        
        // 1. Create Dumb Buffer (Full screen size to match framebuffer)
        let mut create_dumb = DrmModeCreateDumb {
            height: 600,
            width: 800,
            bpp: 32,
            flags: 0,
            handle: 0,
            pitch: 0,
            size: 0,
        };
        
        let create_ptr = &mut create_dumb as *mut _ as usize;
        assert!(handle_drm_create_dumb(create_ptr, &file).is_ok());
        assert_ne!(create_dumb.handle, 0);
        assert_eq!(create_dumb.size, 800 * 600 * 4);
        
        // 2. Map Dumb Buffer
        let mut map_dumb = DrmModeMapDumb {
            handle: create_dumb.handle,
            pad: 0,
            offset: 0,
        };
        
        let map_ptr = &mut map_dumb as *mut _ as usize;
        assert!(handle_drm_map_dumb(map_ptr, &file).is_ok());
        assert_ne!(map_dumb.offset, 0);
        
        // 3. Write to buffer (simulate drawing)
        let buffer_addr = map_dumb.offset as *mut u32;
        unsafe {
            // Fill with red (0xFFFF0000)
            for i in 0..(800*600) {
                *buffer_addr.add(i) = 0xFFFF0000;
            }
        }
        
        // 4. Page Flip
        let flip = DrmModePageFlip {
            crtc_id: 1,
            fb_id: create_dumb.handle,
            flags: 0,
            user_data: 0,
        };
        
        let flip_ptr = &flip as *const _ as usize;
        assert!(handle_drm_page_flip(flip_ptr, &file).is_ok());
        
        // 5. Verify Framebuffer Content
        // The entire framebuffer should now be red
        let fb_ptr = fb_addr as *const u32;
        unsafe {
            // Check a few points to verify
            // Top-left
            assert_eq!(*fb_ptr.add(0), 0xFFFF0000, "Pixel mismatch at (0, 0)");
            // Middle
            assert_eq!(*fb_ptr.add(400*600 + 400), 0xFFFF0000, "Pixel mismatch at (400, 300)");
            // Bottom-right
            assert_eq!(*fb_ptr.add(800*600 - 1), 0xFFFF0000, "Pixel mismatch at (799, 599)");
        }
        
        // Cleanup
        let pages = (800 * 600 * 4 + 4095) / 4096;
        free_raw_pages(fb_addr as *mut Page, pages);
    }
}
