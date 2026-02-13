//! Linux DRM device implementation
//!
//! This module provides a Linux-compatible DRM device that maps
//! DRM ioctls to Scarlet's generic graphics device traits.

use alloc::{collections::BTreeMap, format, string::String, sync::Arc, vec::Vec};
use spin::RwLock;

use crate::device::{
    Device, DeviceType,
    char::CharDevice,
    graphics::manager::{FramebufferResource, GraphicsManager},
    manager::DeviceManager,
};
use crate::object::capability::{ControlOps, MemoryMappingOps, Selectable};

use super::ioctl::{
    DrmModeCardRes, DrmModeCreateDumb, DrmModeCrtc, DrmModeDestroyDumb, DrmModeInfo,
    DrmModeMapDumb, DrmModePageFlip, DrmVersion,
};

/// DRM device for Linux compatibility
///
/// This device provides a Linux DRM-compatible interface that maps
/// DRM ioctls to Scarlet's generic graphics device traits.
pub struct DrmDevice {
    device_id: usize,
    framebuffer_name: String,
    dumb_buffers: RwLock<BTreeMap<u32, DumbBufferInfo>>,
    next_handle: RwLock<u32>,
}

/// Information about a dumb buffer
#[derive(Debug, Clone)]
#[allow(dead_code)] // Fields used for DRM buffer tracking
struct DumbBufferInfo {
    handle: u32,
    width: u32,
    height: u32,
    bpp: u32,
    pitch: u32,
    size: u64,
    physical_addr: Option<usize>,
}

impl DrmDevice {
    /// Create a new DRM device for the specified framebuffer
    pub fn new(framebuffer_name: String) -> Option<Self> {
        let graphics_manager = GraphicsManager::get_manager();
        let fb_resource = graphics_manager.get_framebuffer(&framebuffer_name)?;

        Some(Self {
            device_id: fb_resource.source_device_id,
            framebuffer_name,
            dumb_buffers: RwLock::new(BTreeMap::new()),
            next_handle: RwLock::new(1),
        })
    }

    /// Get the underlying graphics device
    fn get_graphics_device(&self) -> Option<Arc<dyn Device>> {
        let device_manager = DeviceManager::get_manager();
        device_manager.get_device(self.device_id)
    }

    /// Get the framebuffer resource
    fn get_framebuffer_resource(&self) -> Option<Arc<FramebufferResource>> {
        let graphics_manager = GraphicsManager::get_manager();
        graphics_manager.get_framebuffer(&self.framebuffer_name)
    }

    /// Allocate a new dumb buffer handle
    fn alloc_handle(&self) -> u32 {
        let mut handle = self.next_handle.write();
        let result = *handle;
        *handle = handle.wrapping_add(1);
        if *handle == 0 {
            *handle = 1;
        }
        result
    }

    /// Handle DRM_VERSION ioctl
    fn handle_version(&self, arg: usize) -> Result<i32, &'static str> {
        let user_ptr = arg as *mut DrmVersion;
        if user_ptr.is_null() {
            return Err("Invalid argument pointer");
        }

        let version = DrmVersion {
            version_major: 1,
            version_minor: 0,
            version_patchlevel: 0,
            name_len: 7,
            name: "scarlet\0".as_bytes().as_ptr() as u64,
            date_len: 11,
            date: "20250101\0".as_bytes().as_ptr() as u64,
            desc_len: 32,
            desc: "Scarlet DRM Compatibility Layer\0".as_bytes().as_ptr() as u64,
        };

        unsafe {
            core::ptr::write(user_ptr, version);
        }

