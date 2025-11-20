//! # DRM Type Definitions
//!
//! This module defines the data structures used by the Linux DRM interface.
//! These structures are compatible with Linux's DRM API and are used for
//! communication between user space and kernel space.

/// DRM version information
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct DrmVersion {
    /// Major version number
    pub version_major: i32,
    /// Minor version number
    pub version_minor: i32,
    /// Patch level
    pub version_patchlevel: i32,
    /// Length of name string
    pub name_len: usize,
    /// Pointer to name string
    pub name: usize,
    /// Length of date string
    pub date_len: usize,
    /// Pointer to date string
    pub date: usize,
    /// Length of description string
    pub desc_len: usize,
    /// Pointer to description string
    pub desc: usize,
}

/// DRM mode resources
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct DrmModeCardRes {
    /// Pointer to array of framebuffer IDs
    pub fb_id_ptr: u64,
    /// Pointer to array of CRTC IDs
    pub crtc_id_ptr: u64,
    /// Pointer to array of connector IDs
    pub connector_id_ptr: u64,
    /// Pointer to array of encoder IDs
    pub encoder_id_ptr: u64,
    /// Number of framebuffers
    pub count_fbs: u32,
    /// Number of CRTCs
    pub count_crtcs: u32,
    /// Number of connectors
    pub count_connectors: u32,
    /// Number of encoders
    pub count_encoders: u32,
    /// Minimum width
    pub min_width: u32,
    /// Maximum width
    pub max_width: u32,
    /// Minimum height
    pub min_height: u32,
    /// Maximum height
    pub max_height: u32,
}

/// DRM mode information
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct DrmModeModeInfo {
    /// Pixel clock in KHz
    pub clock: u32,
    /// Horizontal display size
    pub hdisplay: u16,
    /// Horizontal sync start
    pub hsync_start: u16,
    /// Horizontal sync end
    pub hsync_end: u16,
    /// Horizontal total size
    pub htotal: u16,
    /// Horizontal skew
    pub hskew: u16,
    /// Vertical display size
    pub vdisplay: u16,
    /// Vertical sync start
    pub vsync_start: u16,
    /// Vertical sync end
    pub vsync_end: u16,
    /// Vertical total size
    pub vtotal: u16,
    /// Vertical scan
    pub vscan: u16,
    /// Vertical refresh rate
    pub vrefresh: u32,
    /// Mode flags
    pub flags: u32,
    /// Mode type
    pub type_: u32,
    /// Mode name
    pub name: [u8; 32],
}

/// DRM CRTC (Cathode Ray Tube Controller) configuration
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct DrmModeCrtc {
    /// CRTC ID (set on output)
    pub crtc_id: u32,
    /// Framebuffer ID (input/output)
    pub fb_id: u32,
    /// X position (input/output)
    pub x: u32,
    /// Y position (input/output)
    pub y: u32,
    /// Gamma size (output)
    pub gamma_size: u32,
    /// Whether mode is valid (input/output)
    pub mode_valid: u32,
    /// Mode information (input/output)
    pub mode: DrmModeModeInfo,
}

/// DRM create dumb buffer request
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct DrmModeCreateDumb {
    /// Height in pixels (input)
    pub height: u32,
    /// Width in pixels (input)
    pub width: u32,
    /// Bits per pixel (input)
    pub bpp: u32,
    /// Flags (input)
    pub flags: u32,
    /// Handle (output)
    pub handle: u32,
    /// Pitch/stride (output)
    pub pitch: u32,
    /// Size (output)
    pub size: u64,
}

/// DRM map dumb buffer request
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct DrmModeMapDumb {
    /// Handle (input)
    pub handle: u32,
    /// Padding
    pub pad: u32,
    /// Offset (output) - used with mmap
    pub offset: u64,
}

/// DRM destroy dumb buffer request
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct DrmModeDestroyDumb {
    /// Handle (input)
    pub handle: u32,
}

