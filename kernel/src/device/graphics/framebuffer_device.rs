//! # Framebuffer Character Device Module
//!
//! This module provides character device interface for framebuffer access.
//! It integrates with the GraphicsManager to provide user-space access to
//! framebuffer resources through the standard character device interface.
//!
//! ## Overview
//!
//! The FramebufferCharDevice provides:
//! - Basic read/write operations to framebuffer memory
//! - Control operations (ioctl-equivalent) for device configuration
//! - Integration with GraphicsManager for resource management
//! - Standard character device interface for user programs
//! - Support for Linux-compatible framebuffer ioctls

extern crate alloc;

use alloc::{collections::BTreeMap, sync::Arc, vec, vec::Vec};
use core::any::Any;
use spin::{Mutex, RwLock};

use crate::device::{
    Device, DeviceType,
    char::CharDevice,
    graphics::{FramebufferConfig, manager::FramebufferResource, output::DisplayRegion},
    manager::DeviceManager,
};
use crate::environment::PAGE_SIZE;
use crate::object::capability::memory_mapping::{
    AccessKind, AccessOp, ResolveFaultError, ResolveFaultResult,
};
use crate::object::capability::selectable::Selectable;
use crate::object::capability::{ControlOps, MemoryMappingOps};
use crate::sched::scheduler::get_task_by_id;
use crate::timer::{TimerHandler, add_timer, get_tick, ms_to_ticks};
use crate::vm::addr::phys_to_virt;

/// Linux framebuffer ioctl command constants
/// These provide compatibility with Linux framebuffer applications
pub mod framebuffer_commands {
    /// Get variable screen information
    pub const FBIOGET_VSCREENINFO: u32 = 0x4600;
    /// Set variable screen information  
    pub const FBIOPUT_VSCREENINFO: u32 = 0x4601;
    /// Get fixed screen information
    pub const FBIOGET_FSCREENINFO: u32 = 0x4602;
    /// Flush framebuffer to display
    pub const FBIO_FLUSH: u32 = 0x4620;
}

/// Variable screen information structure (Linux fb_var_screeninfo compatible)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct FbVarScreenInfo {
    /// Visible resolution width
    pub xres: u32,
    /// Visible resolution height  
    pub yres: u32,
    /// Virtual resolution width
    pub xres_virtual: u32,
    /// Virtual resolution height
    pub yres_virtual: u32,
    /// Offset from virtual to visible resolution
    pub xoffset: u32,
    /// Offset from virtual to visible resolution
    pub yoffset: u32,
    /// Bits per pixel
    pub bits_per_pixel: u32,
    /// Grayscale != 0 means graylevels instead of colors
    pub grayscale: u32,
    /// Red bitfield
    pub red: FbBitfield,
    /// Green bitfield
    pub green: FbBitfield,
    /// Blue bitfield
    pub blue: FbBitfield,
    /// Transparency bitfield
    pub transp: FbBitfield,
    /// Non-zero if not grayscale
    pub nonstd: u32,
    /// Activate settings
    pub activate: u32,
    /// Screen height in mm
    pub height: u32,
    /// Screen width in mm
    pub width: u32,
    /// Acceleration flags
    pub accel_flags: u32,
    /// Pixel clock in picoseconds
    pub pixclock: u32,
    /// Time from sync to picture
    pub left_margin: u32,
    /// Time from picture to sync
    pub right_margin: u32,
    /// Time from sync to picture
    pub upper_margin: u32,
    /// Time from picture to sync
    pub lower_margin: u32,
    /// Length of horizontal sync
    pub hsync_len: u32,
    /// Length of vertical sync
    pub vsync_len: u32,
    /// Sync flags
    pub sync: u32,
    /// Video mode flags
    pub vmode: u32,
    /// Rotation angle (0=normal, 1=90°, 2=180°, 3=270°)
    pub rotate: u32,
    /// Color space for frame buffer
    pub colorspace: u32,
    /// Reserved for future use
    pub reserved: [u32; 4],
}

impl Default for FbVarScreenInfo {
    fn default() -> Self {
        Self {
            xres: 0,
            yres: 0,
            xres_virtual: 0,
            yres_virtual: 0,
            xoffset: 0,
            yoffset: 0,
            bits_per_pixel: 0,
            grayscale: 0,
            red: FbBitfield::default(),
            green: FbBitfield::default(),
            blue: FbBitfield::default(),
            transp: FbBitfield::default(),
            nonstd: 0,
            activate: 0,
            height: 0,
            width: 0,
            accel_flags: 0,
            pixclock: 0,
            left_margin: 0,
            right_margin: 0,
            upper_margin: 0,
            lower_margin: 0,
            hsync_len: 0,
            vsync_len: 0,
            sync: 0,
            vmode: 0,
            rotate: 0,
            colorspace: 0,
            reserved: [0; 4],
        }
    }
}

/// Fixed screen information structure (Linux fb_fix_screeninfo compatible)
#[repr(C)]
#[derive(Debug, Clone)]
pub struct FbFixScreenInfo {
    /// Identification string
    pub id: [u8; 16],
    /// Start of frame buffer memory (physical address)
    pub smem_start: usize,
    /// Length of frame buffer memory
    pub smem_len: u32,
    /// Framebuffer type
    pub type_: u32,
    /// Type of auxiliary display
    pub type_aux: u32,
    /// Visual type
    pub visual: u32,
    /// Zero if no hardware panning
    pub xpanstep: u16,
    /// Zero if no hardware panning
    pub ypanstep: u16,
    /// Zero if no hardware ywrap
    pub ywrapstep: u16,
    /// Length of a line in bytes
    pub line_length: u32,
    /// Start of memory mapped I/O
    pub mmio_start: usize,
    /// Length of memory mapped I/O
    pub mmio_len: u32,
    /// Acceleration type
    pub accel: u32,
    /// Capabilities
    pub capabilities: u16,
    /// Reserved for future compatibility
    pub reserved: [u16; 2],
}

impl Default for FbFixScreenInfo {
    fn default() -> Self {
        Self {
            id: [0; 16],
            smem_start: 0,
            smem_len: 0,
            type_: 0,
            type_aux: 0,
            visual: 0,
            xpanstep: 0,
            ypanstep: 0,
            ywrapstep: 0,
            line_length: 0,
            mmio_start: 0,
            mmio_len: 0,
            accel: 0,
            capabilities: 0,
            reserved: [0; 2],
        }
    }
}

/// Bitfield information for color components
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct FbBitfield {
    /// Beginning of bitfield (LSB is 0)
    pub offset: u32,
    /// Length of bitfield
    pub length: u32,
    /// MSB position (0 = MSB is rightmost)
    pub msb_right: u32,
}

impl Default for FbBitfield {
    fn default() -> Self {
        Self {
            offset: 0,
            length: 0,
            msb_right: 0,
        }
    }
}

/// Active legacy framebuffer mmap alias.
#[derive(Debug, Clone)]
struct FramebufferMapping {
    task_id: usize,
    vaddr: usize,
    length: usize,
    offset: usize,
    write_protect: bool,
}

#[derive(Debug)]
struct FbCompatDirtyState {
    dirty: bool,
    timer_armed: bool,
    generation: usize,
}

struct FbCompatFlushHandler {
    fb_resource: Arc<FramebufferResource>,
    mappings: Arc<RwLock<BTreeMap<(usize, usize), FramebufferMapping>>>,
    dirty_state: Arc<Mutex<FbCompatDirtyState>>,
    #[cfg(test)]
    device_manager_addr: Option<usize>,
}

