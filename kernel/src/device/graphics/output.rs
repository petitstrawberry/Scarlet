//! Display output abstraction

use alloc::string::String;

use super::FramebufferConfig;

/// Rectangular display update region.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayRegion {
    /// Left edge in pixels.
    pub x: u32,
    /// Top edge in pixels.
    pub y: u32,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

impl DisplayRegion {
    /// Create a new display update region.
    ///
    /// # Arguments
    ///
    /// * `x` - Left edge in pixels.
    /// * `y` - Top edge in pixels.
    /// * `width` - Width in pixels.
    /// * `height` - Height in pixels.
    ///
    /// # Returns
    ///
    /// A new `DisplayRegion`.
    pub const fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Create a region covering the full framebuffer.
    ///
    /// # Arguments
    ///
    /// * `config` - Framebuffer configuration.
    ///
    /// # Returns
    ///
    /// A region covering the visible framebuffer.
    pub const fn full(config: &FramebufferConfig) -> Self {
        Self {
            x: 0,
            y: 0,
            width: config.width,
            height: config.height,
        }
    }
}

/// Physical or virtual nature of a display connector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectorKind {
    /// The driver cannot tell.
    Unknown,
    /// A panel built into the machine.
    Internal,
    /// A port for an external monitor (DisplayPort, HDMI, ...).
    External,
    /// An output of a virtual display device such as virtio-gpu.
    Virtual,
}

/// One display connector exposed by a graphics device.
///
/// A connector is a place a monitor can be attached, whether or not one is.
/// Connectors are indexed from zero within their device; index 0 is the
/// connector the device's primary-output methods (`get_framebuffer_config`,
/// `present_*`) drive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectorInfo {
    /// Index within the owning device.
    pub index: u32,
    /// Driver-chosen name, unique within the device (e.g. `"virtual-1"`).
    pub name: String,
    /// Connector kind.
    pub kind: ConnectorKind,
    /// Whether a monitor is attached and enabled.
    pub connected: bool,
    /// Current mode width in pixels, or 0 when not connected.
    pub width: u32,
    /// Current mode height in pixels, or 0 when not connected.
    pub height: u32,
    /// Refresh rate in millihertz, or 0 when unknown.
    pub refresh_mhz: u32,
    /// Physical size in millimetres, when the monitor reports it.
    pub physical_size_mm: Option<(u32, u32)>,
}
