//! Color palette API

use super::{ColorScheme, SemanticColor, SystemColors};

/// Color palette for the application
///
/// Provides access to semantic and system colors based on the current color scheme.
/// This is the main API for accessing colors in ScarletUI.
#[derive(Clone, PartialEq, Debug)]
pub struct ColorPalette {
    scheme: ColorScheme,
    semantic: SemanticColor,
    system: SystemColors,
}

impl ColorPalette {
    /// Create a color palette for light scheme
    pub fn light() -> Self {
        Self {
            scheme: ColorScheme::Light,
            semantic: SemanticColor::light(),
            system: SystemColors::light(),
        }
    }

    /// Create a color palette for dark scheme
    pub fn dark() -> Self {
        Self {
            scheme: ColorScheme::Dark,
            semantic: SemanticColor::dark(),
            system: SystemColors::dark(),
        }
    }

    /// Get the current color scheme
    pub fn scheme(&self) -> ColorScheme {
        self.scheme
    }

    // Semantic color accessors

    /// Main background color
    pub fn background(&self) -> crate::Color {
        self.semantic.background
    }

    /// Secondary background color
    pub fn background_secondary(&self) -> crate::Color {
        self.semantic.background_secondary
    }

    /// Tertiary background color
    pub fn background_tertiary(&self) -> crate::Color {
        self.semantic.background_tertiary
    }

    /// Primary text color
    pub fn text(&self) -> crate::Color {
        self.semantic.text
    }

    /// Primary text color (Apple-style naming)
    pub fn text_primary(&self) -> crate::Color {
        self.semantic.text
    }

    /// Secondary text color
    pub fn text_secondary(&self) -> crate::Color {
        self.semantic.text_secondary
    }

    /// Tertiary text color (disabled)
    pub fn text_tertiary(&self) -> crate::Color {
        self.semantic.text_tertiary
    }

    /// Inverse text color
    pub fn text_inverse(&self) -> crate::Color {
        self.semantic.text_inverse
    }

    /// Primary brand color
    pub fn primary(&self) -> crate::Color {
        self.semantic.primary
    }

    /// Light variant of primary
    pub fn primary_light(&self) -> crate::Color {
        self.semantic.primary_light
    }

    /// Dark variant of primary
    pub fn primary_dark(&self) -> crate::Color {
        self.semantic.primary_dark
    }

    /// Secondary brand color
    pub fn secondary(&self) -> crate::Color {
        self.semantic.secondary
    }

    /// Accent color
    pub fn accent(&self) -> crate::Color {
        self.semantic.accent
    }

    /// Accent highlight color
    pub fn accent_highlight(&self) -> crate::Color {
        self.semantic.accent_highlight
    }

    /// Success color
    pub fn success(&self) -> crate::Color {
        self.semantic.success
    }

    /// Error color
    pub fn error(&self) -> crate::Color {
        self.semantic.error
    }

    /// Warning color
    pub fn warning(&self) -> crate::Color {
        self.semantic.warning
    }

    /// Info color
    pub fn info(&self) -> crate::Color {
        self.semantic.info
    }

    /// Border color
    pub fn border(&self) -> crate::Color {
        self.semantic.border
    }

    /// Divider color
    pub fn divider(&self) -> crate::Color {
        self.semantic.divider
    }

    /// Surface color
    pub fn surface(&self) -> crate::Color {
        self.semantic.surface
    }

    /// Surface variant color
    pub fn surface_variant(&self) -> crate::Color {
        self.semantic.surface_variant
    }

    /// Button background color
    pub fn button_background(&self) -> crate::Color {
        self.semantic.button_background
    }

    /// Overlay color
    pub fn overlay(&self) -> crate::Color {
        self.semantic.overlay
    }

    /// Shadow color
    pub fn shadow(&self) -> crate::Color {
        self.semantic.shadow
    }

    // Window color accessors

    /// Window background color
    pub fn window_background(&self) -> crate::Color {
        self.semantic.window_background
    }

    /// Window border color
    pub fn window_border(&self) -> crate::Color {
        self.semantic.window_border
    }

    /// Window titlebar background (active)
    pub fn window_titlebar_background(&self) -> crate::Color {
        self.semantic.window_titlebar_background
    }

    /// Window titlebar text (active window)
    pub fn window_titlebar_text_active(&self) -> crate::Color {
        self.semantic.window_titlebar_text_active
    }

    /// Window titlebar text (inactive window)
    pub fn window_titlebar_text_inactive(&self) -> crate::Color {
        self.semantic.window_titlebar_text_inactive
    }

    /// Window titlebar border
    pub fn window_titlebar_border(&self) -> crate::Color {
        self.semantic.window_titlebar_border
    }

    /// Window shadow color
    pub fn window_shadow(&self) -> crate::Color {
        self.semantic.window_shadow
    }

    // System color accessors

    /// Get system gray colors
    pub fn gray(&self) -> &GrayColors {
        &self.system.gray
    }

    /// Get system blue colors
    pub fn blue(&self) -> &BlueColors {
        &self.system.blue
    }

    /// Get system green colors
    pub fn green(&self) -> &GreenColors {
        &self.system.green
    }

    /// Get system orange colors
    pub fn orange(&self) -> &OrangeColors {
        &self.system.orange
    }

    /// Get system pink colors
    pub fn pink(&self) -> &PinkColors {
        &self.system.pink
    }

    /// Get system purple colors
    pub fn purple(&self) -> &PurpleColors {
        &self.system.purple
    }

    /// Get system red colors
    pub fn red(&self) -> &RedColors {
        &self.system.red
    }

    /// Get system yellow colors
    pub fn yellow(&self) -> &YellowColors {
        &self.system.yellow
    }

    // Menu color accessors

    /// Menu hover color
    pub fn menu_hover(&self) -> crate::Color {
        crate::Color::rgb(0.824, 0.824, 0.839)
    }

    /// Menu active color (pressed)
    pub fn menu_active(&self) -> crate::Color {
        crate::Color::rgb(0.706, 0.706, 0.745)
    }
}

impl Default for ColorPalette {
    fn default() -> Self {
        Self::light()
    }
}

// Re-export system color types
pub use super::system::{
    BlueColors, GrayColors, GreenColors, OrangeColors, PinkColors, PurpleColors, RedColors,
    YellowColors,
};