/// Framebuffer character device implementation
///
/// This device provides character-based access to framebuffer memory.
/// It acts as a bridge between user-space programs and the graphics
/// hardware through the GraphicsManager.
///
/// Note: This device is stateless and does not maintain position information.
/// Position management is handled by the FileObject layer for proper POSIX semantics.
pub struct FramebufferCharDevice {
    /// The framebuffer resource this device represents
    fb_resource: Arc<FramebufferResource>,
    /// Track legacy framebuffer mmap aliases by (task_id, virtual_start).
    mappings: Arc<RwLock<BTreeMap<(usize, usize), FramebufferMapping>>>,
    /// Dirty state for legacy fb compat one-shot flushing.
    dirty_state: Arc<Mutex<FbCompatDirtyState>>,
    /// Timer handler kept alive for the legacy fb compat one-shot flush.
    flush_handler: Arc<FbCompatFlushHandler>,
    #[cfg(test)]
    device_manager_addr: Option<usize>,
}

struct CurrentFramebufferInfo {
    config: FramebufferConfig,
    physical_addr: usize,
    size: usize,
}

impl FramebufferCharDevice {
    /// Create a new framebuffer character device
    ///
    /// # Arguments
    ///
    /// * `fb_resource` - The framebuffer resource to access
    ///
    /// # Returns
    ///
    /// A new FramebufferCharDevice instance
    pub fn new(fb_resource: Arc<FramebufferResource>) -> Self {
        let mappings = Arc::new(RwLock::new(BTreeMap::new()));
        let dirty_state = Arc::new(Mutex::new(FbCompatDirtyState {
            dirty: false,
            timer_armed: false,
            generation: 0,
        }));
        let flush_handler = Arc::new(FbCompatFlushHandler {
            fb_resource: Arc::clone(&fb_resource),
            mappings: Arc::clone(&mappings),
            dirty_state: Arc::clone(&dirty_state),
            #[cfg(test)]
            device_manager_addr: None,
        });

        Self {
            fb_resource,
            mappings,
            dirty_state,
            flush_handler,
            #[cfg(test)]
            device_manager_addr: None,
        }
    }

    #[cfg(test)]
    pub fn new_with_device_manager(
        fb_resource: Arc<FramebufferResource>,
        device_manager: &DeviceManager,
    ) -> Self {
        let mappings = Arc::new(RwLock::new(BTreeMap::new()));
        let dirty_state = Arc::new(Mutex::new(FbCompatDirtyState {
            dirty: false,
            timer_armed: false,
            generation: 0,
        }));
        let device_manager_addr = device_manager as *const DeviceManager as usize;
        let flush_handler = Arc::new(FbCompatFlushHandler {
            fb_resource: Arc::clone(&fb_resource),
            mappings: Arc::clone(&mappings),
            dirty_state: Arc::clone(&dirty_state),
            device_manager_addr: Some(device_manager_addr),
        });

        Self {
            fb_resource,
            mappings,
            dirty_state,
            flush_handler,
            device_manager_addr: Some(device_manager_addr),
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

    /// Get the framebuffer name this device represents
    pub fn get_framebuffer_name(&self) -> &str {
        &self.fb_resource.logical_name
    }

    fn page_aligned_size(size: usize) -> usize {
        (size + crate::environment::PAGE_SIZE - 1) & !(crate::environment::PAGE_SIZE - 1)
    }

    fn current_framebuffer_info(&self) -> Result<CurrentFramebufferInfo, &'static str> {
        if let Some(device) = self
            .device_manager()
            .get_device(self.fb_resource.source_device_id)
        {
            if let Some(graphics_device) = device.as_graphics_device() {
                let config = graphics_device.get_framebuffer_config()?;
                let physical_addr = graphics_device.get_framebuffer_address()?;
                let size = Self::page_aligned_size(config.size());
                return Ok(CurrentFramebufferInfo {
                    config,
                    physical_addr,
                    size,
                });
            }
        }

        Ok(CurrentFramebufferInfo {
            config: self.fb_resource.config.clone(),
            physical_addr: self.fb_resource.physical_addr,
            size: self.fb_resource.size,
        })
    }

    fn current_task_id() -> usize {
        crate::task::mytask().map(|task| task.get_id()).unwrap_or(0)
    }

    fn record_mapping_for_current_task(
        &self,
        vaddr: usize,
        length: usize,
        offset: usize,
        write_protect: bool,
    ) {
        let task_id = Self::current_task_id();
        let mapping = FramebufferMapping {
            task_id,
            vaddr,
            length,
            offset,
            write_protect,
        };
        self.mappings.write().insert((task_id, vaddr), mapping);
    }

    fn mark_legacy_dirty(&self) {
        let arm_context = {
            let mut state = self.dirty_state.lock();
            state.dirty = true;
            if state.timer_armed {
                None
            } else {
                state.timer_armed = true;
                state.generation = state.generation.wrapping_add(1);
                Some(state.generation)
            }
        };

        if let Some(context) = arm_context {
            let handler: Arc<dyn TimerHandler> = self.flush_handler.clone();
            add_timer(
                get_tick().saturating_add(ms_to_ticks(16)),
                &handler,
                context,
            );
        }
    }

    fn disarm_legacy_dirty_timer(&self) {
        let mut state = self.dirty_state.lock();
        state.dirty = false;
        state.timer_armed = false;
        state.generation = state.generation.wrapping_add(1);
    }

    fn mapping_for_fault(
        &self,
        access: &AccessKind,
        vm_start: usize,
        info: &CurrentFramebufferInfo,
    ) -> Option<FramebufferMapping> {
        let task_id = Self::current_task_id();
        if let Some(mapping) = self.mappings.read().get(&(task_id, vm_start)).cloned() {
            return Some(mapping);
        }

        let task = crate::task::mytask()?;
        let memory_map = task.vm_manager.search_memory_map(access.vaddr)?;
        let map_object_offset = memory_map.pmarea.start.saturating_sub(info.physical_addr);
        let map_vm_offset = memory_map.vmarea.start.saturating_sub(memory_map.vm_start);
        let base_offset = map_object_offset.saturating_sub(map_vm_offset);
        let mapping = FramebufferMapping {
            task_id,
            vaddr: memory_map.vm_start,
            length: memory_map
                .vmarea
                .end
                .saturating_sub(memory_map.vm_start)
                .saturating_add(1),
            offset: base_offset,
            write_protect: memory_map.is_shared,
        };
        self.mappings
            .write()
            .insert((task_id, memory_map.vm_start), mapping.clone());
        Some(mapping)
    }

    fn write_protect_mappings(
        mappings: &RwLock<BTreeMap<(usize, usize), FramebufferMapping>>,
        info: &CurrentFramebufferInfo,
    ) {
        let snapshot: Vec<FramebufferMapping> = mappings.read().values().cloned().collect();
        for mapping in snapshot {
            if !mapping.write_protect {
                continue;
            }
            let Some(task) = get_task_by_id(mapping.task_id) else {
                continue;
            };
            let Some(root_pagetable) = task.vm_manager.get_root_page_table() else {
                continue;
            };

            let asid = task.vm_manager.get_asid();
            let mut page_offset = 0usize;
            while page_offset < mapping.length {
                let object_offset = mapping.offset.saturating_add(page_offset);
                if object_offset >= info.size {
                    break;
                }

                root_pagetable.map(
                    asid,
                    mapping.vaddr.saturating_add(page_offset),
                    info.physical_addr.saturating_add(object_offset),
                    0x1 | 0x08,
                    true,
                    false,
                );
                page_offset = page_offset.saturating_add(PAGE_SIZE);
            }
        }
    }
}

impl FbCompatFlushHandler {
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

    fn current_framebuffer_info(&self) -> Result<CurrentFramebufferInfo, &'static str> {
        if let Some(device) = self
            .device_manager()
            .get_device(self.fb_resource.source_device_id)
        {
            if let Some(graphics_device) = device.as_graphics_device() {
                let config = graphics_device.get_framebuffer_config()?;
                let physical_addr = graphics_device.get_framebuffer_address()?;
                let size = FramebufferCharDevice::page_aligned_size(config.size());
                return Ok(CurrentFramebufferInfo {
                    config,
                    physical_addr,
                    size,
                });
            }
        }

