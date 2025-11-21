//! DRM File Object
//! 
//! This module implements the DrmFile structure which represents an open file description
//! for a DRM device. It handles ioctls and manages resources associated with the file
//! description, such as GEM handles.

use core::any::Any;
use alloc::sync::Arc;
use hashbrown::HashMap;
use spin::Mutex;

use crate::fs::{FileObject, SeekFrom, FileMetadata, FileType, FilePermission, DeviceFileInfo};
use crate::object::KernelObject;
use crate::object::capability::{ControlOps, MemoryMappingOps, Selectable};
use crate::object::capability::stream::{StreamOps, StreamError};
use crate::abi::linux::drm::ioctls;
use crate::device::DeviceType;

/// DRM File Object
/// 
/// Represents an open connection to the DRM subsystem.
/// Manages GEM handles which map integer IDs to KernelObjects (GraphicsBuffers).
pub struct DrmFile {
    /// Connection to the physical device (Session)
    /// For now, we assume a single global graphics manager or device 0.
    /// In a multi-device system, this would store the device ID.
    device_id: usize,
    
    /// Translation Table: Linux GEM Handle -> Scarlet Object Entity
    /// We store Arc<KernelObject> instead of Handle(usize) to ensure
    /// safety when DrmFile is shared across tasks (e.g., via fork/IPC).
    gem_handles: Mutex<HashMap<u32, Arc<KernelObject>>>,
    
    /// Next GEM handle ID to allocate
    next_gem_id: Mutex<u32>,
}

impl DrmFile {
    /// Create a new DrmFile instance
    pub fn new(device_id: usize) -> Self {
        Self {
            device_id,
            gem_handles: Mutex::new(HashMap::new()),
            next_gem_id: Mutex::new(1),
        }
    }
    
    /// Get the device ID associated with this file
    pub fn device_id(&self) -> usize {
        self.device_id
    }
    
    /// Allocate a new GEM handle for a kernel object
    pub fn add_gem_handle(&self, object: Arc<KernelObject>) -> Result<u32, &'static str> {
        let mut handles = self.gem_handles.lock();
        let mut next_id = self.next_gem_id.lock();
        
        let id = *next_id;
        if id == u32::MAX {
            return Err("GEM handle space exhausted");
        }
        *next_id += 1;
        
        handles.insert(id, object);
        Ok(id)
    }
    
    /// Get a kernel object by GEM handle
    pub fn get_gem_object(&self, handle: u32) -> Option<Arc<KernelObject>> {
        let handles = self.gem_handles.lock();
        handles.get(&handle).cloned()
    }
    
    /// Remove a GEM handle
    pub fn remove_gem_handle(&self, handle: u32) -> Option<Arc<KernelObject>> {
        let mut handles = self.gem_handles.lock();
        handles.remove(&handle)
    }
}

impl StreamOps for DrmFile {
    fn read(&self, _buffer: &mut [u8]) -> Result<usize, StreamError> {
        Err(StreamError::NotSupported)
    }
    
    fn write(&self, _buffer: &[u8]) -> Result<usize, StreamError> {
        Err(StreamError::NotSupported)
    }
}

impl FileObject for DrmFile {
    fn seek(&self, _whence: SeekFrom) -> Result<u64, StreamError> {
        Err(StreamError::NotSupported)
    }
    
    fn metadata(&self) -> Result<FileMetadata, StreamError> {
        Ok(FileMetadata {
            file_type: FileType::CharDevice(DeviceFileInfo {
                device_id: self.device_id,
                device_type: DeviceType::Char,
            }),
            size: 0,
            permissions: FilePermission {
                read: true,
                write: true,
                execute: false,
            },
            created_time: 0,
            modified_time: 0,
            accessed_time: 0,
            file_id: 0, // Should be unique
            link_count: 1,
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl ControlOps for DrmFile {
    fn control(&self, command: u32, arg: usize) -> Result<i32, &'static str> {
        // Dispatch ioctls to handlers in ioctls.rs
        match command {
            ioctls::commands::DRM_IOCTL_VERSION => {
                ioctls::handle_drm_version(arg)
            },
            ioctls::commands::DRM_IOCTL_MODE_GETRESOURCES => {
                ioctls::handle_drm_get_resources(arg)
            },
            ioctls::commands::DRM_IOCTL_MODE_GETCRTC => {
                ioctls::handle_drm_get_crtc(arg, self.device_id)
            },
            ioctls::commands::DRM_IOCTL_MODE_CREATE_DUMB => {
                ioctls::handle_drm_create_dumb(arg, self)
            },
            ioctls::commands::DRM_IOCTL_MODE_MAP_DUMB => {
                ioctls::handle_drm_map_dumb(arg, self)
            },
            ioctls::commands::DRM_IOCTL_MODE_DESTROY_DUMB => {
                ioctls::handle_drm_destroy_dumb(arg, self)
            },
            ioctls::commands::DRM_IOCTL_MODE_PAGE_FLIP => {
                ioctls::handle_drm_page_flip(arg, self)
            },
            _ => Err("Unknown DRM ioctl"),
        }
    }
}

impl MemoryMappingOps for DrmFile {
    fn get_mapping_info(&self, _offset: usize, _length: usize) -> Result<(usize, usize, bool), &'static str> {
        // DRM mmap is typically handled by looking up the offset (which is a fake offset)
        // and mapping the underlying buffer.
        // For MVP, we might rely on the fact that map_dumb returns a physical address
        // and the user mmap call might be bypassing this FileObject if it uses /dev/mem or similar.
        
        Err("DRM mmap not fully implemented in DrmFile yet")
    }

    fn on_mapped(&self, _vaddr: usize, _paddr: usize, _length: usize, _offset: usize) {}
    fn on_unmapped(&self, _vaddr: usize, _length: usize) {}
    fn supports_mmap(&self) -> bool {
        true
    }
}

impl Selectable for DrmFile {}
