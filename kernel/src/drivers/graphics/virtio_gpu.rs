//! # VirtIO GPU Device Driver
//! 
//! This module provides a driver for VirtIO GPU devices, implementing the
//! GraphicsDevice trait for integration with the kernel's graphics subsystem.
//!
//! The driver supports basic framebuffer operations and display management
//! according to the VirtIO GPU specification.


use alloc::{boxed::Box, sync::Arc};
use spin::{Mutex, RwLock};

use crate::{
    device::{graphics::{FramebufferConfig, GraphicsDevice, PixelFormat}, Device, DeviceType},
    drivers::virtio::{device::VirtioDevice, queue::{DescriptorFlag, VirtQueue}},
    mem::page::{allocate_raw_pages, Page}, object::capability::{ControlOps, MemoryMappingOps}, timer::{add_timer, get_tick, ms_to_ticks, SoftwareTimer, TimerHandler},
};
use core::{ptr, sync::atomic::fence};

// VirtIO GPU Constants
const VIRTIO_GPU_F_VIRGL: u32 = 0;
const VIRTIO_GPU_F_EDID: u32 = 1;

// VirtIO GPU Control Commands
const VIRTIO_GPU_CMD_GET_DISPLAY_INFO: u32 = 0x0100;
const VIRTIO_GPU_CMD_RESOURCE_CREATE_2D: u32 = 0x0101;
const VIRTIO_GPU_CMD_RESOURCE_UNREF: u32 = 0x0102;
const VIRTIO_GPU_CMD_SET_SCANOUT: u32 = 0x0103;
const VIRTIO_GPU_CMD_RESOURCE_FLUSH: u32 = 0x0104;
const VIRTIO_GPU_CMD_TRANSFER_TO_HOST_2D: u32 = 0x0105;
const VIRTIO_GPU_CMD_RESOURCE_ATTACH_BACKING: u32 = 0x0106;
const VIRTIO_GPU_CMD_RESOURCE_DETACH_BACKING: u32 = 0x0107;

// VirtIO GPU Response Types
const VIRTIO_GPU_RESP_OK_NODATA: u32 = 0x1100;
const VIRTIO_GPU_RESP_OK_DISPLAY_INFO: u32 = 0x1101;

// VirtIO GPU Formats
const VIRTIO_GPU_FORMAT_B8G8R8A8_UNORM: u32 = 1;
const VIRTIO_GPU_FORMAT_B8G8R8X8_UNORM: u32 = 2;
const VIRTIO_GPU_FORMAT_A8R8G8B8_UNORM: u32 = 3;
const VIRTIO_GPU_FORMAT_X8R8G8B8_UNORM: u32 = 4;
const VIRTIO_GPU_FORMAT_R8G8B8A8_UNORM: u32 = 67;
const VIRTIO_GPU_FORMAT_X8B8G8R8_UNORM: u32 = 68;
const VIRTIO_GPU_FORMAT_A8B8G8R8_UNORM: u32 = 121;
const VIRTIO_GPU_FORMAT_R8G8B8X8_UNORM: u32 = 134;

// Maximum number of scanouts
const VIRTIO_GPU_MAX_SCANOUTS: usize = 16;

/// VirtIO GPU command header
#[repr(C)]
struct VirtioGpuCtrlHdr {
    hdr_type: u32,
    flags: u32,
    fence_id: u64,
    ctx_id: u32,
    padding: u32,
}