        Ok(CurrentFramebufferInfo {
            config: self.fb_resource.config.clone(),
            physical_addr: self.fb_resource.physical_addr,
            size: self.fb_resource.size,
        })
    }

    fn trigger_display_update(&self, region: DisplayRegion) -> Result<(), &'static str> {
        let device_manager = self.device_manager();
        if let Some(device) = device_manager.get_device(self.fb_resource.source_device_id) {
            if let Some(graphics_device) = device.as_graphics_device() {
                let (config, physical_addr) = graphics_device.get_framebuffer_info()?;
                let region = DisplayRegion::new(
                    region.x.min(config.width),
                    region.y.min(config.height),
                    region.width.min(config.width.saturating_sub(region.x)),
                    region.height.min(config.height.saturating_sub(region.y)),
                );
                graphics_device.present_current_framebuffer_region(region)?;
                if physical_addr == 0 {
                    return Err("Graphics device framebuffer address is null");
                }
            }
        }

        Ok(())
    }
}

impl TimerHandler for FbCompatFlushHandler {
    fn on_timer_expired(self: Arc<Self>, context: usize) {
        let should_flush = {
            let mut state = self.dirty_state.lock();
            if !state.timer_armed || state.generation != context {
                false
            } else {
                state.timer_armed = false;
                if state.dirty {
                    state.dirty = false;
                    true
                } else {
                    false
                }
            }
        };

        if !should_flush {
            return;
        }

        if let Ok(info) = self.current_framebuffer_info() {
            if info.physical_addr != 0 {
                let _ = self.trigger_display_update(DisplayRegion::full(&info.config));
                FramebufferCharDevice::write_protect_mappings(&self.mappings, &info);
            }
        }
    }
}

impl Device for FramebufferCharDevice {
    fn device_type(&self) -> DeviceType {
        DeviceType::Char
    }

    fn name(&self) -> &'static str {
        "framebuffer"
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

impl CharDevice for FramebufferCharDevice {
    /// Read a single byte from the framebuffer
    ///
    /// This method is not supported in the new position-per-file-handle architecture.
    /// Use read_at() through a DevFileObject instead for proper position management.
    ///
    /// # Returns
    ///
    /// Always returns None to indicate unsupported operation
    fn read_byte(&self) -> Option<u8> {
        // This method is intentionally unsupported in the new architecture
        // Position management should be done by DevFileObject, not the device
        None
    }

    /// Write a single byte to the framebuffer
    ///
    /// This method is not supported in the new position-per-file-handle architecture.
    /// Use write_at() through a DevFileObject instead for proper position management.
    ///
    /// # Arguments
    ///
    /// * `_byte` - The byte to write (ignored)
    ///
    /// # Returns
    ///
    /// Always returns an error to indicate unsupported operation
    fn write_byte(&self, _byte: u8) -> Result<(), &'static str> {
        // This method is intentionally unsupported in the new architecture
        // Position management should be done by DevFileObject, not the device
        Err("write_byte is not supported - use write_at through DevFileObject instead")
    }

    /// Write multiple bytes to the framebuffer
    ///
    /// # Returns
    ///
    /// Always returns an error to indicate unsupported operation
    fn write(&self, _buffer: &[u8]) -> Result<usize, &'static str> {
        Err("write is not supported - use write_at through DevFileObject instead")
    }

    /// Check if the device is ready for reading
    ///
    /// # Returns
    ///
    /// True if framebuffer is valid
    fn can_read(&self) -> bool {
        match self.current_framebuffer_info() {
            Ok(info) => info.physical_addr != 0 && info.size > 0,
            Err(_) => false,
        }
    }

    /// Check if the device is ready for writing
    ///
    /// # Returns
    ///
    /// True if framebuffer is valid
    fn can_write(&self) -> bool {
        match self.current_framebuffer_info() {
            Ok(info) => info.physical_addr != 0 && info.size > 0,
            Err(_) => false,
        }
    }

    /// Read data from a specific position in the framebuffer
    ///
    /// # Arguments
    ///
    /// * `position` - Byte offset to read from
    /// * `buffer` - Buffer to read data into
    ///
    /// # Returns
    ///
    /// Result containing the number of bytes read or an error
    fn read_at(&self, position: u64, buffer: &mut [u8]) -> Result<usize, &'static str> {
        let info = self.current_framebuffer_info()?;

        // Check if framebuffer address is valid
        if info.physical_addr == 0 {
            return Err("Invalid framebuffer address");
        }

        // Use logical framebuffer size for boundary checks
        let logical_size = info.config.size();
        let start_pos = position as usize;
        if start_pos >= logical_size {
            return Ok(0); // EOF
        }

        let available = logical_size - start_pos;
        let to_read = buffer.len().min(available);

        // Read data from framebuffer memory.
        //
        // NOTE: On QEMU+HVF, some non-volatile memcpy-style accesses to MMIO-like
        // framebuffer regions can VM-exit as EC_DATAABORT without ISV set, which
        // causes QEMU's HVF backend to abort (assert(isv)). Use byte-wise volatile
        // accesses to keep the trapped instruction decodable.
        unsafe {
            let fb_ptr = phys_to_virt(info.physical_addr) as *const u8;
            let src_ptr = fb_ptr.add(start_pos);

            for i in 0..to_read {
                buffer[i] = core::ptr::read_volatile(src_ptr.add(i));
            }
        }

        Ok(to_read)
    }

    /// Write data to a specific position in the framebuffer
    ///
    /// # Arguments
    ///
    /// * `position` - Byte offset to write to
    /// * `buffer` - Buffer containing data to write
    ///
    /// # Returns
    ///
    /// Result containing the number of bytes written or an error
    fn write_at(&self, position: u64, buffer: &[u8]) -> Result<usize, &'static str> {
        let info = self.current_framebuffer_info()?;

        // Check if framebuffer address is valid
        if info.physical_addr == 0 {
            return Err("Invalid framebuffer address");
        }

        // Use logical framebuffer size for boundary checks
        let logical_size = info.config.size();
        let start_pos = position as usize;
        if start_pos >= logical_size {
            return Err("Position beyond framebuffer size");
        }

        let available = logical_size - start_pos;
        let to_write = buffer.len().min(available);

        // Write data to framebuffer memory.
        // See note in read_at() about QEMU+HVF and ISV.
        unsafe {
            let fb_ptr = phys_to_virt(info.physical_addr) as *mut u8;
            let dst_ptr = fb_ptr.add(start_pos);

            for i in 0..to_write {
                core::ptr::write_volatile(dst_ptr.add(i), buffer[i]);
            }
        }

        if to_write > 0 {
            self.mark_legacy_dirty();
        }

        Ok(to_write)
    }
}

impl ControlOps for FramebufferCharDevice {
    fn control(&self, command: u32, arg: usize) -> Result<i32, &'static str> {
        use framebuffer_commands::*;

        match command {
            FBIOGET_VSCREENINFO => self.handle_get_vscreeninfo(arg),
            FBIOGET_FSCREENINFO => self.handle_get_fscreeninfo(arg),
            FBIO_FLUSH => self.handle_flush(arg),
            FBIOPUT_VSCREENINFO => self.handle_put_vscreeninfo(arg),
            _ => Err("Unsupported framebuffer control command"),
        }
    }

    fn supported_control_commands(&self) -> Vec<(u32, &'static str)> {
        use framebuffer_commands::*;
        vec![
            (FBIOGET_VSCREENINFO, "Get variable screen information"),
            (FBIOGET_FSCREENINFO, "Get fixed screen information"),
            (FBIO_FLUSH, "Flush framebuffer to display"),
            (FBIOPUT_VSCREENINFO, "Set variable screen information"),
        ]
    }
}