/// DRM page flip request
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct DrmModePageFlip {
    /// CRTC ID (input)
    pub crtc_id: u32,
    /// Framebuffer ID (input)
    pub fb_id: u32,
    /// Flags (input)
    pub flags: u32,
    /// User data (input) - returned in event
    pub user_data: u64,
}

/// DRM connector information
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct DrmModeGetConnector {
    /// Pointer to array of encoder IDs
    pub encoders_ptr: u64,
    /// Pointer to array of mode information
    pub modes_ptr: u64,
    /// Pointer to array of property IDs
    pub props_ptr: u64,
    /// Pointer to array of property values
    pub prop_values_ptr: u64,
    /// Number of modes
    pub count_modes: u32,
    /// Number of properties
    pub count_props: u32,
    /// Number of encoders
    pub count_encoders: u32,
    /// Encoder ID
    pub encoder_id: u32,
    /// Connector ID
    pub connector_id: u32,
    /// Connector type
    pub connector_type: u32,
    /// Connector type ID
    pub connector_type_id: u32,
    /// Connection status
    pub connection: u32,
    /// Width in millimeters
    pub mm_width: u32,
    /// Height in millimeters
    pub mm_height: u32,
    /// Subpixel order
    pub subpixel: u32,
    /// Padding
    pub pad: u32,
}

/// DRM encoder information
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct DrmModeGetEncoder {
    /// Encoder ID
    pub encoder_id: u32,
    /// Encoder type
    pub encoder_type: u32,
    /// CRTC ID
    pub crtc_id: u32,
    /// Possible CRTCs (bitmask)
    pub possible_crtcs: u32,
    /// Possible clones (bitmask)
    pub possible_clones: u32,
}

// DRM constants

/// DRM connector status: connected
pub const DRM_MODE_CONNECTED: u32 = 1;
/// DRM connector status: disconnected
pub const DRM_MODE_DISCONNECTED: u32 = 2;
/// DRM connector status: unknown
pub const DRM_MODE_UNKNOWNCONNECTION: u32 = 3;

/// DRM connector type: VGA
pub const DRM_MODE_CONNECTOR_VGA: u32 = 1;
/// DRM connector type: DVII
pub const DRM_MODE_CONNECTOR_DVII: u32 = 2;
/// DRM connector type: DVID
pub const DRM_MODE_CONNECTOR_DVID: u32 = 3;
/// DRM connector type: DVIA
pub const DRM_MODE_CONNECTOR_DVIA: u32 = 4;
/// DRM connector type: Composite
pub const DRM_MODE_CONNECTOR_Composite: u32 = 5;
/// DRM connector type: SVIDEO
pub const DRM_MODE_CONNECTOR_SVIDEO: u32 = 6;
/// DRM connector type: LVDS
pub const DRM_MODE_CONNECTOR_LVDS: u32 = 7;
/// DRM connector type: Component
pub const DRM_MODE_CONNECTOR_Component: u32 = 8;
/// DRM connector type: 9PinDIN
pub const DRM_MODE_CONNECTOR_9PinDIN: u32 = 9;
/// DRM connector type: DisplayPort
pub const DRM_MODE_CONNECTOR_DisplayPort: u32 = 10;
/// DRM connector type: HDMIA
pub const DRM_MODE_CONNECTOR_HDMIA: u32 = 11;
/// DRM connector type: HDMIB
pub const DRM_MODE_CONNECTOR_HDMIB: u32 = 12;
/// DRM connector type: TV
pub const DRM_MODE_CONNECTOR_TV: u32 = 13;
/// DRM connector type: eDP
pub const DRM_MODE_CONNECTOR_eDP: u32 = 14;
/// DRM connector type: VIRTUAL
pub const DRM_MODE_CONNECTOR_VIRTUAL: u32 = 15;
/// DRM connector type: DSI
pub const DRM_MODE_CONNECTOR_DSI: u32 = 16;

/// DRM page flip flag: event
pub const DRM_MODE_PAGE_FLIP_EVENT: u32 = 0x01;
/// DRM page flip flag: async
pub const DRM_MODE_PAGE_FLIP_ASYNC: u32 = 0x02;
