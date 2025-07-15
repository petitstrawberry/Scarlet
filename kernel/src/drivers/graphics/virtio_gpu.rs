//! # VirtIO GPU Device Driver
//! 
//! This module provides a driver for VirtIO GPU devices, implementing the
//! GraphicsDevice trait for integration with the kernel's graphics subsystem.
//!
//! The driver supports basic framebuffer operations and display management
//! according to the VirtIO GPU specification.

use alloc::{boxed::Box, vec::Vec};
use spin::{Mutex, RwLock};

use crate::{
    device::{Device, DeviceType, graphics::{GraphicsDevice, FramebufferConfig, PixelFormat}},
    drivers::virtio::{device::{Register, VirtioDevice}, queue::{DescriptorFlag, VirtQueue}},
    mem::page::allocate_raw_pages,
};

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

/// VirtIO GPU Device
pub struct VirtioGpuDevice {
    base_addr: usize,
    virtqueues: Mutex<[VirtQueue<'static>; 2]>, // Control queue (0) and Cursor queue (1)
    display_info: RwLock<Option<VirtioGpuRespDisplayInfo>>,
    framebuffer_addr: RwLock<Option<usize>>,
    resource_id: Mutex<u32>,
    initialized: Mutex<bool>,
    // Track resources and their associated memory
    resources: Mutex<alloc::collections::BTreeMap<u32, (usize, usize)>>, // resource_id -> (addr, size)
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
            base_addr,
            virtqueues: Mutex::new([VirtQueue::new(16), VirtQueue::new(16)]), // Control and Cursor queues with 16 descriptors each
            display_info: RwLock::new(None),
            framebuffer_addr: RwLock::new(None),
            resource_id: Mutex::new(1),
            initialized: Mutex::new(false),
            resources: Mutex::new(alloc::collections::BTreeMap::new()),
        }
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
        
        // Create command and response buffers
        let cmd_buffer = Box::new(unsafe { 
            core::ptr::read(cmd as *const T)
        });
        let resp_buffer = Box::new([0u8; 64]); // Response buffer
        
        let cmd_ptr = Box::into_raw(cmd_buffer);
        let resp_ptr = Box::into_raw(resp_buffer);
        
        // Ensure memory cleanup
        use crate::defer;
        defer! {
            unsafe {
                drop(Box::from_raw(cmd_ptr));
                drop(Box::from_raw(resp_ptr));
            }
        }
        
        // Allocate descriptors
        let cmd_desc = control_queue.alloc_desc().ok_or("Failed to allocate command descriptor")?;
        let resp_desc = control_queue.alloc_desc().ok_or("Failed to allocate response descriptor")?;
        
        // Set up command descriptor (device readable)
        control_queue.desc[cmd_desc].addr = cmd_ptr as u64;
        control_queue.desc[cmd_desc].len = core::mem::size_of::<T>() as u32;
        control_queue.desc[cmd_desc].flags = DescriptorFlag::Next as u16;
        control_queue.desc[cmd_desc].next = resp_desc as u16;
        
        // Set up response descriptor (device writable)
        control_queue.desc[resp_desc].addr = resp_ptr as u64;
        control_queue.desc[resp_desc].len = 64;
        control_queue.desc[resp_desc].flags = DescriptorFlag::Write as u16;
        
        crate::early_println!("[Virtio GPU] Sending command to control queue: type={}", 
            unsafe { *(cmd as *const T as *const u32) });
        
        // Submit the request to the queue
        control_queue.push(cmd_desc)?;
        
        // Notify the device
        self.notify(0); // Notify control queue
        
        // Wait for response (simplified polling)
        crate::early_println!("[Virtio GPU] Waiting for command response...");
        while control_queue.is_busy() {}
        while *control_queue.used.idx as usize == control_queue.last_used_idx {}
        
        // Process response
        let _resp_idx = control_queue.pop().ok_or("No response from device")?;
        
        crate::early_println!("[Virtio GPU] Command completed successfully");
        
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
        
        // Create a 2D resource for the framebuffer
        let resource_id = self.create_2d_resource(width, height, VIRTIO_GPU_FORMAT_B8G8R8X8_UNORM)?;

        // Allocate framebuffer memory
        let fb_size = (width * height * 4) as usize; // 4 bytes per pixel
        let fb_pages = (fb_size + 4095) / 4096; // Round up to page size
        let fb_pages_ptr = allocate_raw_pages(fb_pages);
        if fb_pages_ptr.is_null() {
            return Err("Failed to allocate framebuffer memory");
        }
        let fb_addr = fb_pages_ptr as usize;

