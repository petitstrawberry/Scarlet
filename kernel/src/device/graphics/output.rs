//! Display output abstraction

use alloc::string::String;

use super::FramebufferConfig;

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
}

/// A display mode (resolution + refresh rate)
#[derive(Debug, Clone)]
pub struct DisplayMode {
    pub width: u32,
    pub height: u32,
    pub refresh_rate: u32,
    pub name: String,
}