/// VirtIO GPU rectangle
#[repr(C)]
#[derive(Clone, Copy)]
struct VirtioGpuRect {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

/// VirtIO GPU display info
#[repr(C)]
struct VirtioGpuRespDisplayInfo {
    hdr: VirtioGpuCtrlHdr,
    pmodes: [VirtioGpuDisplayOne; VIRTIO_GPU_MAX_SCANOUTS],
}

/// VirtIO GPU display mode
#[repr(C)]
#[derive(Clone, Copy)]
struct VirtioGpuDisplayOne {
    r: VirtioGpuRect,
    enabled: u32,
    flags: u32,
}

/// VirtIO GPU resource create 2D
#[repr(C)]
struct VirtioGpuResourceCreate2d {
    hdr: VirtioGpuCtrlHdr,
    resource_id: u32,
    format: u32,
    width: u32,
    height: u32,
}

/// VirtIO GPU set scanout
#[repr(C)]
struct VirtioGpuSetScanout {
    hdr: VirtioGpuCtrlHdr,
    r: VirtioGpuRect,
    scanout_id: u32,
    resource_id: u32,
}

/// VirtIO GPU resource flush
#[repr(C)]
struct VirtioGpuResourceFlush {
    hdr: VirtioGpuCtrlHdr,
    r: VirtioGpuRect,
    resource_id: u32,
    padding: u32,
}

/// VirtIO GPU transfer to host 2D
#[repr(C)]
struct VirtioGpuTransferToHost2d {
    hdr: VirtioGpuCtrlHdr,
    r: VirtioGpuRect,
    offset: u64,
    resource_id: u32,
    padding: u32,
}

/// VirtIO GPU resource attach backing
#[repr(C)]
struct VirtioGpuResourceAttachBacking {
    hdr: VirtioGpuCtrlHdr,
    resource_id: u32,
    nr_entries: u32,
}

/// VirtIO GPU memory entry
#[repr(C)]
struct VirtioGpuMemEntry {
    addr: u64,
    length: u32,
    padding: u32,
}

/// VirtIO GPU Device Core
pub struct VirtioGpuDeviceCore {
    base_addr: usize,
    virtqueues: Mutex<[VirtQueue<'static>; 2]>, // Control queue (0) and Cursor queue (1)
    display_info: RwLock<Option<VirtioGpuRespDisplayInfo>>,
    framebuffer_addr: RwLock<Option<usize>>,
    shadow_framebuffer_addr: RwLock<Option<usize>>,
    boxed_framebuffer: RwLock<Option<Box<[Page]>>>, // Boxed framebuffer for easier management
    boxed_shadow_framebuffer: RwLock<Option<Box<[Page]>>>, // Boxed shadow framebuffer
    resource_id: Mutex<u32>,
    initialized: Mutex<bool>,
    // Track resources and their associated memory
    resources: Mutex<alloc::collections::BTreeMap<u32, (usize, usize)>>, // resource_id -> (addr, size)
}

impl VirtioGpuDeviceCore {
    /// Create a new VirtIO GPU device
    ///
    /// # Arguments
    ///
    /// * `base_addr` - The base address of the device
    ///
    /// # Returns
    ///
    /// A new instance of `VirtioGpuDevice`
    pub fn new(base_addr: usize) -> Self {
        let mut device = Self {
            base_addr,
            virtqueues: Mutex::new([VirtQueue::new(64), VirtQueue::new(64)]), // Control and Cursor queues with 64 descriptors each
            display_info: RwLock::new(None),
            framebuffer_addr: RwLock::new(None),
            shadow_framebuffer_addr: RwLock::new(None),
            boxed_framebuffer: RwLock::new(None),
            boxed_shadow_framebuffer: RwLock::new(None), 
            resource_id: Mutex::new(1),
            initialized: Mutex::new(false),
            resources: Mutex::new(alloc::collections::BTreeMap::new()),
        };
        
        // Initialize virtqueues first
        {
            let mut virtqueues = device.virtqueues.lock();
            for queue in virtqueues.iter_mut() {
                queue.init();
            }
        }
        
        // Initialize the VirtIO device - this will set up the queues with the device
        if device.init().is_err() {
            crate::early_println!("[Virtio GPU] Warning: Failed to initialize VirtIO device");
        }
        
        // crate::early_println!("[Virtio GPU] Device created and initialized at {:#x}", base_addr);
        device
    }

    /// Get next resource ID
    fn next_resource_id(&self) -> u32 {
        let mut id = self.resource_id.lock();
        let current = *id;
        *id += 1;
        current
    }

    /// Send a command to the control queue
    fn send_control_command<T>(&self, cmd: &T) -> Result<(), &'static str> {
        let mut virtqueues = self.virtqueues.lock();
        let control_queue = &mut virtqueues[0]; // Control queue is index 0

        // The response buffer is allocated on the stack. It's faster and
        // its memory is automatically reclaimed when the function returns.
        let mut resp_buffer = [0u8; 64];

        // Allocate descriptors
        let cmd_desc = control_queue.alloc_desc().ok_or("Failed to allocate command descriptor")?;
        let resp_desc = match control_queue.alloc_desc() {
            Some(desc) => desc,
            None => {
                // Free the already allocated cmd_desc before returning error
                control_queue.free_desc(cmd_desc);
                return Err("Failed to allocate response descriptor");
            }
        };

        // Set up command descriptor (device readable)
        let cmd_desc_ptr = &mut control_queue.desc[cmd_desc] as *mut crate::drivers::virtio::queue::Descriptor;
        unsafe {
            core::ptr::write_volatile(&mut (*cmd_desc_ptr).addr, (cmd as *const T) as u64);
            core::ptr::write_volatile(&mut (*cmd_desc_ptr).len, core::mem::size_of::<T>() as u32);
            core::ptr::write_volatile(&mut (*cmd_desc_ptr).flags, DescriptorFlag::Next as u16);
            core::ptr::write_volatile(&mut (*cmd_desc_ptr).next, resp_desc as u16);
        }

        // Set up response descriptor (device writable)
        let resp_desc_ptr = &mut control_queue.desc[resp_desc] as *mut crate::drivers::virtio::queue::Descriptor;
        unsafe {
            core::ptr::write_volatile(&mut (*resp_desc_ptr).addr, resp_buffer.as_mut_ptr() as u64);
            core::ptr::write_volatile(&mut (*resp_desc_ptr).len, resp_buffer.len() as u32); // Use .len() for safety
            core::ptr::write_volatile(&mut (*resp_desc_ptr).flags, DescriptorFlag::Write as u16);
        }

        // crate::early_println!("[Virtio GPU] Sending command to control queue: type={}",
        //     unsafe { *(cmd as *const T as *const u32) });

        // Submit the request to the queue
        if let Err(e) = control_queue.push(cmd_desc) {
            // Free descriptors if push fails
            control_queue.free_desc(resp_desc);
            control_queue.free_desc(cmd_desc);
            return Err(e);
        }

        // Notify the device
        self.notify(0); // Notify control queue

        // Wait for response (simplified polling)
        // crate::early_println!("[Virtio GPU] Waiting for command response...");
        while control_queue.is_busy() {}
        while *control_queue.used.idx == control_queue.last_used_idx {}

        // Process response
        let _resp_idx = match control_queue.pop() {
            Some(idx) => idx,
            None => {
                // Free descriptors even if pop fails (device may have processed them)
                control_queue.free_desc(resp_desc);
                control_queue.free_desc(cmd_desc);
                return Err("No response from device");
            }
        };

