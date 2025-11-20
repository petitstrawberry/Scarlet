//! Scarlet Graphics Control Commands
//!
//! This module defines OS-independent control commands for graphics operations.
//! These commands can be used by OS-specific ABI layers (like Linux DRM) to
//! perform graphics operations without directly accessing devices.
//!
//! Similar to tty control commands (SCTL_TTY_*), these provide an abstraction
//! layer between OS ABIs and Scarlet's graphics subsystem.

/// Scarlet Graphics Control Commands
/// 
/// Command format: 0x5347_XXXX (SG = Scarlet Graphics)
pub mod commands {
    /// Get framebuffer configuration by device ID
    /// arg: device_id (usize)
    /// returns: packed config (width << 32 | height)
    pub const SCTL_GFX_GET_CONFIG: u32 = 0x5347_0001;
    
    /// Get framebuffer address by device ID
    /// arg: device_id (usize)
    /// returns: physical address (usize)
    pub const SCTL_GFX_GET_ADDRESS: u32 = 0x5347_0002;
    
    /// Flush framebuffer region by device ID
    /// arg: packed flush params (device_id, x, y, width, height)
    /// returns: 0 on success
    pub const SCTL_GFX_FLUSH: u32 = 0x5347_0003;
    
    /// Get pixel format by device ID
    /// arg: device_id (usize)
    /// returns: pixel format code
    pub const SCTL_GFX_GET_FORMAT: u32 = 0x5347_0004;
    
    /// Get framebuffer size (in bytes) by device ID
    /// arg: device_id (usize)
    /// returns: size in bytes
    pub const SCTL_GFX_GET_SIZE: u32 = 0x5347_0005;
    
    /// Get device ID by framebuffer name
    /// arg: pointer to framebuffer name string
    /// returns: device_id or -1 on error
    pub const SCTL_GFX_GET_DEVICE_ID: u32 = 0x5347_0006;
    
    /// Get framebuffer count
    /// arg: unused
    /// returns: number of framebuffers
    pub const SCTL_GFX_GET_FB_COUNT: u32 = 0x5347_0007;
}

/// Flush parameters structure
/// Used to pack flush operation parameters
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct FlushParams {
    pub device_id: usize,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl FlushParams {
    /// Pack flush parameters for passing to control command
    pub fn pack(&self) -> usize {
        // Store pointer to self
        self as *const FlushParams as usize
    }
    
    /// Unpack flush parameters from control command argument
    pub unsafe fn unpack(arg: usize) -> &'static Self {
        &*(arg as *const FlushParams)
    }
}