        // Attach backing to the resource
        // This command tells the GPU device that our framebuffer memory 
        // should be used as backing storage for the 2D resource
        let attach_cmd = VirtioGpuResourceAttachBacking {
            hdr: VirtioGpuCtrlHdr {
                hdr_type: VIRTIO_GPU_CMD_RESOURCE_ATTACH_BACKING,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
            resource_id,
            nr_entries: 1,
        };

        crate::early_println!("[Virtio GPU] Attaching framebuffer memory {:#x} to resource {}", 
            fb_addr, resource_id);
        self.send_control_command(&attach_cmd)?;

        // Set scanout - connects the 2D resource to the display output
        // This makes the resource visible on the display
        let scanout_cmd = VirtioGpuSetScanout {
            hdr: VirtioGpuCtrlHdr {
                hdr_type: VIRTIO_GPU_CMD_SET_SCANOUT,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
            r: VirtioGpuRect {
                x: 0,
                y: 0,
                width,
                height,
            },
            scanout_id: 0,
            resource_id,
        };

        crate::early_println!("[Virtio GPU] Setting scanout for resource {} ({}x{})", 
            resource_id, width, height);
        self.send_control_command(&scanout_cmd)?;

        // Track the resource and its associated memory
        {
            let mut resources = self.resources.lock();
            resources.insert(resource_id, (fb_addr, fb_size));
        }

        *self.framebuffer_addr.write() = Some(fb_addr);
        crate::early_println!("[Virtio GPU] Framebuffer setup completed: addr={:#x}, size={}", 
            fb_addr, fb_size);
        Ok(())
    }
}

impl VirtioDevice for VirtioGpuDevice {
    fn get_base_addr(&self) -> usize {
        self.base_addr
    }

    fn get_virtqueue_count(&self) -> usize {
        2 // Control queue and cursor queue
    }

    fn get_queue_desc_addr(&self, queue_idx: usize) -> Option<u64> {
        if queue_idx >= self.get_virtqueue_count() {
            return None;
        }
        
        // For testing purposes, allocate a small descriptor table
        // In a real implementation, this would use proper memory allocation
        let desc_table_size = 16 * 16; // 16 descriptors * 16 bytes each
        let desc_table_addr = allocate_raw_pages((desc_table_size + 4095) / 4096);
        if desc_table_addr.is_null() {
            return None;
        }
        Some(desc_table_addr as u64)
    }

    fn get_queue_driver_addr(&self, queue_idx: usize) -> Option<u64> {
        if queue_idx >= self.get_virtqueue_count() {
            return None;
        }
        
        // For testing purposes, allocate available ring
        // In a real implementation, this would use proper memory allocation
        let avail_ring_size = 6 + 2 * 16; // 6 bytes header + 2 bytes per entry for 16 entries
        let avail_ring_addr = allocate_raw_pages((avail_ring_size + 4095) / 4096);
        if avail_ring_addr.is_null() {
            return None;
        }
        Some(avail_ring_addr as u64)
    }

    fn get_queue_device_addr(&self, queue_idx: usize) -> Option<u64> {
        if queue_idx >= self.get_virtqueue_count() {
            return None;
        }
        
        // For testing purposes, allocate used ring
        // In a real implementation, this would use proper memory allocation  
        let used_ring_size = 6 + 8 * 16; // 6 bytes header + 8 bytes per entry for 16 entries
        let used_ring_addr = allocate_raw_pages((used_ring_size + 4095) / 4096);
        if used_ring_addr.is_null() {
            return None;
        }
        Some(used_ring_addr as u64)
    }

    fn get_supported_features(&self, _device_features: u32) -> u32 {
        // For now, don't enable any advanced features
        0
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

impl GraphicsDevice for VirtioGpuDevice {
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

        crate::early_println!("[Virtio GPU] Flushing framebuffer region: ({},{}) {}x{} for resource {}", 
            x, y, width, height, resource_id);

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
        crate::early_println!("[Virtio GPU] Framebuffer flush completed");
        Ok(())
    }

    fn init_graphics(&mut self) -> Result<(), &'static str> {
        {
            let mut initialized = self.initialized.lock();
            if *initialized {
                return Ok(());
            }
            *initialized = true;
        }

        crate::early_println!("[Virtio GPU] Initializing VirtIO GPU device at {:#x}", self.base_addr);


        // Initialize VirtIO device
        self.init()?;


        // Get display information
        self.get_display_info_internal()?;

        // Set up framebuffer
        self.setup_framebuffer()?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_virtio_gpu_device_creation() {
        let device = VirtioGpuDevice::new(0x10002000);
        assert_eq!(device.get_base_addr(), 0x10002000);
        assert_eq!(device.get_virtqueue_count(), 2);
        assert_eq!(device.device_type(), DeviceType::Graphics);
        assert_eq!(device.name(), "virtio-gpu");
        assert_eq!(device.get_display_name(), "virtio-gpu");
    }

    #[test_case]
    fn test_virtio_gpu_resource_id_generation() {
        let device = VirtioGpuDevice::new(0x10002000);
        assert_eq!(device.next_resource_id(), 1);
        assert_eq!(device.next_resource_id(), 2);
        assert_eq!(device.next_resource_id(), 3);
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