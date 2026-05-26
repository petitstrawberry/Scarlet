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

/// A single display connector / output (panel, DP port, HDMI, etc.)
pub trait DisplayOutput: Send + Sync {
    /// Human-readable name (e.g. "internal-panel", "dp0")
    fn name(&self) -> &str;

    /// Whether a display is connected (may require HPD detect)
    fn is_connected(&self) -> bool;

    /// Present a framebuffer on this output.
    ///
    /// `fb_paddr` is the physical address of the framebuffer memory.
    /// For direct-mapped displays (e.g. simple-fb) this is a no-op.
    /// For coprocessor-driven displays (e.g. DCPext) this triggers DMA/scanout.
    fn present(&self, config: &FramebufferConfig, fb_paddr: usize) -> Result<(), &'static str>;

    /// Present a framebuffer region on this output.
    ///
    /// # Arguments
    ///
    /// * `config` - Framebuffer configuration.
    /// * `fb_paddr` - Physical address of the framebuffer memory.
    /// * `region` - Updated display region.
    ///
    /// # Returns
    ///
    /// Success or an error describing why presentation failed.
    fn present_region(
        &self,
        config: &FramebufferConfig,
        fb_paddr: usize,
        _region: DisplayRegion,
    ) -> Result<(), &'static str> {
        self.present(config, fb_paddr)
    }
}

/// A display mode (resolution + refresh rate)
#[derive(Debug, Clone)]
pub struct DisplayMode {
    pub width: u32,
    pub height: u32,
    pub refresh_rate: u32,
    pub name: String,
}

pub struct SimpleFbOutput {
    name: String,
}

impl SimpleFbOutput {
    pub fn new(name: &str) -> Self {
        Self {
            name: String::from(name),
        }
    }
}

impl DisplayOutput for SimpleFbOutput {
    fn name(&self) -> &str {
        &self.name
    }

    fn is_connected(&self) -> bool {
        true
    }

    fn present(&self, _config: &FramebufferConfig, _fb_paddr: usize) -> Result<(), &'static str> {
        Ok(())
    }
}
