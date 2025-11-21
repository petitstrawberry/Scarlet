//! # DRM Character Device Module
//!
//! This module provides character device interface for DRM access.
//! It integrates with the GraphicsManager to provide user-space access to
//! DRM resources through the standard character device interface.

extern crate alloc;

use core::any::Any;
use alloc::{sync::Arc, vec::Vec, collections::BTreeMap};
use spin::RwLock;

use crate::device::{
    char::CharDevice, Device, DeviceType
};
use crate::object::capability::{ControlOps, MemoryMappingOps};
use crate::object::capability::selectable::Selectable;
use crate::abi::linux::drm::{ioctls, DrmFile};
use crate::task::mytask;

/// DRM character device implementation
pub struct DrmCharDevice {
    /// The underlying graphics device ID
    device_id: usize,
    /// Per-task DRM file context
    /// Key: Task ID
    /// Value: DrmFile context
    clients: RwLock<BTreeMap<usize, Arc<DrmFile>>>,
}

impl DrmCharDevice {
    /// Create a new DRM character device
    pub fn new(device_id: usize) -> Self {
        Self {
            device_id,
            clients: RwLock::new(BTreeMap::new()),
        }
    }

    /// Get or create DrmFile for current task
    fn get_current_client(&self) -> Result<Arc<DrmFile>, &'static str> {
        let task = mytask().ok_or("No current task")?;
        let task_id = task.get_id();
        
        // Fast path: check if exists
        {
            let clients = self.clients.read();
            if let Some(client) = clients.get(&task_id) {
                return Ok(client.clone());
            }
        }
        
        // Slow path: create new
        let mut clients = self.clients.write();
        // Check again in case of race
        if let Some(client) = clients.get(&task_id) {
            return Ok(client.clone());
        }
        
        let client = Arc::new(DrmFile::new(self.device_id));
        clients.insert(task_id, client.clone());
        Ok(client)
    }
}

impl Device for DrmCharDevice {
    fn device_type(&self) -> DeviceType {
        DeviceType::Char
    }

    fn name(&self) -> &'static str {
        "drm"
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

impl CharDevice for DrmCharDevice {
    fn read_byte(&self) -> Option<u8> {
        None
    }

    fn write_byte(&self, _byte: u8) -> Result<(), &'static str> {
        Err("write_byte is not supported")
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

impl ControlOps for DrmCharDevice {
    fn control(&self, command: u32, arg: usize) -> Result<i32, &'static str> {
        let client = self.get_current_client()?;
        
        match command {
            ioctls::commands::DRM_IOCTL_VERSION => {
                ioctls::handle_drm_version(arg)
            }
            ioctls::commands::DRM_IOCTL_MODE_GETRESOURCES => {
                ioctls::handle_drm_get_resources(arg)
            }
            ioctls::commands::DRM_IOCTL_MODE_GETCRTC => {
                ioctls::handle_drm_get_crtc(arg, self.device_id)
            }
            ioctls::commands::DRM_IOCTL_MODE_CREATE_DUMB => {
                ioctls::handle_drm_create_dumb(arg, &client)
            }
            ioctls::commands::DRM_IOCTL_MODE_MAP_DUMB => {
                ioctls::handle_drm_map_dumb(arg, &client)
            }
            ioctls::commands::DRM_IOCTL_MODE_DESTROY_DUMB => {
                ioctls::handle_drm_destroy_dumb(arg, &client)
            }
            ioctls::commands::DRM_IOCTL_MODE_PAGE_FLIP => {
                ioctls::handle_drm_page_flip(arg, &client)
            }
            _ => {
                Err("Unsupported DRM ioctl")
            }
        }
    }
    
    fn supported_control_commands(&self) -> Vec<(u32, &'static str)> {
        use ioctls::commands::*;
        alloc::vec![
            (DRM_IOCTL_VERSION, "Get driver version"),
            (DRM_IOCTL_MODE_GETRESOURCES, "Get mode resources"),
            (DRM_IOCTL_MODE_GETCRTC, "Get CRTC configuration"),
            (DRM_IOCTL_MODE_CREATE_DUMB, "Create dumb buffer"),
            (DRM_IOCTL_MODE_MAP_DUMB, "Map dumb buffer"),
            (DRM_IOCTL_MODE_DESTROY_DUMB, "Destroy dumb buffer"),
            (DRM_IOCTL_MODE_PAGE_FLIP, "Perform page flip"),
        ]
    }
}

impl MemoryMappingOps for DrmCharDevice {
    fn get_mapping_info(&self, offset: usize, _length: usize) 
                       -> Result<(usize, usize, bool), &'static str> {
        // In our simplified implementation, the offset returned by MAP_DUMB
        // IS the physical address.
        // So we just return it as is.
        // In a real DRM driver, we would look up the GEM object by the "fake offset"
        // and get its physical pages.
        
        // Basic validation
        if offset == 0 {
            return Err("Invalid offset");
        }
        
        let paddr = offset;
        let permissions = 0x3; // Read and Write
        let is_shared = true; 
        
        Ok((paddr, permissions, is_shared))
    }
    
    fn on_mapped(&self, _vaddr: usize, _paddr: usize, _length: usize, _offset: usize) {
        // No tracking needed for now
    }
    
    fn on_unmapped(&self, _vaddr: usize, _length: usize) {
        // No tracking needed for now
    }
    
    fn supports_mmap(&self) -> bool {
        true
    }
}

impl Selectable for DrmCharDevice {}
