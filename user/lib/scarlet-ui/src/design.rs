//! Design system for Scarlet Desktop applications
//!
//! This module provides a consistent color palette and design tokens
//! that should be used across all Scarlet Desktop apps.

use crate::Color;

/// Common color palette for Scarlet Desktop apps
///
/// Inspired by macOS with a modern, clean aesthetic.
/// All desktop applications should use these colors for consistency.
pub mod palette {
    use super::Color;

    // Background colors
    /// macOS light gray background (main window bg)
    pub const BG: Color = Color::rgb(242, 242, 247);
    /// Pure white surface (cards, panels, content areas)
    pub const SURFACE: Color = Color::rgb(255, 255, 255);
    /// Slightly darker background (sidebars, toolbars)
    pub const SIDEBAR_BG: Color = Color::rgb(230, 230, 235);
    /// Elevated surface (dialogs, popovers)
    pub const ELEVATED: Color = Color::rgb(245, 245, 250);

    // Border and separator
    /// Subtle border color (dividers, borders)
    pub const BORDER: Color = Color::rgb(200, 200, 200);
    /// Separator line color (between sections)
    pub const SEPARATOR: Color = Color::rgb(180, 180, 180);
    /// Focus ring color
    pub const FOCUS_RING: Color = Color::rgb(0, 122, 255);

    // Primary colors
    /// macOS blue (primary actions, links, active states)
    pub const PRIMARY: Color = Color::rgb(0, 122, 255);
    /// Darker blue (hover states)
    pub const PRIMARY_DARK: Color = Color::rgb(0, 95, 200);
    /// Lighter blue (subtle highlights)
    pub const PRIMARY_LIGHT: Color = Color::rgb(100, 180, 255);
    /// Hover state background
    pub const HOVER: Color = Color::rgb(220, 220, 225);

    // Text colors
    /// Main text color (almost black, for readability)
    pub const TEXT_MAIN: Color = Color::rgb(30, 30, 30);
    /// Secondary text color (labels, descriptions)
    pub const TEXT_SUB: Color = Color::rgb(100, 100, 100);
    /// Muted text color (placeholders, disabled text)
    pub const TEXT_MUTE: Color = Color::rgb(140, 140, 140);
    /// Inverted text (on dark backgrounds)
    pub const TEXT_INVERTED: Color = Color::rgb(255, 255, 255);

    // Status colors
    /// Success (green, for positive states)
    pub const SUCCESS: Color = Color::rgb(52, 199, 89);
    /// Success background (subtle)
    pub const SUCCESS_BG: Color = Color::rgb(232, 253, 240);
    /// Warning (amber/orange, for caution states)
    pub const WARNING: Color = Color::rgb(245, 158, 11);
    /// Warning background (subtle)
    pub const WARNING_BG: Color = Color::rgb(254, 252, 232);
    /// Error (red, for destructive states)
    pub const ERROR: Color = Color::rgb(239, 68, 68);
    /// Error background (subtle)
    pub const ERROR_BG: Color = Color::rgb(254, 242, 242);
    /// Info (blue, for informational states)
    pub const INFO: Color = Color::rgb(59, 130, 246);
    /// Info background (subtle)
    pub const INFO_BG: Color = Color::rgb(239, 246, 255);

    // Overlay colors
    /// Modal overlay (semi-transparent black)
    pub const OVERLAY: Color = Color::rgba(0, 0, 0, 128);
    /// Tooltip overlay (semi-transparent black)
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