        Ok(0)
    }

    /// Handle DRM_MODE_GETRESOURCES ioctl
    fn handle_get_resources(&self, arg: usize) -> Result<i32, &'static str> {
        let user_ptr = arg as *mut DrmModeCardRes;
        if user_ptr.is_null() {
            return Err("Invalid argument pointer");
        }

        let fb_resource = match self.get_framebuffer_resource() {
            Some(res) => res,
            None => return Err("Framebuffer not found"),
        };

        let config = &fb_resource.config;

        let resources = DrmModeCardRes {
            fb_id_ptr: 0,
            crtc_id_ptr: 0,
            connector_id_ptr: 0,
            encoder_id_ptr: 0,
            count_fbs: 1,
            count_crtcs: 1,
            count_connectors: 1,
            count_encoders: 1,
            min_width: 1,
            max_width: config.width,
            min_height: 1,
            max_height: config.height,
        };

        unsafe {
            core::ptr::write(user_ptr, resources);
        }

        Ok(0)
    }

    /// Handle DRM_MODE_GETCRTC ioctl
    fn handle_get_crtc(&self, arg: usize) -> Result<i32, &'static str> {
        let user_ptr = arg as *mut DrmModeCrtc;
        if user_ptr.is_null() {
            return Err("Invalid argument pointer");
        }

        unsafe {
            let mut crtc = core::ptr::read(user_ptr);

            let fb_resource = match self.get_framebuffer_resource() {
                Some(res) => res,
                None => return Err("Framebuffer not found"),
            };

            let config = &fb_resource.config;

            crtc.fb_id = 1; // Default framebuffer ID
            crtc.x = 0;
            crtc.y = 0;
            crtc.mode_valid = 1;
            crtc.mode = DrmModeInfo::new();
            crtc.mode.hdisplay = config.width as u16;
            crtc.mode.vdisplay = config.height as u16;

            core::ptr::write(user_ptr, crtc);
        }

        Ok(0)
    }

    /// Handle DRM_MODE_SETCRTC ioctl
    fn handle_set_crtc(&self, arg: usize) -> Result<i32, &'static str> {
        let user_ptr = arg as *const DrmModeCrtc;
        if user_ptr.is_null() {
            return Err("Invalid argument pointer");
        }

        unsafe {
            let _crtc = core::ptr::read(user_ptr);
        }

        Ok(0)
    }

    /// Handle DRM_MODE_CREATE_DUMB ioctl
    fn handle_create_dumb(&self, arg: usize) -> Result<i32, &'static str> {
        let user_ptr = arg as *mut DrmModeCreateDumb;
        if user_ptr.is_null() {
            return Err("Invalid argument pointer");
        }

        unsafe {
            let req = core::ptr::read(user_ptr);

            let handle = self.alloc_handle();
            let pitch = req.width * req.bpp.div_ceil(8);
            let size = (pitch * req.height) as u64;

            let info = DumbBufferInfo {
                handle,
                width: req.width,
                height: req.height,
                bpp: req.bpp,
                pitch,
                size,
                physical_addr: None,
            };

            self.dumb_buffers.write().insert(handle, info);

            let resp = DrmModeCreateDumb {
                height: req.height,
                width: req.width,
                bpp: req.bpp,
                flags: req.flags,
                handle,
                pitch,
                size,
            };

            core::ptr::write(user_ptr, resp);
        }

        Ok(0)
    }

    /// Handle DRM_MODE_MAP_DUMB ioctl
    fn handle_map_dumb(&self, arg: usize) -> Result<i32, &'static str> {
        let user_ptr = arg as *mut DrmModeMapDumb;
        if user_ptr.is_null() {
            return Err("Invalid argument pointer");
        }

        unsafe {
            let req = core::ptr::read(user_ptr);

            let dumb_buffers = self.dumb_buffers.read();
            let _info = dumb_buffers
                .get(&req.handle)
                .ok_or("Invalid dumb buffer handle")?;

            let resp = DrmModeMapDumb {
                handle: req.handle,
                pad: 0,
                offset: 0,
            };

            core::ptr::write(user_ptr, resp);
        }

        Ok(0)
    }

    /// Handle DRM_MODE_DESTROY_DUMB ioctl
    fn handle_destroy_dumb(&self, arg: usize) -> Result<i32, &'static str> {
        let user_ptr = arg as *const DrmModeDestroyDumb;
        if user_ptr.is_null() {
            return Err("Invalid argument pointer");
        }

        unsafe {
            let req = core::ptr::read(user_ptr);
            self.dumb_buffers.write().remove(&req.handle);
        }

        Ok(0)
    }

    /// Handle DRM_MODE_PAGE_FLIP ioctl
    fn handle_page_flip(&self, arg: usize) -> Result<i32, &'static str> {
        let user_ptr = arg as *const DrmModePageFlip;
        if user_ptr.is_null() {
            return Err("Invalid argument pointer");
        }

        unsafe {
            let _req = core::ptr::read(user_ptr);
        }

        let _device = self.get_graphics_device();
        Ok(0)
    }
}

