//! Semantic color definitions for UI elements

use crate::color::Color;

/// Semantic colors for UI elements
///
/// Provides semantic color names that automatically adapt to the current color scheme.
/// This follows the design pattern from macOS, iOS, and modern web frameworks.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct SemanticColor {
    // Base Colors
    /// Main background color
    pub background: Color,
    /// Secondary background color (e.g., cards, panels)
    pub background_secondary: Color,
    /// Tertiary background color (e.g., nested containers)
    pub background_tertiary: Color,

    // Text Colors
    /// Primary text color
    pub text: Color,
    /// Secondary text color (less prominent)
    pub text_secondary: Color,
    /// Tertiary text color (disabled state)
    pub text_tertiary: Color,
    /// Inverse text color (for dark backgrounds)
    pub text_inverse: Color,

    // Primary Brand Colors
    /// Primary brand color
    pub primary: Color,
    /// Light variant of primary
    pub primary_light: Color,
    /// Dark variant of primary
    pub primary_dark: Color,

    // Secondary
    /// Secondary brand color
    pub secondary: Color,

    // Accent Colors
    /// Accent color for highlights and emphasis
    pub accent: Color,
    /// Highlight variant of accent
    pub accent_highlight: Color,

    // Functional Colors
    /// Success state (green)
    pub success: Color,
    /// Error state (red)
    pub error: Color,
    /// Warning state (yellow/orange)
    pub warning: Color,
    /// Info state (blue)
    pub info: Color,

    // Border & Divider
    /// Border color for UI elements
    pub border: Color,
    /// Divider color between sections
    pub divider: Color,

    // Surface
    /// Surface color for elevated elements
    pub surface: Color,
    /// Variant surface color
    pub surface_variant: Color,
    /// Button background color
    pub button_background: Color,

    // Overlay
    /// Overlay/backdrop color
    pub overlay: Color,
    /// Shadow color
    pub shadow: Color,

    // Window Colors
    /// Window background color
    pub window_background: Color,
    /// Window border color
    pub window_border: Color,
    /// Window titlebar background (active window)
    pub window_titlebar_background: Color,
    /// Window titlebar text (active window)
    pub window_titlebar_text_active: Color,
    /// Window titlebar text (inactive window)
    pub window_titlebar_text_inactive: Color,
    /// Window titlebar border
    pub window_titlebar_border: Color,
    /// Window shadow color
    pub window_shadow: Color,
}

impl SemanticColor {
    /// Create semantic colors for light scheme
    pub fn light() -> Self {
        Self {
            // Base Colors - white/light gray
            background: Color::rgb(1.0, 1.0, 1.0),
            background_secondary: Color::rgb(0.97, 0.97, 0.97),
            background_tertiary: Color::rgb(0.95, 0.95, 0.97),

            // Text Colors - Apple label colors
            text: Color::rgb(0, 0, 0),                    // label
            text_secondary: Color::rgba(60, 60, 67, 0.6), // secondaryLabel
            text_tertiary: Color::rgba(60, 60, 67, 0.3),  // tertiaryLabel
            text_inverse: Color::rgb(1.0, 1.0, 1.0),

            // Primary - scarlet red (緋色)
            primary: Color::rgb(0.82, 0.15, 0.15),
            primary_light: Color::rgb(0.92, 0.35, 0.35),
            primary_dark: Color::rgb(0.60, 0.10, 0.10),

            // Secondary - gray
            secondary: Color::rgb(0.56, 0.56, 0.58),

            // Accent - cyan/teal
            accent: Color::rgb(0.35, 0.78, 0.98),
            accent_highlight: Color::rgb(0.53, 0.84, 0.96),

            // Functional Colors
            success: Color::rgb(0.2, 0.78, 0.35),
            error: Color::rgb(1.0, 0.23, 0.19),
            warning: Color::rgb(1.0, 0.58, 0.0),
            info: Color::rgb(0.0, 0.48, 1.0),

            // Border & Divider
            border: Color::rgb(0.78, 0.78, 0.78),
            divider: Color::rgb(0.89, 0.89, 0.89),

            // Surface
            surface: Color::rgb(1.0, 1.0, 1.0),
            surface_variant: Color::rgb(0.95, 0.95, 0.97),
            button_background: Color::rgb(229, 229, 234), // systemGray5

            // Overlay
            overlay: Color::rgba(0.0, 0.0, 0.0, 0.24),
            shadow: Color::rgba(0.0, 0.0, 0.0, 0.1),

            // Window Colors
            window_background: Color::rgb(1.0, 1.0, 1.0),
            window_border: Color::rgb(0.71, 0.71, 0.71),
            window_titlebar_background: Color::rgb(0.92, 0.92, 0.92),
            window_titlebar_text_active: Color::rgb(0.0, 0.0, 0.0),
            window_titlebar_text_inactive: Color::rgb(0.39, 0.39, 0.39),
            window_titlebar_border: Color::rgb(0.78, 0.78, 0.78),
            window_shadow: Color::rgba(0.0, 0.0, 0.0, 0.2),
        }
    }