        // Free descriptors (responsibility of driver, not VirtQueue)
        control_queue.free_desc(resp_desc);
        control_queue.free_desc(cmd_desc);

        // crate::early_println!("[Virtio GPU] Command completed successfully");

        Ok(())
    }

    /// Get display information from the device
    fn get_display_info_internal(&mut self) -> Result<(), &'static str> {
        // Create get display info command
        let cmd = VirtioGpuCtrlHdr {
            hdr_type: VIRTIO_GPU_CMD_GET_DISPLAY_INFO,
            flags: 0,
            fence_id: 0,
            ctx_id: 0,
            padding: 0,
        };

        // Send command
        self.send_control_command(&cmd)?;

        // For now, create a default display configuration
        let mut display_info = VirtioGpuRespDisplayInfo {
            hdr: VirtioGpuCtrlHdr {
                hdr_type: VIRTIO_GPU_RESP_OK_DISPLAY_INFO,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
            pmodes: [VirtioGpuDisplayOne {
                r: VirtioGpuRect {
                    x: 0,
                    y: 0,
                    width: 1024,
                    height: 768,
                },
                enabled: 1,
                flags: 0,
            }; VIRTIO_GPU_MAX_SCANOUTS],
        };

        // Only enable the first display
        for i in 1..VIRTIO_GPU_MAX_SCANOUTS {
            display_info.pmodes[i].enabled = 0;
        }

        *self.display_info.write() = Some(display_info);
        Ok(())
    }

    /// Create a 2D resource
    fn create_2d_resource(&self, width: u32, height: u32, format: u32) -> Result<u32, &'static str> {
        let resource_id = self.next_resource_id();
        
        let cmd = VirtioGpuResourceCreate2d {
            hdr: VirtioGpuCtrlHdr {
                hdr_type: VIRTIO_GPU_CMD_RESOURCE_CREATE_2D,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
            resource_id,
            format,
            width,
            height,
        };

        self.send_control_command(&cmd)?;
        Ok(resource_id)
    }

    /// Attach backing memory to a resource
    fn attach_backing_to_resource(&self, resource_id: u32, addr: usize, size: usize) -> Result<(), &'static str> {
        // Create attach backing command + memory entry in a single buffer
        #[repr(C)]
        struct AttachBackingWithEntry {
            attach: VirtioGpuResourceAttachBacking,
            entry: VirtioGpuMemEntry,
        }

        let cmd = AttachBackingWithEntry {
            attach: VirtioGpuResourceAttachBacking {
                hdr: VirtioGpuCtrlHdr {
                    hdr_type: VIRTIO_GPU_CMD_RESOURCE_ATTACH_BACKING,
                    flags: 0,
                    fence_id: 0,
                    ctx_id: 0,
                    padding: 0,
                },
                resource_id,
                nr_entries: 1,
            },
            entry: VirtioGpuMemEntry {
                addr: addr as u64,
                length: size as u32,
                padding: 0,
            },
        };

        // crate::early_println!("[Virtio GPU] Attaching framebuffer memory {:#x} (size {}) to resource {}", 
        //     addr, size, resource_id);
        self.send_control_command(&cmd)?;
        Ok(())
    }

    /// Set up framebuffer
    fn setup_framebuffer(&self) -> Result<(), &'static str> {
        let display_info = self.display_info.read();
        let display_info = display_info.as_ref().ok_or("No display info available")?;
        let primary_display = &display_info.pmodes[0];
        if primary_display.enabled == 0 {
            return Err("Primary display not enabled");
        }
        let width = primary_display.r.width;
        let height = primary_display.r.height;
        let resource_id = self.create_2d_resource(width, height, VIRTIO_GPU_FORMAT_B8G8R8X8_UNORM)?;
        let fb_size = (width * height * 4) as usize;
        let fb_pages = (fb_size + 4095) / 4096;
        let fb_pages_ptr = allocate_raw_pages(fb_pages);
        if fb_pages_ptr.is_null() {
            return Err("Failed to allocate framebuffer memory");
        }
        let fb_addr = fb_pages_ptr as usize;
        // Store the framebuffer in boxed memory for easier management
        self.boxed_framebuffer.write().replace(unsafe { Box::from_raw(core::ptr::slice_from_raw_parts_mut(fb_pages_ptr, fb_pages)) });
        self.attach_backing_to_resource(resource_id, fb_addr, fb_size)?; // Attach backing memory to the resource
        // Set scanout to use this framebuffer
        let scanout_cmd = VirtioGpuSetScanout {
            hdr: VirtioGpuCtrlHdr {
                hdr_type: VIRTIO_GPU_CMD_SET_SCANOUT,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
            r: VirtioGpuRect { x: 0, y: 0, width, height },
            scanout_id: 0,
            resource_id,
        };
        self.send_control_command(&scanout_cmd)?;
        {
            let mut resources = self.resources.lock();
            resources.insert(resource_id, (fb_addr, fb_size));
        }
        *self.framebuffer_addr.write() = Some(fb_addr);
        // Allocate shadow framebuffer
        let shadow_pages_ptr = allocate_raw_pages(fb_pages);
        if shadow_pages_ptr.is_null() {
            return Err("Failed to allocate shadow framebuffer memory");
        }
        let shadow_addr = shadow_pages_ptr as usize;
        // Store the shadow framebuffer in boxed memory for easier management
        self.boxed_shadow_framebuffer.write().replace(unsafe { Box::from_raw(core::ptr::slice_from_raw_parts_mut(shadow_pages_ptr, fb_pages)) });
        // Initialize shadow framebuffer with the contents of the framebuffer
        let fb_size = fb_size as usize;
        unsafe {
            ptr::copy_nonoverlapping(fb_addr as *const u8, shadow_addr as *mut u8, fb_size);
        }
        *self.shadow_framebuffer_addr.write() = Some(shadow_addr);
        Ok(())
    }

