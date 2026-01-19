//! Design system for Scarlet Desktop applications
//!
//! This module provides a consistent color palette and design tokens
//! that should be used across all Scarlet Desktop apps.
//!
//! The palette system supports both light and dark modes, with automatic
//! selection based on the desktop configuration.

use crate::Color;
use core::sync::atomic::{AtomicBool, Ordering};

/// Global theme mode (light or dark)
static IS_DARK_MODE: AtomicBool = AtomicBool::new(false);

/// Set the current theme mode
pub fn set_dark_mode(enabled: bool) {
    IS_DARK_MODE.store(enabled, Ordering::SeqCst);
}

/// Get the current theme mode
pub fn is_dark_mode() -> bool {
    IS_DARK_MODE.load(Ordering::SeqCst)
}

/// Toggle between light and dark mode
pub fn toggle_theme() {
    let current = is_dark_mode();
    set_dark_mode(!current);
}

/// Color palette for a specific theme mode
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    // Background colors
    pub bg: Color,
    pub surface: Color,
    pub sidebar_bg: Color,
    pub elevated: Color,

    // Border and separator
    pub border: Color,
    pub separator: Color,
    pub focus_ring: Color,

    // Primary colors
    pub primary: Color,
    pub primary_dark: Color,
    pub primary_light: Color,
    pub hover: Color,

    // Text colors
    pub text_main: Color,
    pub text_sub: Color,
    pub text_mute: Color,
    pub text_inverted: Color,

    // Status colors
    pub success: Color,
    pub success_bg: Color,
    pub warning: Color,
    pub warning_bg: Color,
    pub error: Color,
    pub error_bg: Color,
    pub info: Color,
    pub info_bg: Color,

    // Overlay colors
    pub overlay: Color,
    pub tooltip_bg: Color,
}

impl Palette {
    /// Get the current palette based on theme mode
    pub fn current() -> &'static Self {
        if is_dark_mode() {
            &DARK_PALETTE
        } else {
            &LIGHT_PALETTE
        }
    }

    /// Get light palette
    pub fn light() -> &'static Self {
        &LIGHT_PALETTE
    }

    /// Get dark palette
    pub fn dark() -> &'static Self {
        &DARK_PALETTE
    }
}

/// Light theme palette
const LIGHT_PALETTE: Palette = Palette {
    // Background colors - macOS light gray
    bg: Color::rgb(242, 242, 247),
    surface: Color::rgb(255, 255, 255),
    sidebar_bg: Color::rgb(230, 230, 235),
    elevated: Color::rgb(245, 245, 250),

    // Border and separator
    border: Color::rgb(200, 200, 200),
    separator: Color::rgb(180, 180, 180),
    focus_ring: Color::rgb(0, 122, 255),

    // Primary colors - Scarlet red (muted)
    primary: Color::rgb(190, 30, 50),
    primary_dark: Color::rgb(160, 20, 35),
    primary_light: Color::rgb(255, 120, 140),
    hover: Color::rgb(220, 220, 225),

    // Text colors
    text_main: Color::rgb(30, 30, 30),
    text_sub: Color::rgb(100, 100, 100),
    text_mute: Color::rgb(140, 140, 140),
    text_inverted: Color::rgb(255, 255, 255),

    // Status colors
    success: Color::rgb(52, 199, 89),
    success_bg: Color::rgb(232, 253, 240),
    warning: Color::rgb(245, 158, 11),
    warning_bg: Color::rgb(254, 252, 232),
    error: Color::rgb(239, 68, 68),
    error_bg: Color::rgb(254, 242, 242),
    info: Color::rgb(59, 130, 246),
    info_bg: Color::rgb(239, 246, 255),

    // Overlay colors
    overlay: Color::rgba(0, 0, 0, 128),
    tooltip_bg: Color::rgba(30, 30, 30, 230),
};

/// Dark theme palette
const DARK_PALETTE: Palette = Palette {
    // Background colors - dark gray
    bg: Color::rgb(30, 30, 35),
    surface: Color::rgb(40, 40, 45),
    sidebar_bg: Color::rgb(25, 25, 30),
    elevated: Color::rgb(35, 35, 40),

    // Border and separator
    border: Color::rgb(60, 60, 70),
    separator: Color::rgb(70, 70, 80),
    focus_ring: Color::rgb(100, 180, 255),

    // Primary colors - Scarlet red (adjusted for dark mode)
    primary: Color::rgb(220, 60, 80),
    primary_dark: Color::rgb(255, 100, 120),
    primary_light: Color::rgb(180, 40, 60),
    hover: Color::rgb(50, 50, 60),

    // Text colors
    text_main: Color::rgb(235, 235, 240),
    text_sub: Color::rgb(170, 170, 180),
    text_mute: Color::rgb(130, 130, 140),
    text_inverted: Color::rgb(30, 30, 35),

    // Status colors
    success: Color::rgb(74, 222, 128),
    success_bg: Color::rgb(30, 60, 40),
    warning: Color::rgb(250, 200, 50),
    warning_bg: Color::rgb(70, 60, 20),
    error: Color::rgb(250, 100, 100),
    error_bg: Color::rgb(70, 30, 30),
    info: Color::rgb(100, 180, 255),
    info_bg: Color::rgb(30, 50, 80),

    // Overlay colors
    overlay: Color::rgba(0, 0, 0, 180),
    tooltip_bg: Color::rgba(20, 20, 25, 240),
};