impl MemoryMappingOps for FramebufferCharDevice {
    fn get_mapping_info(
        &self,
        offset: usize,
        length: usize,
    ) -> Result<(usize, usize, bool), &'static str> {
        let info = self.current_framebuffer_info()?;

        // VMM requires page-aligned physical mappings.
        if offset % crate::environment::PAGE_SIZE != 0 {
            return Err("Framebuffer mmap offset must be page-aligned");
        }
        if length % crate::environment::PAGE_SIZE != 0 {
            return Err("Framebuffer mmap length must be page-aligned");
        }

        // Check if framebuffer supports memory mapping
        if info.physical_addr == 0 || info.size == 0 {
            return Err("Invalid framebuffer configuration");
        }
        if info.physical_addr % crate::environment::PAGE_SIZE != 0 {
            return Err("Framebuffer physical address must be page-aligned");
        }

        // Basic validation
        // fb_resource.size is the page-aligned physical size, safe for mmap
        if offset >= info.size {
            return Err("Offset exceeds framebuffer size");
        }

        let available_size = info.size - offset;
        if length > available_size {
            return Err("Requested length exceeds available framebuffer size");
        }

        let paddr = info.physical_addr + offset;
        let permissions = 0x3; // Read and Write
        let is_shared = true; // Framebuffer mappings are shared

        Ok((paddr, permissions, is_shared))
    }

    fn on_mapped(&self, vaddr: usize, paddr: usize, length: usize, offset: usize) {
        self.record_mapping_for_current_task(vaddr, length, offset, paddr != 0);
    }

    fn on_unmapped(&self, vaddr: usize, length: usize) {
        let task_id = Self::current_task_id();
        let unmap_end = vaddr.saturating_add(length);
        self.mappings.write().retain(|_, mapping| {
            if task_id != 0 && mapping.task_id != task_id {
                return true;
            }
            let mapping_end = mapping.vaddr.saturating_add(mapping.length);
            mapping_end <= vaddr || mapping.vaddr >= unmap_end
        });
    }

    fn supports_mmap(&self) -> bool {
        match self.current_framebuffer_info() {
            Ok(info) => info.physical_addr != 0 && info.size > 0,
            Err(_) => false,
        }
    }

    fn resolve_fault(
        &self,
        access: &AccessKind,
        _page_idx: usize,
        vm_start: usize,
    ) -> Result<ResolveFaultResult, ResolveFaultError> {
        let info = self
            .current_framebuffer_info()
            .map_err(|_| ResolveFaultError::Invalid)?;
        let mapping = self
            .mapping_for_fault(access, vm_start, &info)
            .ok_or(ResolveFaultError::Invalid)?;

        if access.vaddr < mapping.vaddr {
            return Err(ResolveFaultError::Invalid);
        }

        let page_offset = (access.vaddr - mapping.vaddr) & !(PAGE_SIZE - 1);
        let object_offset = mapping.offset.saturating_add(page_offset);
        if object_offset >= info.size {
            return Err(ResolveFaultError::Unmapped);
        }

        if matches!(access.op, AccessOp::Store) {
            self.mark_legacy_dirty();
        }

        Ok(ResolveFaultResult {
            paddr_page_base: info.physical_addr.saturating_add(object_offset),
            is_tail: false,
        })
    }

    fn fault_page_permissions(&self, access: &AccessKind, default_permissions: usize) -> usize {
        if matches!(access.op, AccessOp::Store) {
            default_permissions
        } else {
            default_permissions & !0x2
        }
    }

    fn mmap_owner_name(&self) -> alloc::string::String {
        alloc::string::String::from("framebuffer")
    }
}

impl Selectable for FramebufferCharDevice {
    fn wait_until_ready(
        &self,
        _interest: crate::object::capability::selectable::ReadyInterest,
        _trapframe: &mut crate::arch::Trapframe,
        _timeout_ticks: Option<u64>,
        _min_wait_ticks: u64,
    ) -> crate::object::capability::selectable::SelectWaitOutcome {
        crate::object::capability::selectable::SelectWaitOutcome::Ready
    }
}

impl FramebufferCharDevice {
    /// Build a FbVarScreenInfo reflecting the current framebuffer configuration
    fn current_var_info(&self) -> FbVarScreenInfo {
        let info = self
            .current_framebuffer_info()
            .unwrap_or_else(|_| CurrentFramebufferInfo {
                config: self.fb_resource.config.clone(),
                physical_addr: self.fb_resource.physical_addr,
                size: self.fb_resource.size,
            });
        let config = &info.config;

        let mut var_info = FbVarScreenInfo::default();
        var_info.xres = config.width;
        var_info.yres = config.height;
        var_info.xres_virtual = config.width;
        var_info.yres_virtual = config.height;
        var_info.bits_per_pixel = (config.format.bytes_per_pixel() * 8) as u32;

        match config.format {
            super::PixelFormat::RGBA8888 => {
                var_info.red = FbBitfield {
                    offset: 0,
                    length: 8,
                    msb_right: 0,
                };
                var_info.green = FbBitfield {
                    offset: 8,
                    length: 8,
                    msb_right: 0,
                };
                var_info.blue = FbBitfield {
                    offset: 16,
                    length: 8,
                    msb_right: 0,
                };
                var_info.transp = FbBitfield {
                    offset: 24,
                    length: 8,
                    msb_right: 0,
                };
            }
            super::PixelFormat::BGRA8888 => {
                var_info.blue = FbBitfield {
                    offset: 0,
                    length: 8,
                    msb_right: 0,
                };
                var_info.green = FbBitfield {
                    offset: 8,
                    length: 8,
                    msb_right: 0,
                };
                var_info.red = FbBitfield {
                    offset: 16,
                    length: 8,
                    msb_right: 0,
                };
                var_info.transp = FbBitfield {
                    offset: 24,
                    length: 8,
                    msb_right: 0,
                };
            }
            super::PixelFormat::XRGB8888 => {
                var_info.red = FbBitfield {
                    offset: 0,
                    length: 8,
                    msb_right: 0,
                };
                var_info.green = FbBitfield {
                    offset: 8,
                    length: 8,
                    msb_right: 0,
                };
                var_info.blue = FbBitfield {
                    offset: 16,
                    length: 8,
                    msb_right: 0,
                };
                var_info.transp = FbBitfield {
                    offset: 24,
                    length: 0,
                    msb_right: 0,
                };
            }
            super::PixelFormat::XBGR8888 => {
                var_info.blue = FbBitfield {
                    offset: 0,
                    length: 8,
                    msb_right: 0,
                };
                var_info.green = FbBitfield {
                    offset: 8,
                    length: 8,
                    msb_right: 0,
                };
                var_info.red = FbBitfield {
                    offset: 16,
                    length: 8,
                    msb_right: 0,
                };
                var_info.transp = FbBitfield {
                    offset: 24,
                    length: 0,
                    msb_right: 0,
                };
            }
            super::PixelFormat::XRGB2101010 => {
                var_info.red = FbBitfield {
                    offset: 20,
                    length: 10,
                    msb_right: 0,
                };
                var_info.green = FbBitfield {
                    offset: 10,
                    length: 10,
                    msb_right: 0,
                };
                var_info.blue = FbBitfield {
                    offset: 0,
                    length: 10,
                    msb_right: 0,
                };
                var_info.transp = FbBitfield {
                    offset: 30,
                    length: 0,
                    msb_right: 0,
                };
            }
            super::PixelFormat::RGB888 => {
                var_info.red = FbBitfield {
                    offset: 0,
                    length: 8,
                    msb_right: 0,
                };
                var_info.green = FbBitfield {
                    offset: 8,
                    length: 8,
                    msb_right: 0,
                };
                var_info.blue = FbBitfield {
                    offset: 16,
                    length: 8,
                    msb_right: 0,
                };
                var_info.transp = FbBitfield {
                    offset: 0,
                    length: 0,
                    msb_right: 0,
                };
            }
            super::PixelFormat::RGB565 => {
                var_info.red = FbBitfield {
                    offset: 11,
                    length: 5,
                    msb_right: 0,
                };
                var_info.green = FbBitfield {
                    offset: 5,
                    length: 6,
                    msb_right: 0,
                };
                var_info.blue = FbBitfield {
                    offset: 0,
                    length: 5,
                    msb_right: 0,
                };
                var_info.transp = FbBitfield {
                    offset: 0,
                    length: 0,
                    msb_right: 0,
                };
            }
            super::PixelFormat::ARGB1555 => {
                var_info.red = FbBitfield {
                    offset: 10,
                    length: 5,
                    msb_right: 0,
                };
                var_info.green = FbBitfield {
                    offset: 5,
                    length: 5,
                    msb_right: 0,
                };
                var_info.blue = FbBitfield {
                    offset: 0,
                    length: 5,
                    msb_right: 0,
                };
                var_info.transp = FbBitfield {
                    offset: 15,
                    length: 1,
                    msb_right: 0,
                };
            }
            super::PixelFormat::XRGB1555 => {
                var_info.red = FbBitfield {
                    offset: 10,
                    length: 5,
                    msb_right: 0,
                };
                var_info.green = FbBitfield {
                    offset: 5,
                    length: 5,
                    msb_right: 0,
                };
                var_info.blue = FbBitfield {
                    offset: 0,
                    length: 5,
                    msb_right: 0,
                };
                var_info.transp = FbBitfield {
                    offset: 15,
                    length: 0,
                    msb_right: 0,
                };
            }
        }

        var_info
    }