    fn get_display_name(&self) -> &'static str {
        "virtio-gpu"
    }

    fn get_framebuffer_config(&self) -> Result<FramebufferConfig, &'static str> {
        let display_info = self.display_info.read();
        let display_info = display_info.as_ref().ok_or("Device not initialized")?;
        
        let primary_display = &display_info.pmodes[0];
        if primary_display.enabled == 0 {
            return Err("Primary display not enabled");
        }

        Ok(FramebufferConfig::new(
            primary_display.r.width,
            primary_display.r.height,
            PixelFormat::BGRA8888, // VirtIO GPU typically uses BGRA format
        ))
    }

    fn get_framebuffer_address(&self) -> Result<usize, &'static str> {
        self.framebuffer_addr.read()
            .ok_or("Framebuffer not initialized")
    }

    fn flush_framebuffer(&self, x: u32, y: u32, width: u32, height: u32) -> Result<(), &'static str> {
        let display_info = self.display_info.read();
        let _display_info = display_info.as_ref().ok_or("Device not initialized")?;
        
        // Get the resource ID from our tracked resources
        let resource_id = {
            let resources = self.resources.lock();
            if let Some((_, _)) = resources.get(&1) {
                1 // Use primary framebuffer resource
            } else {
                return Err("No framebuffer resource found");
            }
        };

        // crate::early_println!("[Virtio GPU] Flushing framebuffer region: ({},{}) {}x{} for resource {}", 
        //     x, y, width, height, resource_id);

        // Transfer to host - copies data from guest memory to host
        // This is necessary because the host GPU driver needs to know
        // that the framebuffer contents have changed
        let transfer_cmd = VirtioGpuTransferToHost2d {
            hdr: VirtioGpuCtrlHdr {
                hdr_type: VIRTIO_GPU_CMD_TRANSFER_TO_HOST_2D,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
            r: VirtioGpuRect { x, y, width, height },
            offset: 0,
            resource_id,
            padding: 0,
        };

        self.send_control_command(&transfer_cmd)?;

        // Flush resource - tells the display to update the specified region
        // This actually triggers the display update
        let flush_cmd = VirtioGpuResourceFlush {
            hdr: VirtioGpuCtrlHdr {
                hdr_type: VIRTIO_GPU_CMD_RESOURCE_FLUSH,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
            r: VirtioGpuRect { x, y, width, height },
            resource_id,
            padding: 0,
        };

        self.send_control_command(&flush_cmd)?;
        // crate::early_println!("[Virtio GPU] Framebuffer flush completed");
        Ok(())
    }
}

impl VirtioDevice for VirtioGpuDeviceCore {
    fn get_base_addr(&self) -> usize {
        self.base_addr
    }

    fn get_virtqueue_count(&self) -> usize {
        2 // Control queue and cursor queue
    }

    fn get_virtqueue_size(&self, queue_idx: usize) -> usize {
        if queue_idx >= self.get_virtqueue_count() {
            panic!("Invalid queue index: {}", queue_idx);
        }
        
        let virtqueues = self.virtqueues.lock();
        virtqueues[queue_idx].get_queue_size()
    }

    fn get_queue_desc_addr(&self, queue_idx: usize) -> Option<u64> {
        if queue_idx >= self.get_virtqueue_count() {
            return None;
        }
        
        let virtqueues = self.virtqueues.lock();
        Some(virtqueues[queue_idx].desc.as_ptr() as u64)
    }

    fn get_queue_driver_addr(&self, queue_idx: usize) -> Option<u64> {
        if queue_idx >= self.get_virtqueue_count() {
            return None;
        }
        
        let virtqueues = self.virtqueues.lock();
        Some(virtqueues[queue_idx].avail.flags as *const u16 as u64)
    }

    fn get_queue_device_addr(&self, queue_idx: usize) -> Option<u64> {
        if queue_idx >= self.get_virtqueue_count() {
            return None;
        }
        
        let virtqueues = self.virtqueues.lock();
        Some(virtqueues[queue_idx].used.flags as *const u16 as u64)
    }

    fn get_supported_features(&self, _device_features: u32) -> u32 {
        // For now, don't enable any advanced features
        0
    }
}

pub struct VirtioGpuDevice {
    core: Arc<Mutex<VirtioGpuDeviceCore>>,
    handler: Option<Arc<dyn TimerHandler>>,
}

impl VirtioGpuDevice {
    /// Create a new VirtIO GPU device
    ///
    /// # Arguments
    ///
    /// * `base_addr` - The base address of the device
    ///
    /// # Returns
    ///
    /// A new instance of `VirtioGpuDevice`
    pub fn new(base_addr: usize) -> Self {
        Self {
            core: Arc::new(Mutex::new(VirtioGpuDeviceCore::new(base_addr))),
            handler: None,
        }
    }
}

impl Device for VirtioGpuDevice {
    fn device_type(&self) -> DeviceType {
        DeviceType::Graphics
    }

    fn name(&self) -> &'static str {
        "virtio-gpu"
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
        self
    }

    fn as_graphics_device(&self) -> Option<&dyn GraphicsDevice> {
        Some(self)
    }
}