/// Backward compatibility: module with const colors (light mode defaults)
///
/// DEPRECATED: Use `Palette::current()` instead for theme-aware code.
/// These constants are provided for backward compatibility.
pub mod palette {
    use super::Color;

    // Background colors
    pub const BG: Color = Color::rgb(242, 242, 247);
    pub const SURFACE: Color = Color::rgb(255, 255, 255);
    pub const SIDEBAR_BG: Color = Color::rgb(230, 230, 235);
    pub const ELEVATED: Color = Color::rgb(245, 245, 250);

    // Border and separator
    pub const BORDER: Color = Color::rgb(200, 200, 200);
    pub const SEPARATOR: Color = Color::rgb(180, 180, 180);
    pub const FOCUS_RING: Color = Color::rgb(0, 122, 255);

    // Primary colors
    pub const PRIMARY: Color = Color::rgb(190, 30, 50);
    pub const PRIMARY_DARK: Color = Color::rgb(160, 20, 35);
    pub const PRIMARY_LIGHT: Color = Color::rgb(255, 120, 140);
    pub const HOVER: Color = Color::rgb(220, 220, 225);

    // Text colors
    pub const TEXT_MAIN: Color = Color::rgb(30, 30, 30);
    pub const TEXT_SUB: Color = Color::rgb(100, 100, 100);
    pub const TEXT_MUTE: Color = Color::rgb(140, 140, 140);
    pub const TEXT_INVERTED: Color = Color::rgb(255, 255, 255);

    // Status colors
    pub const SUCCESS: Color = Color::rgb(52, 199, 89);
    pub const SUCCESS_BG: Color = Color::rgb(232, 253, 240);
    pub const WARNING: Color = Color::rgb(245, 158, 11);
    pub const WARNING_BG: Color = Color::rgb(254, 252, 232);
    pub const ERROR: Color = Color::rgb(239, 68, 68);
    pub const ERROR_BG: Color = Color::rgb(254, 242, 242);
    pub const INFO: Color = Color::rgb(59, 130, 246);
    pub const INFO_BG: Color = Color::rgb(239, 246, 255);

    // Overlay colors
    pub const OVERLAY: Color = Color::rgba(0, 0, 0, 128);
    pub const TOOLTIP_BG: Color = Color::rgba(30, 30, 30, 230);
}

/// Spacing constants for consistent layouts
pub mod spacing {
    /// Base unit for spacing (4px)
    pub const UNIT: u32 = 4;
    /// Small spacing (8px)
    pub const SMALL: u32 = 8;
    /// Medium spacing (16px)
    pub const MEDIUM: u32 = 16;
    /// Large spacing (24px)
    pub const LARGE: u32 = 24;
    /// Extra large spacing (32px)
    pub const XLARGE: u32 = 32;
}

/// Border radius values for consistent rounded corners
pub mod radius {
    /// No radius (sharp corners)
    pub const NONE: u32 = 0;
    /// Small radius (buttons, tags)
    pub const SMALL: u32 = 4;
    /// Medium radius (cards, panels)
    pub const MEDIUM: u32 = 8;
    /// Large radius (modals, popovers)
    pub const LARGE: u32 = 12;
    /// Extra large radius (hero elements)
    pub const XLARGE: u32 = 16;
}

/// Typography scale for consistent font sizing
pub mod typography {
    /// Caption text (11px)
    pub const CAPTION: f32 = 11.0;
    /// Small text (12px)
    pub const SMALL: f32 = 12.0;
    /// Body text (13px)
    pub const BODY: f32 = 13.0;
    /// Subheading (14px)
    pub const SUBHEADING: f32 = 14.0;
    /// Heading (16px)
    pub const HEADING: f32 = 16.0;
    /// Title (20px)
    pub const TITLE: f32 = 20.0;
    /// Large title (24px)
    pub const LARGE_TITLE: f32 = 24.0;
    /// Display (28px)
    pub const DISPLAY: f32 = 28.0;
}

/// Common sizes for UI elements
pub mod size {
    /// Button height (32px)
    pub const BUTTON_HEIGHT: u32 = 32;
    /// Toolbar height (40px)
    pub const TOOLBAR_HEIGHT: u32 = 40;
    /// Sidebar width (200px)
    pub const SIDEBAR_WIDTH: u32 = 200;
    /// Icon size (16px)
    pub const ICON: u32 = 16;
    /// Touch target (44px)
    pub const TOUCH_TARGET: u32 = 44;
}