    /// Handle FBIOGET_VSCREENINFO control command
    fn handle_get_vscreeninfo(&self, arg: usize) -> Result<i32, &'static str> {
        if arg == 0 {
            return Err("Invalid argument pointer");
        }

        // Try to get current task for user pointer translation
        // If no task (kernel context), use pointer directly
        let target_ptr = if let Some(current_task) = crate::task::mytask() {
            // User space: translate virtual address to physical
            current_task
                .vm_manager
                .translate_to_kva(arg)
                .ok_or("Invalid user pointer - not mapped")?
        } else {
            // Kernel space: use pointer directly
            arg
        };

        let var_info = self.current_var_info();

        // Safely copy to user space using translated physical address
        unsafe {
            let user_ptr = target_ptr as *mut FbVarScreenInfo;
            core::ptr::write(user_ptr, var_info);
        }

        Ok(0) // Success
    }

    /// Handle FBIOGET_FSCREENINFO control command
    fn handle_get_fscreeninfo(&self, arg: usize) -> Result<i32, &'static str> {
        if arg == 0 {
            return Err("Invalid argument pointer");
        }

        // Try to get current task for user pointer translation
        // If no task (kernel context), use pointer directly
        let target_ptr = if let Some(current_task) = crate::task::mytask() {
            // User space: translate virtual address to physical
            current_task
                .vm_manager
                .translate_to_kva(arg)
                .ok_or("Invalid user pointer - not mapped")?
        } else {
            // Kernel space: use pointer directly
            arg
        };

        let info = self.current_framebuffer_info()?;
        let config = &info.config;

        // Create fixed screen info structure
        let mut fix_info = FbFixScreenInfo::default();

        // Set identification string
        let fb_name = self.fb_resource.logical_name.as_bytes();
        let copy_len = fb_name.len().min(fix_info.id.len() - 1);
        fix_info.id[..copy_len].copy_from_slice(&fb_name[..copy_len]);

        fix_info.smem_start = info.physical_addr;
        fix_info.smem_len = info.size as u32;
        fix_info.line_length = config.stride;
        fix_info.type_ = 0; // FB_TYPE_PACKED_PIXELS
        fix_info.visual = 2; // FB_VISUAL_TRUECOLOR

        // Safely copy to user space using translated physical address
        unsafe {
            let user_ptr = target_ptr as *mut FbFixScreenInfo;
            core::ptr::write(user_ptr, fix_info);
        }

        Ok(0) // Success
    }

    /// Handle FBIO_FLUSH control command
    ///
    /// This command forces any pending framebuffer changes to be displayed.
    /// For memory-mapped framebuffers, this typically involves ensuring
    /// CPU caches are flushed and any display controller updates are triggered.
    fn handle_flush(&self, _arg: usize) -> Result<i32, &'static str> {
        let info = self.current_framebuffer_info()?;

        // Check if framebuffer address is valid
        if info.physical_addr == 0 {
            return Err("Invalid framebuffer address");
        }

        // Flush the CPU cache for the framebuffer memory
        // In a real implementation, this would ensure that any writes to the framebuffer
        // are visible to the display controller.
        // TODO: Implement actual cache flushing logic

        // Trigger display controller update if needed
        // For some hardware, writing to framebuffer memory doesn't immediately update the display
        self.trigger_display_update(DisplayRegion::full(&info.config))?;
        self.disarm_legacy_dirty_timer();
        Self::write_protect_mappings(&self.mappings, &info);

        Ok(0) // Success
    }

    /// Trigger display controller update
    ///
    /// Some display controllers require explicit commands to update the display
    /// from framebuffer contents. This method handles such updates.
    fn trigger_display_update(&self, region: DisplayRegion) -> Result<(), &'static str> {
        // Try to get the source graphics device to trigger a display update
        let device_manager = self.device_manager();
        if let Some(device) = device_manager.get_device(self.fb_resource.source_device_id) {
            // Check if the device supports graphics operations
            if let Some(graphics_device) = device.as_graphics_device() {
                let (config, physical_addr) = graphics_device.get_framebuffer_info()?;

                let region = DisplayRegion::new(
                    region.x.min(config.width),
                    region.y.min(config.height),
                    region.width.min(config.width.saturating_sub(region.x)),
                    region.height.min(config.height.saturating_sub(region.y)),
                );
                graphics_device.present_current_framebuffer_region(region)?;

                // Verify that the framebuffer address is still valid
                if physical_addr == 0 {
                    return Err("Graphics device framebuffer address is null");
                }
            }
        }

        // For virtualized environments (like QEMU), framebuffer writes are often
        // automatically reflected on the display, so no additional action is needed

        Ok(())
    }

    /// Handle FBIOPUT_VSCREENINFO control command  
    fn handle_put_vscreeninfo(&self, arg: usize) -> Result<i32, &'static str> {
        // Basic validation of user pointer
        if arg == 0 {
            return Ok(-22); // EINVAL
        }

        // Translate user-space pointer if available
        let target_ptr = if let Some(current_task) = crate::task::mytask() {
            match current_task.vm_manager.translate_to_kva(arg) {
                Some(p) => p,
                None => return Ok(-14), // EFAULT
            }
        } else {
            arg
        } as *mut FbVarScreenInfo;

        // Read the requested settings (not strictly required, but useful for future validation)
        let _req = unsafe { core::ptr::read(target_ptr as *const FbVarScreenInfo) };

        // We currently do not support mode switching. Acknowledge the request
        // and write back the actually supported mode so userland can proceed.
        let accepted = self.current_var_info();
        unsafe {
            core::ptr::write(target_ptr, accepted);
        }

        Ok(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::{
        Device,
        graphics::{
            FramebufferConfig, GenericGraphicsDevice, PixelFormat, manager::GraphicsManager,
        },
    };
    use alloc::{string::ToString, sync::Arc};
    use spin::RwLock;

    /// Test utility to create isolated managers for each test.
    fn setup_test_managers() -> (GraphicsManager, DeviceManager) {
        (GraphicsManager::new(), DeviceManager::new_for_test())
    }

    #[test_case]
    fn test_framebuffer_char_device_creation() {
        // Create test framebuffer resource
        let config = FramebufferConfig::new(1024, 768, PixelFormat::RGBA8888);
        let fb_resource = Arc::new(FramebufferResource::new(
            0,
            "fb0".to_string(),
            config,
            0x80000000,
            1024 * 768 * 4,
        ));

        let device = FramebufferCharDevice::new(fb_resource);
        assert_eq!(device.get_framebuffer_name(), "fb0");
        assert_eq!(device.device_type(), DeviceType::Char);
        assert_eq!(device.name(), "framebuffer");
    }

    #[test_case]
    fn test_framebuffer_char_device_read_write_at() {
        let (graphics_manager, device_manager) = setup_test_managers();
        let mut test_device = GenericGraphicsDevice::new("test-gpu-read-write");
        let config = FramebufferConfig::new(100, 100, PixelFormat::RGBA8888);
        test_device.set_framebuffer_config(config.clone());

        // Allocate memory for test framebuffer
        let fb_size = config.size();
        let fb_pages = (fb_size + 4095) / 4096;
        let fb_addr = crate::mem::page::allocate_raw_pages(fb_pages) as usize;
        test_device.set_framebuffer_address(crate::vm::addr::virt_to_phys(fb_addr));

        let shared_device: Arc<dyn Device> = Arc::new(test_device);

        // Register device with DeviceManager first
        let device_id = device_manager
            .register_device_with_name("test-gpu-read-write".to_string(), shared_device.clone());

        // Then register with GraphicsManager
        graphics_manager
            .register_framebuffer_from_device_with_device_manager(
                device_id,
                shared_device,
                &device_manager,
            )
            .unwrap();

        // Get the framebuffer resource that was assigned to this specific device
        let fb_resource = {
            let fb_names = graphics_manager.get_framebuffer_names();
            let fb_name = fb_names
                .iter()
                .find(|name| {
                    if let Some(fb_resource) = graphics_manager.get_framebuffer(name) {
                        fb_resource.source_device_id == device_id
                    } else {
                        false
                    }
                })
                .expect("Should have framebuffer for this device");
            graphics_manager
                .get_framebuffer(fb_name)
                .expect("Framebuffer should exist")
        };
        let char_device =
            FramebufferCharDevice::new_with_device_manager(fb_resource, &device_manager);

        // Test write_at operation
        let test_data = [0x12, 0x34, 0x56, 0x78];
        let written = char_device.write_at(0, &test_data).unwrap();
        assert_eq!(written, 4);

        // Test read_at operation
        let mut read_buffer = [0u8; 4];
        let read_count = char_device.read_at(0, &mut read_buffer).unwrap();
        assert_eq!(read_count, 4);
        assert_eq!(read_buffer, test_data);
    }

    #[test_case]
    fn test_framebuffer_char_device_boundaries() {
        let (graphics_manager, device_manager) = setup_test_managers();
        let mut test_device = GenericGraphicsDevice::new("test-gpu-boundaries");
        let config = FramebufferConfig::new(10, 10, PixelFormat::RGB888); // Small 10x10 framebuffer
        test_device.set_framebuffer_config(config.clone());

        let fb_size = config.size(); // 10 * 10 * 3 = 300 bytes
        let fb_pages = (fb_size + 4095) / 4096;
        let fb_addr = crate::mem::page::allocate_raw_pages(fb_pages) as usize;
        test_device.set_framebuffer_address(crate::vm::addr::virt_to_phys(fb_addr));

        let shared_device: Arc<dyn Device> = Arc::new(test_device);
        let device_id = device_manager
            .register_device_with_name("test-gpu-boundaries".to_string(), shared_device.clone());
        graphics_manager
            .register_framebuffer_from_device_with_device_manager(
                device_id,
                shared_device,
                &device_manager,
            )
            .unwrap();

        let fb_resource = {
            let fb_names = graphics_manager.get_framebuffer_names();
            let fb_name = fb_names
                .iter()
                .find(|name| {
                    if let Some(fb_resource) = graphics_manager.get_framebuffer(name) {
                        fb_resource.source_device_id == device_id
                    } else {
                        false
                    }
                })
                .expect("Should have framebuffer for this device");
            graphics_manager
                .get_framebuffer(fb_name)
                .expect("Framebuffer should exist")
        };
        let char_device =
            FramebufferCharDevice::new_with_device_manager(fb_resource, &device_manager);

        // First, clear the framebuffer by writing zeros
        let zero_buffer = vec![0u8; fb_size];
        assert_eq!(char_device.write_at(0, &zero_buffer).unwrap(), fb_size);

        // Test writing at the start
        let start_data = [0xFF, 0x00, 0xFF];
        assert_eq!(char_device.write_at(0, &start_data).unwrap(), 3);

        // Test writing at the end (non-overlapping with partial write test)
        let end_data = [0x00, 0xFF, 0x00];
        assert_eq!(
            char_device
                .write_at((fb_size - 6) as u64, &end_data)
                .unwrap(),
            3
        );

        // Test writing beyond boundaries (should fail or write partial)
        let beyond_data = [0xAA, 0xBB, 0xCC, 0xDD];
        let result = char_device.write_at(fb_size as u64, &beyond_data);
        assert!(result.is_err() || result.unwrap() == 0);

        // Test partial write at boundary (this will overwrite the last 2 bytes)
        let partial_data = [0x11, 0x22, 0x33, 0x44, 0x55];
        let written = char_device
            .write_at((fb_size - 2) as u64, &partial_data)
            .unwrap();
        assert_eq!(written, 2); // Should only write 2 bytes that fit

        // Verify reads
        let mut read_start = [0u8; 3];
        assert_eq!(char_device.read_at(0, &mut read_start).unwrap(), 3);
        assert_eq!(read_start, start_data);

        let mut read_end = [0u8; 3];
        assert_eq!(
            char_device
                .read_at((fb_size - 6) as u64, &mut read_end)
                .unwrap(),
            3
        );
        assert_eq!(read_end, end_data);

        // Verify the partial write at the very end
        let mut read_partial = [0u8; 2];
        assert_eq!(
            char_device
                .read_at((fb_size - 2) as u64, &mut read_partial)
                .unwrap(),
            2
        );
        assert_eq!(read_partial, [0x11, 0x22]);
    }

    #[test_case]
    fn test_framebuffer_char_device_pixel_formats() {
        for (pixel_format, expected_bpp) in [
            (PixelFormat::RGB565, 2),
            (PixelFormat::RGB888, 3),
            (PixelFormat::RGBA8888, 4),
            (PixelFormat::BGRA8888, 4),
        ] {
            let (graphics_manager, device_manager) = setup_test_managers();
            let mut test_device = GenericGraphicsDevice::new("test-gpu-pixel-format");
            let config = FramebufferConfig::new(4, 4, pixel_format); // 4x4 pixels
            test_device.set_framebuffer_config(config.clone());

            let fb_size = config.size();
            let expected_size = 4 * 4 * expected_bpp;
            assert_eq!(fb_size, expected_size);

            let fb_pages = (fb_size + 4095) / 4096;
            let fb_addr = crate::mem::page::allocate_raw_pages(fb_pages) as usize;
            test_device.set_framebuffer_address(crate::vm::addr::virt_to_phys(fb_addr));

            let shared_device: Arc<dyn Device> = Arc::new(test_device);
            let device_id = device_manager.register_device_with_name(
                alloc::format!("test-gpu-{:?}", pixel_format),
                shared_device.clone(),
            );
            graphics_manager
                .register_framebuffer_from_device_with_device_manager(
                    device_id,
                    shared_device,
                    &device_manager,
                )
                .unwrap();

            let fb_resource = {
                let fb_names = graphics_manager.get_framebuffer_names();
                let fb_name = fb_names
                    .iter()
                    .find(|name| {
                        if let Some(fb_resource) = graphics_manager.get_framebuffer(name) {
                            fb_resource.source_device_id == device_id
                        } else {
                            false
                        }
                    })
                    .expect("Should have framebuffer for this device");
                graphics_manager
                    .get_framebuffer(fb_name)
                    .expect("Framebuffer should exist")
            };
            let char_device =
                FramebufferCharDevice::new_with_device_manager(fb_resource, &device_manager);

            // Test writing a single pixel
            let pixel_data = match expected_bpp {
                2 => vec![0xFF, 0x00],             // RGB565
                3 => vec![0xFF, 0x00, 0xFF],       // RGB888
                4 => vec![0xFF, 0x00, 0xFF, 0x80], // RGBA8888/BGRA8888
                _ => unreachable!(),
            };

            assert_eq!(char_device.write_at(0, &pixel_data).unwrap(), expected_bpp);

            // Test reading the pixel back
            let mut read_pixel = vec![0u8; expected_bpp];
            assert_eq!(
                char_device.read_at(0, &mut read_pixel).unwrap(),
                expected_bpp
            );
            assert_eq!(read_pixel, pixel_data);
        }
    }

    #[test_case]
    fn test_framebuffer_char_device_capabilities() {
        // Test with valid framebuffer
        let (graphics_manager, device_manager) = setup_test_managers();
        let mut test_device = GenericGraphicsDevice::new("test-gpu-caps");
        let config = FramebufferConfig::new(100, 100, PixelFormat::RGBA8888);
        test_device.set_framebuffer_config(config.clone());

        let fb_size = config.size();
        let fb_pages = (fb_size + 4095) / 4096;
        let fb_addr = crate::mem::page::allocate_raw_pages(fb_pages) as usize;
        test_device.set_framebuffer_address(crate::vm::addr::virt_to_phys(fb_addr));

        let shared_device: Arc<dyn Device> = Arc::new(test_device);
        let device_id = device_manager
            .register_device_with_name("test-gpu-caps".to_string(), shared_device.clone());
        graphics_manager
            .register_framebuffer_from_device_with_device_manager(
                device_id,
                shared_device,
                &device_manager,
            )
            .unwrap();

        let fb_resource = {
            let fb_names = graphics_manager.get_framebuffer_names();
            let fb_name = fb_names
                .iter()
                .find(|name| {
                    if let Some(fb_resource) = graphics_manager.get_framebuffer(name) {
                        fb_resource.source_device_id == device_id
                    } else {
                        false
                    }
                })
                .expect("Should have framebuffer for this device");
            graphics_manager
                .get_framebuffer(fb_name)
                .expect("Framebuffer should exist")
        };
        let char_device =
            FramebufferCharDevice::new_with_device_manager(fb_resource, &device_manager);

        // Test capabilities
        assert!(char_device.can_read());
        assert!(char_device.can_write());
        assert_eq!(char_device.device_type(), DeviceType::Char);
        assert_eq!(char_device.name(), "framebuffer");

        // Test with invalid framebuffer (zero address)
        let invalid_config = FramebufferConfig::new(10, 10, PixelFormat::RGB888);
        let invalid_resource = Arc::new(FramebufferResource {
            source_device_id: 999,
            logical_name: "invalid".to_string(),
            config: invalid_config,
            physical_addr: 0, // Invalid address
            size: 300,
            created_char_device_id: RwLock::new(None),
            created_display_device_id: RwLock::new(None),
        });
        let invalid_device = FramebufferCharDevice::new(invalid_resource);

        assert!(!invalid_device.can_read());
        assert!(!invalid_device.can_write());
    }

    #[test_case]
    fn test_framebuffer_char_device_unsupported_methods() {
        let (graphics_manager, device_manager) = setup_test_managers();
        let mut test_device = GenericGraphicsDevice::new("test-gpu-unsupported");
        let config = FramebufferConfig::new(10, 10, PixelFormat::RGB888);
        test_device.set_framebuffer_config(config.clone());

        let fb_size = config.size();
        let fb_pages = (fb_size + 4095) / 4096;
        let fb_addr = crate::mem::page::allocate_raw_pages(fb_pages) as usize;
        test_device.set_framebuffer_address(crate::vm::addr::virt_to_phys(fb_addr));

        let shared_device: Arc<dyn Device> = Arc::new(test_device);
        let device_id = device_manager
            .register_device_with_name("test-gpu-unsupported".to_string(), shared_device.clone());
        graphics_manager
            .register_framebuffer_from_device_with_device_manager(
                device_id,
                shared_device,
                &device_manager,
            )
            .unwrap();

        let fb_resource = {
            let fb_names = graphics_manager.get_framebuffer_names();
            let fb_name = fb_names
                .iter()
                .find(|name| {
                    if let Some(fb_resource) = graphics_manager.get_framebuffer(name) {
                        fb_resource.source_device_id == device_id
                    } else {
                        false
                    }
                })
                .expect("Should have framebuffer for this device");
            graphics_manager
                .get_framebuffer(fb_name)
                .expect("Framebuffer should exist")
        };
        let char_device =
            FramebufferCharDevice::new_with_device_manager(fb_resource, &device_manager);

        // Test that read_byte returns None (unsupported)
        assert_eq!(char_device.read_byte(), None);

        // Test that write_byte returns error (unsupported)
        let result = char_device.write_byte(0xFF);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not supported"));
    }

    #[test_case]
    fn test_framebuffer_char_device_large_operations() {
        let (graphics_manager, device_manager) = setup_test_managers();
        let mut test_device = GenericGraphicsDevice::new("test-gpu-large");
        let config = FramebufferConfig::new(256, 256, PixelFormat::RGBA8888);
        test_device.set_framebuffer_config(config.clone());

        let fb_size = config.size(); // 256 * 256 * 4 = 262,144 bytes
        let fb_pages = (fb_size + 4095) / 4096;
        let fb_addr = crate::mem::page::allocate_raw_pages(fb_pages) as usize;
        test_device.set_framebuffer_address(crate::vm::addr::virt_to_phys(fb_addr));

        let shared_device: Arc<dyn Device> = Arc::new(test_device);
        let device_id = device_manager
            .register_device_with_name("test-gpu-large".to_string(), shared_device.clone());
        graphics_manager
            .register_framebuffer_from_device_with_device_manager(
                device_id,
                shared_device,
                &device_manager,
            )
            .unwrap();

        let fb_resource = {
            let fb_names = graphics_manager.get_framebuffer_names();
            let fb_name = fb_names
                .iter()
                .find(|name| {
                    if let Some(fb_resource) = graphics_manager.get_framebuffer(name) {
                        fb_resource.source_device_id == device_id
                    } else {
                        false
                    }
                })
                .expect("Should have framebuffer for this device");
            graphics_manager
                .get_framebuffer(fb_name)
                .expect("Framebuffer should exist")
        };
        let char_device =
            FramebufferCharDevice::new_with_device_manager(fb_resource, &device_manager);

        // Test large write operation
        let large_data = vec![0x55u8; 4096]; // 4KB
        let written = char_device.write_at(0, &large_data).unwrap();
        assert_eq!(written, 4096);

        // Test large read operation
        let mut read_buffer = vec![0u8; 4096];
        let read_count = char_device.read_at(0, &mut read_buffer).unwrap();
        assert_eq!(read_count, 4096);
        assert_eq!(read_buffer, large_data);

        // Test writing across page boundaries
        let cross_page_data = vec![0xAAu8; 8192]; // 8KB
        let written = char_device.write_at(2048, &cross_page_data).unwrap();
        assert_eq!(written, 8192);

        // Test reading across page boundaries
        let mut cross_read_buffer = vec![0u8; 8192];
        let read_count = char_device.read_at(2048, &mut cross_read_buffer).unwrap();
        assert_eq!(read_count, 8192);
        assert_eq!(cross_read_buffer, cross_page_data);
    }

    #[test_case]
    fn test_framebuffer_char_device_pattern_operations() {
        let (graphics_manager, device_manager) = setup_test_managers();
        let mut test_device = GenericGraphicsDevice::new("test-gpu-pattern");
        let config = FramebufferConfig::new(16, 16, PixelFormat::RGB888);
        test_device.set_framebuffer_config(config.clone());

        let fb_size = config.size(); // 16 * 16 * 3 = 768 bytes
        let fb_pages = (fb_size + 4095) / 4096;
        let fb_addr = crate::mem::page::allocate_raw_pages(fb_pages) as usize;
        test_device.set_framebuffer_address(crate::vm::addr::virt_to_phys(fb_addr));

        let shared_device: Arc<dyn Device> = Arc::new(test_device);
        let device_id = device_manager
            .register_device_with_name("test-gpu-pattern".to_string(), shared_device.clone());
        graphics_manager
            .register_framebuffer_from_device_with_device_manager(
                device_id,
                shared_device,
                &device_manager,
            )
            .unwrap();

        let fb_resource = {
            let fb_names = graphics_manager.get_framebuffer_names();
            let fb_name = fb_names
                .iter()
                .find(|name| {
                    if let Some(fb_resource) = graphics_manager.get_framebuffer(name) {
                        fb_resource.source_device_id == device_id
                    } else {
                        false
                    }
                })
                .expect("Should have framebuffer for this device");
            graphics_manager
                .get_framebuffer(fb_name)
                .expect("Framebuffer should exist")
        };
        let char_device =
            FramebufferCharDevice::new_with_device_manager(fb_resource, &device_manager);

        // Test checkerboard pattern
        for y in 0..16 {
            for x in 0..16 {
                let pixel_offset = (y * 16 + x) * 3;
                let color = if (x + y) % 2 == 0 {
                    [0xFF, 0x00, 0x00] // Red
                } else {
                    [0x00, 0xFF, 0x00] // Green
                };
                assert_eq!(
                    char_device.write_at(pixel_offset as u64, &color).unwrap(),
                    3
                );
            }
        }

        // Verify checkerboard pattern
        for y in 0..16 {
            for x in 0..16 {
                let pixel_offset = (y * 16 + x) * 3;
                let mut read_color = [0u8; 3];
                assert_eq!(
                    char_device
                        .read_at(pixel_offset as u64, &mut read_color)
                        .unwrap(),
                    3
                );

                let expected_color = if (x + y) % 2 == 0 {
                    [0xFF, 0x00, 0x00] // Red
                } else {
                    [0x00, 0xFF, 0x00] // Green
                };
                assert_eq!(read_color, expected_color);
            }
        }
    }

    #[test_case]
    fn test_framebuffer_memory_mapping_ops() {
        use crate::object::capability::MemoryMappingOps;

        let (graphics_manager, device_manager) = setup_test_managers();
        let mut test_device = GenericGraphicsDevice::new("test-mmap-ops");
        let config = FramebufferConfig::new(4, 4, PixelFormat::RGBA8888);
        test_device.set_framebuffer_config(config.clone());

        let fb_size = config.size();
        let fb_pages = (fb_size + 4095) / 4096;
        let fb_addr = crate::mem::page::allocate_raw_pages(fb_pages) as usize;
        test_device.set_framebuffer_address(crate::vm::addr::virt_to_phys(fb_addr));

        let shared_device: Arc<dyn Device> = Arc::new(test_device);
        let device_id = device_manager
            .register_device_with_name("test-mmap-ops-device".to_string(), shared_device.clone());
        graphics_manager
            .register_framebuffer_from_device_with_device_manager(
                device_id,
                shared_device,
                &device_manager,
            )
            .unwrap();

        let fb_resource = {
            let fb_names = graphics_manager.get_framebuffer_names();
            let fb_name = fb_names
                .iter()
                .find(|name| {
                    if let Some(fb_resource) = graphics_manager.get_framebuffer(name) {
                        fb_resource.source_device_id == device_id
                    } else {
                        false
                    }
                })
                .expect("Should have framebuffer for this device");
            graphics_manager
                .get_framebuffer(fb_name)
                .expect("Framebuffer should exist")
        };

        let fb_device =
            FramebufferCharDevice::new_with_device_manager(fb_resource.clone(), &device_manager);

        // Test supports_mmap
        assert!(fb_device.supports_mmap());

        // Test get_mapping_info
        let result = fb_device.get_mapping_info(0, fb_pages * 4096);
        assert!(result.is_ok());
        let (paddr, permissions, is_shared) = result.unwrap();
        assert_eq!(paddr, crate::vm::addr::virt_to_phys(fb_addr));
        assert_eq!(permissions, 0x3); // Read and Write
        assert!(is_shared);

        // Test invalid offset
        let result = fb_device.get_mapping_info(fb_size + 1, 32);
        assert!(result.is_err());

        // Test invalid length
        let result = fb_device.get_mapping_info(0, fb_size + 1);
        assert!(result.is_err());

        // Test on_mapped callback
        fb_device.on_mapped(0x1000, paddr, fb_pages * 4096, 0);
        {
            let mappings = fb_device.mappings.read();
            assert_eq!(mappings.len(), 1);
            assert!(mappings.contains_key(&(0, 0x1000)));
        }

        // Test on_unmapped callback
        fb_device.on_unmapped(0x1000, fb_pages * 4096);
        {
            let mappings = fb_device.mappings.read();
            assert_eq!(mappings.len(), 0);
        }
    }

    #[test_case]
    fn test_framebuffer_mapping_basic_ops() {
        let (graphics_manager, device_manager) = setup_test_managers();
        let mut test_device = GenericGraphicsDevice::new("test-mapping-basic");
        let config = FramebufferConfig::new(4, 4, PixelFormat::RGBA8888);
        test_device.set_framebuffer_config(config.clone());

        let fb_size = config.size();
        let fb_pages = (fb_size + 4095) / 4096;
        let fb_addr = crate::mem::page::allocate_raw_pages(fb_pages) as usize;
        test_device.set_framebuffer_address(crate::vm::addr::virt_to_phys(fb_addr));

        let shared_device: Arc<dyn Device> = Arc::new(test_device);
        let device_id = device_manager.register_device_with_name(
            "test-mapping-basic-device".to_string(),
            shared_device.clone(),
        );
        graphics_manager
            .register_framebuffer_from_device_with_device_manager(
                device_id,
                shared_device,
                &device_manager,
            )
            .unwrap();

        let fb_resource = {
            let fb_names = graphics_manager.get_framebuffer_names();
            let fb_name = fb_names
                .iter()
                .find(|name| {
                    if let Some(fb_resource) = graphics_manager.get_framebuffer(name) {
                        fb_resource.source_device_id == device_id
                    } else {
                        false
                    }
                })
                .expect("Should have framebuffer for this device");
            graphics_manager
                .get_framebuffer(fb_name)
                .expect("Framebuffer should exist")
        };

        let char_device =
            FramebufferCharDevice::new_with_device_manager(fb_resource, &device_manager);

        // Test that mappings are initially empty
        {
            let mappings = char_device.mappings.read();
            assert_eq!(mappings.len(), 0);
        }

        // Test supports_mmap
        assert!(char_device.supports_mmap());

        // Test get_mapping_info with valid parameters
        let result = char_device.get_mapping_info(0, fb_pages * 4096);
        assert!(result.is_ok());
        let (paddr, permissions, is_shared) = result.unwrap();
        assert_eq!(paddr, crate::vm::addr::virt_to_phys(fb_addr));
        assert_eq!(permissions, 0x3);
        assert!(is_shared);
    }
}