impl ControlOps for VirtioGpuDevice {
    // VirtIO GPU devices don't support control operations by default
    fn control(&self, _command: u32, _arg: usize) -> Result<i32, &'static str> {
        Err("Control operations not supported")
    }
}

impl MemoryMappingOps for VirtioGpuDevice {
    fn get_mapping_info(&self, _offset: usize, _length: usize) 
                       -> Result<(usize, usize, bool), &'static str> {
        Err("Memory mapping not supported by VirtIO GPU device")
    }
    
    fn on_mapped(&self, _vaddr: usize, _paddr: usize, _length: usize, _offset: usize) {
        // VirtIO GPU devices don't support memory mapping
    }
    
    fn on_unmapped(&self, _vaddr: usize, _length: usize) {
        // VirtIO GPU devices don't support memory mapping
    }
    
    fn supports_mmap(&self) -> bool {
        false
    }
}

impl GraphicsDevice for VirtioGpuDevice {
    fn get_display_name(&self) -> &'static str {
        "virtio-gpu"
    }

    fn get_framebuffer_config(&self) -> Result<FramebufferConfig, &'static str> {
        self.core.lock().get_framebuffer_config()
    }

    fn get_framebuffer_address(&self) -> Result<usize, &'static str> {
        self.core.lock().get_framebuffer_address()
    }

    fn flush_framebuffer(&self, x: u32, y: u32, width: u32, height: u32) -> Result<(), &'static str> {
        self.core.lock().flush_framebuffer(x, y, width, height)
    }

    fn init_graphics(&mut self) -> Result<(), &'static str> {
        {
            let core = self.core.lock();
            let mut initialized = core.initialized.lock();
            if *initialized {
                return Ok(());
            }
            *initialized = true;
        }

        // crate::early_println!("[Virtio GPU] Initializing graphics subsystem for device at {:#x}", self.base_addr);

        // Get display information
        self.core.lock().get_display_info_internal()?;

        // Set up framebuffer
        self.core.lock().setup_framebuffer()?;

        let handler: Arc<dyn TimerHandler> = Arc::new(FramebufferUpdateHandler {
            device: self.core.clone(),
        });

        add_timer(get_tick() + ms_to_ticks(16), &handler, 0);

        self.handler = Some(handler);

        // crate::early_println!("[Virtio GPU] Graphics subsystem initialization completed");
        Ok(())
    }
}

struct FramebufferUpdateHandler {
    device: Arc<Mutex<VirtioGpuDeviceCore>>,
}

impl FramebufferUpdateHandler {
    fn compare_and_flush(&self) {
        let (fb_addr, shadow_addr, width, height, fb_size) = {
            let core = self.device.lock();
            let fb_addr = match *core.framebuffer_addr.read() {
                Some(addr) => addr,
                None => return,
            };
            let shadow_addr = match *core.shadow_framebuffer_addr.read() {
                Some(addr) => addr,
                None => return,
            };
            let display_info_guard = core.display_info.read();
            let display_info = match display_info_guard.as_ref() {
                Some(info) => info,
                None => return,
            };
            let width = display_info.pmodes[0].r.width;
            let height = display_info.pmodes[0].r.height;
            let fb_size = (width * height * 4) as usize;
            (fb_addr, shadow_addr, width, height, fb_size)
        };
        // Determine if the framebuffer has changed
        let fb_ptr = fb_addr as *const u8;
        let shadow_ptr = shadow_addr as *const u8;
        let fb_slice = unsafe { core::slice::from_raw_parts(fb_ptr, fb_size) };
        let shadow_slice = unsafe { core::slice::from_raw_parts(shadow_ptr, fb_size) };
        let changed = fb_slice != shadow_slice;
        
        if changed {
            let _ = self.device.lock().flush_framebuffer(0, 0, width, height);
            fence(core::sync::atomic::Ordering::SeqCst);
            unsafe {
                ptr::copy_nonoverlapping(fb_addr as *const u8, shadow_addr as *mut u8, fb_size);
            }
        }
    }
}

