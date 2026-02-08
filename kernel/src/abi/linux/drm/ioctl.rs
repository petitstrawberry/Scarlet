//! Linux DRM (Direct Rendering Manager) ioctl definitions
//!
//! This module provides Linux-compatible DRM ioctl structures and constants
//! for supporting graphics applications that expect standard DRM interfaces.

// DRM ioctl command constants
pub mod commands {
    // DRM base ioctl type
    pub const DRM_IOCTL_BASE: u8 = 0x64;

    // DRM version
    pub const DRM_IOCTL_VERSION: u32 = 0x6400;

    // Mode resources
    pub const DRM_IOCTL_MODE_GETRESOURCES: u32 = 0x64A0;
    pub const DRM_IOCTL_MODE_GETCRTC: u32 = 0x64A1;
    pub const DRM_IOCTL_MODE_SETCRTC: u32 = 0x64A2;

    // Dumb buffer operations
    pub const DRM_IOCTL_MODE_CREATE_DUMB: u32 = 0x64B2;
    pub const DRM_IOCTL_MODE_MAP_DUMB: u32 = 0x64B3;
    pub const DRM_IOCTL_MODE_DESTROY_DUMB: u32 = 0x64B4;

    // Page flip
    pub const DRM_IOCTL_MODE_PAGE_FLIP: u32 = 0x64B0;
}

/// DRM version structure
#[repr(C)]
#[derive(Debug, Clone)]
pub struct DrmVersion {
    pub version_major: i32,
    pub version_minor: i32,
    pub version_patchlevel: i32,
    pub name_len: usize,
    pub name: u64, // Pointer to char buffer
    pub date_len: usize,
    pub date: u64, // Pointer to char buffer
    pub desc_len: usize,
    pub desc: u64, // Pointer to char buffer
}

/// DRM mode resources
#[repr(C)]
#[derive(Debug, Clone)]
pub struct DrmModeCardRes {
    pub fb_id_ptr: u64,
    pub crtc_id_ptr: u64,
    pub connector_id_ptr: u64,
    pub encoder_id_ptr: u64,
    pub count_fbs: u32,
    pub count_crtcs: u32,
    pub count_connectors: u32,
    pub count_encoders: u32,
    pub min_width: u32,
    pub max_width: u32,
    pub min_height: u32,
    pub max_height: u32,
}

/// DRM mode CRTC (display controller)
#[repr(C)]
#[derive(Debug, Clone)]
pub struct DrmModeCrtc {
    pub set_connectors_ptr: u64,
    pub count_connectors: u32,
    pub crtc_id: u32,
    pub fb_id: u32,
    pub x: u32,
    pub y: u32,
    pub gamma_size: u32,
    pub mode_valid: u32,
    pub mode: DrmModeInfo,
}

/// DRM mode information
#[repr(C)]
#[derive(Debug, Clone)]
pub struct DrmModeInfo {
    pub clock: u32,
    pub hdisplay: u16,
    pub hsync_start: u16,
    pub hsync_end: u16,
    pub htotal: u16,
    pub hskew: u16,
    pub vdisplay: u16,
    pub vsync_start: u16,
    pub vsync_end: u16,
    pub vtotal: u16,
    pub vscan: u16,
    pub vrefresh: u32,
    pub flags: u32,
    pub type_: u32,
    pub name: [u8; 32],
}

impl DrmModeInfo {
    pub fn new() -> Self {
        Self {
            clock: 0,
            hdisplay: 0,
            hsync_start: 0,
            hsync_end: 0,
            htotal: 0,
            hskew: 0,
            vdisplay: 0,
            vsync_start: 0,
            vsync_end: 0,
            vtotal: 0,
            vscan: 0,
            vrefresh: 60,
            flags: 0,
            type_: 0,
            name: [0; 32],
        }
    }
}

impl Default for DrmModeInfo {
    fn default() -> Self {
        Self::new()
    }
}

/// DRM dumb buffer creation request
#[repr(C)]
#[derive(Debug, Clone)]
pub struct DrmModeCreateDumb {
    pub height: u32,
    pub width: u32,
    pub bpp: u32,
    pub flags: u32,
    pub handle: u32,
    pub pitch: u32,
    pub size: u64,
}

/// DRM dumb buffer mapping request
#[repr(C)]
#[derive(Debug, Clone)]
pub struct DrmModeMapDumb {
    pub handle: u32,
    pub pad: u32,
    pub offset: u64,
}

/// DRM dumb buffer destruction request
#[repr(C)]
#[derive(Debug, Clone)]
pub struct DrmModeDestroyDumb {
    pub handle: u32,
}

/// DRM page flip request
#[repr(C)]
#[derive(Debug, Clone)]
pub struct DrmModePageFlip {
    pub crtc_id: u32,
    pub fb_id: u32,
    pub flags: u32,
    pub reserved: u32,
    pub user_data: u64,
}

// DRM flags
pub const DRM_MODE_PAGE_FLIP_EVENT: u32 = 0x01;
pub const DRM_MODE_PAGE_FLIP_ASYNC: u32 = 0x02;

// DRM mode types
pub const DRM_MODE_TYPE_BUILTIN: u32 = 1 << 0;
pub const DRM_MODE_TYPE_CLOCK_C: u32 = 1 << 1;
pub const DRM_MODE_TYPE_CRTC_C: u32 = 1 << 2;
pub const DRM_MODE_TYPE_PREFERRED: u32 = 1 << 3;
pub const DRM_MODE_TYPE_DEFAULT: u32 = 1 << 4;
pub const DRM_MODE_TYPE_USERDEF: u32 = 1 << 5;
pub const DRM_MODE_TYPE_DRIVER: u32 = 1 << 6;

// DRM mode flags
pub const DRM_MODE_FLAG_PHSYNC: u32 = 1 << 0;
pub const DRM_MODE_FLAG_NHSYNC: u32 = 1 << 1;
pub const DRM_MODE_FLAG_PVSYNC: u32 = 1 << 2;
pub const DRM_MODE_FLAG_NVSYNC: u32 = 1 << 3;
pub const DRM_MODE_FLAG_INTERLACE: u32 = 1 << 4;
pub const DRM_MODE_FLAG_DBLSCAN: u32 = 1 << 5;
pub const DRM_MODE_FLAG_CSYNC: u32 = 1 << 6;
pub const DRM_MODE_FLAG_PCSYNC: u32 = 1 << 7;
pub const DRM_MODE_FLAG_NCSYNC: u32 = 1 << 8;
pub const DRM_MODE_FLAG_HSKEW: u32 = 1 << 9;
pub const DRM_MODE_FLAG_BCAST: u32 = 1 << 10;
pub const DRM_MODE_FLAG_PIXMUX: u32 = 1 << 11;
pub const DRM_MODE_FLAG_DBLCLK: u32 = 1 << 12;
pub const DRM_MODE_FLAG_CLKDIV2: u32 = 1 << 13;