impl Device for DrmDevice {
    fn device_type(&self) -> DeviceType {
        DeviceType::Char
    }

    fn name(&self) -> &'static str {
        "drm"
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
        self
    }

    fn as_char_device(&self) -> Option<&dyn CharDevice> {
        Some(self)
    }
}

impl CharDevice for DrmDevice {
    fn read_byte(&self) -> Option<u8> {
        None
    }

    fn write_byte(&self, _byte: u8) -> Result<(), &'static str> {
        Err("Write not supported on DRM device")
    }

    fn can_read(&self) -> bool {
        true
    }

    fn can_write(&self) -> bool {
        true
    }

    fn read_at(&self, _position: u64, _buffer: &mut [u8]) -> Result<usize, &'static str> {
        Ok(0)
    }

    fn write_at(&self, _position: u64, _buffer: &[u8]) -> Result<usize, &'static str> {
        Ok(0)
    }
}

impl ControlOps for DrmDevice {
    fn control(&self, command: u32, arg: usize) -> Result<i32, &'static str> {
        use super::ioctl::commands::*;

        match command {
            DRM_IOCTL_VERSION => self.handle_version(arg),
            DRM_IOCTL_MODE_GETRESOURCES => self.handle_get_resources(arg),
            DRM_IOCTL_MODE_GETCRTC => self.handle_get_crtc(arg),
            DRM_IOCTL_MODE_SETCRTC => self.handle_set_crtc(arg),
            DRM_IOCTL_MODE_CREATE_DUMB => self.handle_create_dumb(arg),
            DRM_IOCTL_MODE_MAP_DUMB => self.handle_map_dumb(arg),
            DRM_IOCTL_MODE_DESTROY_DUMB => self.handle_destroy_dumb(arg),
            DRM_IOCTL_MODE_PAGE_FLIP => self.handle_page_flip(arg),
            _ => Err("Unsupported DRM ioctl"),
        }
    }

    fn supported_control_commands(&self) -> Vec<(u32, &'static str)> {
        use super::ioctl::commands::*;
        Vec::from([
            (DRM_IOCTL_VERSION, "Get DRM version"),
            (DRM_IOCTL_MODE_GETRESOURCES, "Get mode resources"),
            (DRM_IOCTL_MODE_GETCRTC, "Get CRTC"),
            (DRM_IOCTL_MODE_SETCRTC, "Set CRTC"),
            (DRM_IOCTL_MODE_CREATE_DUMB, "Create dumb buffer"),
            (DRM_IOCTL_MODE_MAP_DUMB, "Map dumb buffer"),
            (DRM_IOCTL_MODE_DESTROY_DUMB, "Destroy dumb buffer"),
            (DRM_IOCTL_MODE_PAGE_FLIP, "Page flip"),
        ])
    }
}

impl MemoryMappingOps for DrmDevice {
    fn get_mapping_info(
        &self,
        _offset: usize,
        _length: usize,
    ) -> Result<(usize, usize, bool), &'static str> {
        Err("Memory mapping not supported on DRM device")
    }

    fn on_mapped(&self, _vaddr: usize, _paddr: usize, _length: usize, _offset: usize) {}

    fn on_unmapped(&self, _vaddr: usize, _length: usize) {}

    fn supports_mmap(&self) -> bool {
        false
    }
}

impl Selectable for DrmDevice {
    fn wait_until_ready(
        &self,
        _interest: crate::object::capability::selectable::ReadyInterest,
        _trapframe: &mut crate::arch::Trapframe,
        _timeout_ticks: Option<u64>,
    ) -> crate::object::capability::selectable::SelectWaitOutcome {
        crate::object::capability::selectable::SelectWaitOutcome::Ready
    }
}

/// Register DRM devices for all available framebuffers
pub fn register_drm_devices() {
    let graphics_manager = GraphicsManager::get_manager();
    let device_manager = DeviceManager::get_manager();

    for fb_name in graphics_manager.get_framebuffer_names() {
        if let Some(drm_device) = DrmDevice::new(fb_name.clone()) {
            let device_id = device_manager
                .register_device_with_name(format!("drm-{}", fb_name), Arc::new(drm_device));
            crate::early_println!(
                "[DRM] Registered DRM device: /dev/drm-{} (ID: {})",
                fb_name,
                device_id
            );
        }
    }
}