    /// Create semantic colors for dark scheme
    pub fn dark() -> Self {
        Self {
            // Base Colors - dark gray/black
            background: Color::rgb(0.12, 0.12, 0.12),
            background_secondary: Color::rgb(0.18, 0.18, 0.18),
            background_tertiary: Color::rgb(0.24, 0.24, 0.24),

            // Text Colors - Apple label colors
            text: Color::rgb(255, 255, 255),                 // label
            text_secondary: Color::rgba(235, 235, 245, 0.6), // secondaryLabel
            text_tertiary: Color::rgba(235, 235, 245, 0.3),  // tertiaryLabel
            text_inverse: Color::rgb(0.0, 0.0, 0.0),

            // Primary - scarlet red (緋色, lighter variant for dark mode)
            primary: Color::rgb(0.88, 0.25, 0.25),
            primary_light: Color::rgb(0.95, 0.45, 0.45),
            primary_dark: Color::rgb(0.65, 0.15, 0.15),

            // Secondary - gray
            secondary: Color::rgb(0.55, 0.55, 0.57),

            // Accent - cyan/teal
            accent: Color::rgb(0.39, 0.82, 1.0),
            accent_highlight: Color::rgb(0.55, 0.86, 0.96),

            // Functional Colors
            success: Color::rgb(0.27, 0.84, 0.39),
            error: Color::rgb(1.0, 0.31, 0.27),
            warning: Color::rgb(1.0, 0.65, 0.16),
            info: Color::rgb(0.2, 0.55, 1.0),

            // Border & Divider
            border: Color::rgb(0.31, 0.31, 0.31),
            divider: Color::rgb(0.24, 0.24, 0.24),

            // Surface
            surface: Color::rgb(0.18, 0.18, 0.18),
            surface_variant: Color::rgb(0.22, 0.22, 0.22),
            button_background: Color::rgb(44, 44, 46), // systemGray5 (dark)

            // Overlay
            overlay: Color::rgba(0.0, 0.0, 0.0, 0.4),
            shadow: Color::rgba(0.0, 0.0, 0.0, 0.24),

            // Window Colors
            window_background: Color::rgb(0.16, 0.16, 0.16),
            window_border: Color::rgb(0.35, 0.35, 0.35),
            window_titlebar_background: Color::rgb(0.22, 0.22, 0.22),
            window_titlebar_text_active: Color::rgb(1.0, 1.0, 1.0),
            window_titlebar_text_inactive: Color::rgb(0.59, 0.59, 0.59),
            window_titlebar_border: Color::rgb(0.31, 0.31, 0.31),
            window_shadow: Color::rgba(0.0, 0.0, 0.0, 0.31),
        }
    }
}