impl TimerHandler for FramebufferUpdateHandler {
    fn on_timer_expired(self: Arc<Self>, context: usize) {
        self.compare_and_flush();
        let handler = self as Arc<dyn TimerHandler>;
        add_timer(get_tick() + ms_to_ticks(16), &handler, context);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_virtio_gpu_device_creation() {
        let device = VirtioGpuDevice::new(0x10002000);
        assert_eq!(device.core.lock().get_base_addr(), 0x10002000);
        assert_eq!(device.core.lock().get_virtqueue_count(), 2);
        assert_eq!(device.device_type(), DeviceType::Graphics);
        assert_eq!(device.name(), "virtio-gpu");
        assert_eq!(device.core.lock().get_display_name(), "virtio-gpu");
    }

    #[test_case]
    fn test_virtio_gpu_resource_id_generation() {
        let device = VirtioGpuDevice::new(0x10002000);
        assert_eq!(device.core.lock().next_resource_id(), 1);
        assert_eq!(device.core.lock().next_resource_id(), 2);
        assert_eq!(device.core.lock().next_resource_id(), 3);
    }

    #[test_case]
    fn test_virtio_gpu_before_init() {
        let device = VirtioGpuDevice::new(0x10002000);
        // Should fail before initialization
        assert!(device.get_framebuffer_config().is_err());
        assert!(device.get_framebuffer_address().is_err());
    }

    #[test_case]
    fn test_virtio_gpu_init_graphics() {
        let mut device = VirtioGpuDevice::new(0x10002000);
        device.init_graphics().unwrap();
    }

    #[test_case]
    fn test_virtio_gpu_framebuffer_operations() {
        let mut device = VirtioGpuDevice::new(0x10002000);
        
        // Initialize the device
        device.init_graphics().unwrap();
        
        // Get framebuffer configuration
        let config = device.get_framebuffer_config().unwrap();
        assert_eq!(config.width, 1024);
        assert_eq!(config.height, 768);
        assert_eq!(config.format, PixelFormat::BGRA8888);
        
        // Get framebuffer address
        let fb_addr = device.get_framebuffer_address().unwrap();
        assert_ne!(fb_addr, 0);
        
        // Write some test pattern to framebuffer
        unsafe {
            let fb_ptr = fb_addr as *mut u32;
            let pixel_count = (config.width * config.height) as usize;
            
            // Fill with a gradient pattern
            for y in 0..config.height {
                for x in 0..config.width {
                    let pixel_index = (y * config.width + x) as usize;
                    if pixel_index < pixel_count {
                        // Create a simple gradient: red increasing with x, blue with y
                        let red = if config.width > 1 { (x * 255) / (config.width - 1) } else { 0 };
                        let blue = if config.height > 1 { (y * 255) / (config.height - 1) } else { 0 };
                        let green = 0x80; // Fixed green component
                        let alpha = 0xFF; // Fully opaque
                        
                        // BGRA format: Blue | Green | Red | Alpha
                        let pixel = (alpha << 24) | (red << 16) | (green << 8) | blue;
                        *fb_ptr.add(pixel_index) = pixel;
                    }
                }
            }
        }
        
        // Flush the entire framebuffer
        device.flush_framebuffer(0, 0, config.width, config.height).unwrap();
        
        // Verify some pixels were written correctly
        unsafe {
            let fb_ptr = fb_addr as *mut u32;
            
            // Check top-left corner (should be mostly blue)
            let top_left = *fb_ptr;
            assert_eq!((top_left >> 24) & 0xFF, 0xFF); // Alpha
            assert_eq!((top_left >> 16) & 0xFF, 0x00); // Red (should be 0 at x=0)
            assert_eq!((top_left >> 8) & 0xFF, 0x80);  // Green
            assert_eq!(top_left & 0xFF, 0x00);         // Blue (should be 0 at y=0)
            
            // Check bottom-right corner
            let bottom_right_index = ((config.height - 1) * config.width + (config.width - 1)) as usize;
            let bottom_right = *fb_ptr.add(bottom_right_index);
            assert_eq!((bottom_right >> 24) & 0xFF, 0xFF); // Alpha
            assert_eq!((bottom_right >> 16) & 0xFF, 0xFF); // Red (should be max at x=width-1)
            assert_eq!((bottom_right >> 8) & 0xFF, 0x80);  // Green
            assert_eq!(bottom_right & 0xFF, 0xFF);         // Blue (should be max at y=height-1)
        }
    }

    #[test_case]
    fn test_virtio_gpu_pixel_drawing() {
        let mut device = VirtioGpuDevice::new(0x10002000);
        device.init_graphics().unwrap();
        
        let config = device.get_framebuffer_config().unwrap();
        let fb_addr = device.get_framebuffer_address().unwrap();
        
        // Helper function to set a pixel
        let set_pixel = |x: u32, y: u32, color: u32| {
            if x < config.width && y < config.height {
                unsafe {
                    let fb_ptr = fb_addr as *mut u32;
                    let pixel_index = (y * config.width + x) as usize;
                    *fb_ptr.add(pixel_index) = color;
                }
            }
        };
        
        // Draw a simple test pattern
        // Red horizontal line at y=100
        for x in 0..config.width {
            set_pixel(x, 100, 0xFF0000FF); // Red in BGRA format
        }
        
        // Green vertical line at x=200
        for y in 0..config.height {
            set_pixel(200, y, 0xFF00FF00); // Green in BGRA format
        }
        
        // Blue diagonal line
        let min_dim = config.width.min(config.height);
        for i in 0..min_dim {
            set_pixel(i, i, 0xFFFF0000); // Blue in BGRA format
        }
        
        // Flush the changes
        device.flush_framebuffer(0, 0, config.width, config.height).unwrap();
        
        // Verify some of the drawn pixels
        unsafe {
            let fb_ptr = fb_addr as *mut u32;
            
            // Check red line
            let red_pixel_index = (100 * config.width + 50) as usize;
            let red_pixel = *fb_ptr.add(red_pixel_index);
            assert_eq!(red_pixel, 0xFF0000FF);
            
            // Check green line
            let green_pixel_index = (50 * config.width + 200) as usize;
            let green_pixel = *fb_ptr.add(green_pixel_index);
            assert_eq!(green_pixel, 0xFF00FF00);
            
            // Check blue diagonal
            let blue_pixel_index = (100 * config.width + 100) as usize;
            let blue_pixel = *fb_ptr.add(blue_pixel_index);
            assert_eq!(blue_pixel, 0xFFFF0000);
        }
    }

    #[test_case]
    fn test_virtio_gpu_rectangle_drawing() {
        let mut device = VirtioGpuDevice::new(0x10002000);
        device.init_graphics().unwrap();
        
        let config = device.get_framebuffer_config().unwrap();
        let fb_addr = device.get_framebuffer_address().unwrap();
        
        // Helper function to draw a filled rectangle
        let draw_rectangle = |x: u32, y: u32, width: u32, height: u32, color: u32| {
            for dy in 0..height {
                for dx in 0..width {
                    let px = x + dx;
                    let py = y + dy;
                    if px < config.width && py < config.height {
                        unsafe {
                            let fb_ptr = fb_addr as *mut u32;
                            let pixel_index = (py * config.width + px) as usize;
                            *fb_ptr.add(pixel_index) = color;
                        }
                    }
                }
            }
        };
        
        // Clear framebuffer with black
        unsafe {
            let fb_ptr = fb_addr as *mut u32;
            let pixel_count = (config.width * config.height) as usize;
            for i in 0..pixel_count {
                *fb_ptr.add(i) = 0xFF000000; // Black with full alpha
            }
        }
        
        // Draw some rectangles
        draw_rectangle(50, 50, 100, 75, 0xFF0000FF);   // Red rectangle
        draw_rectangle(200, 100, 150, 100, 0xFF00FF00); // Green rectangle
        draw_rectangle(400, 200, 80, 120, 0xFFFF0000);  // Blue rectangle
        
        // Flush changes
        device.flush_framebuffer(0, 0, config.width, config.height).unwrap();
        
        // Verify the rectangles were drawn correctly
        unsafe {
            let fb_ptr = fb_addr as *mut u32;
            
            // Check red rectangle center
            let red_center_index = ((50 + 37) * config.width + (50 + 50)) as usize;
            let red_pixel = *fb_ptr.add(red_center_index);
            assert_eq!(red_pixel, 0xFF0000FF);
            
            // Check green rectangle center
            let green_center_index = ((100 + 50) * config.width + (200 + 75)) as usize;
            let green_pixel = *fb_ptr.add(green_center_index);
            assert_eq!(green_pixel, 0xFF00FF00);
            
            // Check blue rectangle center
            let blue_center_index = ((200 + 60) * config.width + (400 + 40)) as usize;
            let blue_pixel = *fb_ptr.add(blue_center_index);
            assert_eq!(blue_pixel, 0xFFFF0000);
            
            // Check that area outside rectangles is still black
            let background_index = (10 * config.width + 10) as usize;
            let background_pixel = *fb_ptr.add(background_index);
            assert_eq!(background_pixel, 0xFF000000);
        }
    }

    #[test_case]
    fn test_virtio_gpu_border_drawing() {
        let mut device = VirtioGpuDevice::new(0x10002000);
        device.init_graphics().unwrap();
        
        let config = device.get_framebuffer_config().unwrap();
        let fb_addr = device.get_framebuffer_address().unwrap();
        
        // Helper function to draw a rectangle border
        let draw_border = |x: u32, y: u32, width: u32, height: u32, color: u32| {
            // Top and bottom edges
            for dx in 0..width {
                let px = x + dx;
                if px < config.width {
                    // Top edge
                    if y < config.height {
                        unsafe {
                            let fb_ptr = fb_addr as *mut u32;
                            let pixel_index = (y * config.width + px) as usize;
                            *fb_ptr.add(pixel_index) = color;
                        }
                    }
                    // Bottom edge
                    let bottom_y = y + height - 1;
                    if bottom_y < config.height {
                        unsafe {
                            let fb_ptr = fb_addr as *mut u32;
                            let pixel_index = (bottom_y * config.width + px) as usize;
                            *fb_ptr.add(pixel_index) = color;
                        }
                    }
                }
            }
            
            // Left and right edges
            for dy in 0..height {
                let py = y + dy;
                if py < config.height {
                    // Left edge
                    if x < config.width {
                        unsafe {
                            let fb_ptr = fb_addr as *mut u32;
                            let pixel_index = (py * config.width + x) as usize;
                            *fb_ptr.add(pixel_index) = color;
                        }
                    }
                    // Right edge
                    let right_x = x + width - 1;
                    if right_x < config.width {
                        unsafe {
                            let fb_ptr = fb_addr as *mut u32;
                            let pixel_index = (py * config.width + right_x) as usize;
                            *fb_ptr.add(pixel_index) = color;
                        }
                    }
                }
            }
        };
        
        // Clear framebuffer
        unsafe {
            let fb_ptr = fb_addr as *mut u32;
            let pixel_count = (config.width * config.height) as usize;
            for i in 0..pixel_count {
                *fb_ptr.add(i) = 0xFF000000; // Black
            }
        }
        
        // Draw nested borders
        draw_border(10, 10, 200, 150, 0xFF0000FF);    // Red outer border
        draw_border(20, 20, 180, 130, 0xFF00FF00);    // Green middle border
        draw_border(30, 30, 160, 110, 0xFFFF0000);    // Blue inner border
        
        device.flush_framebuffer(0, 0, config.width, config.height).unwrap();
        
        // Verify borders
        unsafe {
            let fb_ptr = fb_addr as *mut u32;
            
            // Check red border corners
            let top_left_red = *fb_ptr.add((10 * config.width + 10) as usize);
            assert_eq!(top_left_red, 0xFF0000FF);
            
            let top_right_red = *fb_ptr.add((10 * config.width + 209) as usize);
            assert_eq!(top_right_red, 0xFF0000FF);
            
            // Check green border
            let green_border = *fb_ptr.add((20 * config.width + 20) as usize);
            assert_eq!(green_border, 0xFF00FF00);
            
            // Check blue border  
            let blue_border = *fb_ptr.add((30 * config.width + 30) as usize);
            assert_eq!(blue_border, 0xFFFF0000);
            
            // Check inside area is still black
            let inside = *fb_ptr.add((50 * config.width + 50) as usize);
            assert_eq!(inside, 0xFF000000);
        }
    }

    #[test_case]
    fn test_virtio_gpu_pixel_format_verification() {
        let mut device = VirtioGpuDevice::new(0x10002000);
        device.init_graphics().unwrap();
        
        let config = device.get_framebuffer_config().unwrap();
        let fb_addr = device.get_framebuffer_address().unwrap();
        
        // Test various pixel format interpretations
        unsafe {
            let fb_ptr = fb_addr as *mut u32;
            
            // Test pure colors in BGRA format
            let test_colors = [
                (0xFF0000FF, "red"),     // Red in BGRA: A=FF, R=00, G=00, B=FF
                (0xFF00FF00, "green"),   // Green in BGRA: A=FF, R=00, G=FF, B=00  
                (0xFFFF0000, "blue"),    // Blue in BGRA: A=FF, R=FF, G=00, B=00
                (0xFFFFFFFF, "white"),   // White in BGRA: A=FF, R=FF, G=FF, B=FF
                (0xFF000000, "black"),   // Black in BGRA: A=FF, R=00, G=00, B=00
                (0xFF808080, "gray"),    // Gray in BGRA: A=FF, R=80, G=80, B=80
            ];
            
            // Write test pattern
            for (i, (color, _name)) in test_colors.iter().enumerate() {
                let x = (i as u32 * 100) % config.width;
                let y = (i as u32 * 100) / config.width;
                if y < config.height {
                    let pixel_index = (y * config.width + x) as usize;
                    *fb_ptr.add(pixel_index) = *color;
                }
            }
            
            device.flush_framebuffer(0, 0, config.width, config.height).unwrap();
            
            // Verify the colors were written correctly
            for (i, (expected_color, _name)) in test_colors.iter().enumerate() {
                let x = (i as u32 * 100) % config.width;
                let y = (i as u32 * 100) / config.width;
                if y < config.height {
                    let pixel_index = (y * config.width + x) as usize;
                    let actual_color = *fb_ptr.add(pixel_index);
                    assert_eq!(actual_color, *expected_color);
                }
            }
        }
        
        // Test partial transparency (though VirtIO GPU might not support it fully)
        unsafe {
            let fb_ptr = fb_addr as *mut u32;
            let semi_transparent_red = 0x800000FF; // 50% transparent red
            let pixel_index = (100 * config.width + 100) as usize;
            *fb_ptr.add(pixel_index) = semi_transparent_red;
            
            device.flush_framebuffer(100, 100, 1, 1).unwrap();
            
            let written_pixel = *fb_ptr.add(pixel_index);
            assert_eq!(written_pixel, semi_transparent_red);
        }
    }

    #[test_case]
    fn test_virtio_gpu_command_flow_verification() {
        let mut device = VirtioGpuDevice::new(0x10002000);
        
        // Test device initialization and command flow
        crate::early_println!("[Test] Starting VirtIO GPU command flow verification");
        device.init_graphics().unwrap();
        
        let config = device.get_framebuffer_config().unwrap();
        let fb_addr = device.get_framebuffer_address().unwrap();
        
        crate::early_println!("[Test] Framebuffer initialized at {:#x}, config: {}x{}", 
            fb_addr, config.width, config.height);
        
        // Write a test pattern and verify the flush process
        unsafe {
            let fb_ptr = fb_addr as *mut u32;
            
            // Write a simple checkerboard pattern
            for y in 0..config.height.min(100) {
                for x in 0..config.width.min(100) {
                    let pixel_index = (y * config.width + x) as usize;
                    let color = if (x / 10 + y / 10) % 2 == 0 {
                        0xFFFF0000 // Blue squares
                    } else {
                        0xFF00FF00 // Green squares  
                    };
                    *fb_ptr.add(pixel_index) = color;
                }
            }
        }
        
        crate::early_println!("[Test] Written checkerboard pattern to framebuffer");
        
        // Test flushing different regions
        device.flush_framebuffer(0, 0, 50, 50).unwrap();
        device.flush_framebuffer(50, 50, 50, 50).unwrap();
        device.flush_framebuffer(0, 0, config.width, config.height).unwrap();
        
        crate::early_println!("[Test] VirtIO GPU command flow verification completed");
    }

    #[test_case]
    fn test_virtio_gpu_resource_management() {
        let mut device = VirtioGpuDevice::new(0x10002000);
        device.init_graphics().unwrap();
        
        // Test that resource IDs are managed correctly
        let config = device.get_framebuffer_config().unwrap();
        let fb_addr = device.get_framebuffer_address().unwrap();
        
        crate::early_println!("[Test] Testing VirtIO GPU resource management");
        crate::early_println!("[Test] Primary framebuffer resource should be ID 1");
        
        // The framebuffer should be associated with resource ID 1
        // (as set up in setup_framebuffer)
        
        // Write some data and flush to verify resource association
        unsafe {
            let fb_ptr = fb_addr as *mut u32;
            // Write a diagonal line pattern
            for i in 0..config.width.min(config.height).min(500) {
                let pixel_index = (i * config.width + i) as usize;
                *fb_ptr.add(pixel_index) = 0xFFFFFF00; // Yellow diagonal
            }
        }
        
        // Flush the diagonal region
        device.flush_framebuffer(0, 0, 
            config.width.min(500), config.height.min(500)).unwrap();
        
        crate::early_println!("[Test] Resource management test completed");
    }
}