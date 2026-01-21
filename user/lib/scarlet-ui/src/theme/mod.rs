//! Theme system for ScarletUI with dynamic color palette support
//!
//! Provides a centralized color theming system with light and dark schemes.

use crate::geometry::Color;
use std::sync::Mutex;
use std::ops::{Deref, DerefMut};

/// Color scheme variants
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorScheme {
    Light,
    Dark,
}

/// Theme configuration with all UI colors
#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    pub scheme: ColorScheme,

    // Window colors
    pub window_background: Color,
    pub window_border: Color,

    // Titlebar colors
    pub titlebar_background: Color,
    pub titlebar_background_end: Color, // For gradient
    pub titlebar_text: Color,
    pub titlebar_border: Color,

    // Titlebar control button colors (minimize, maximize, close)
    pub titlebar_button_background: Color,
    pub titlebar_button_background_hovered: Color,
    pub titlebar_button_icon: Color,

    // Button colors
    pub button_background: Color,
    pub button_background_hovered: Color,
    pub button_background_pressed: Color,
    pub button_text: Color,
    pub button_border: Color,

    // Text colors
    pub text_primary: Color,
    pub text_secondary: Color,

    // Background colors
    pub background_primary: Color,
    pub background_secondary: Color,
}

impl Theme {
    /// Create a new theme with the specified color scheme
    pub fn new(scheme: ColorScheme) -> Self {
        match scheme {
            ColorScheme::Light => Self::light(),
            ColorScheme::Dark => Self::dark(),
        }
    }

    /// Create a light theme (modern and simple gray-based design)
    pub fn light() -> Self {
        Self {
            scheme: ColorScheme::Light,

            // Window colors - based on deprecated
            window_background: Color::rgb(255, 255, 255),
            window_border: Color::rgb(100, 100, 105),

            // Titlebar colors - based on deprecated
            titlebar_background: Color::rgb(235, 235, 238),
            titlebar_background_end: Color::rgb(235, 235, 238),
            titlebar_text: Color::rgb(20, 20, 24),
            titlebar_border: Color::rgb(100, 100, 105),

            // Titlebar control button colors - deprecated style
            titlebar_button_background: Color::rgb(235, 235, 238),
            titlebar_button_background_hovered: Color::rgb(210, 210, 214),
            titlebar_button_icon: Color::rgb(30, 30, 34),

            // Button colors
            button_background: Color::rgb(235, 235, 238),
            button_background_hovered: Color::rgb(220, 220, 224),
            button_background_pressed: Color::rgb(190, 190, 194),
            button_text: Color::rgb(20, 20, 24),
            button_border: Color::rgb(180, 180, 180),

            // Text colors
            text_primary: Color::rgb(20, 20, 24),
            text_secondary: Color::rgb(120, 120, 120),

            // Background colors
            background_primary: Color::rgb(255, 255, 255),
            background_secondary: Color::rgb(245, 245, 245),
        }
    }

    /// Create a dark theme (modern and simple gray-based design)
    pub fn dark() -> Self {
        Self {
            scheme: ColorScheme::Dark,

            // Window colors - dark gray
            window_background: Color::rgb(40, 40, 40),
            window_border: Color::rgb(70, 70, 70),

            // Titlebar colors - gray
            titlebar_background: Color::rgb(50, 50, 60),
            titlebar_background_end: Color::rgb(50, 50, 60),
            titlebar_text: Color::rgb(220, 220, 220),
            titlebar_border: Color::rgb(70, 70, 70),

            // Titlebar control button colors
            titlebar_button_background: Color::rgb(50, 50, 60),
            titlebar_button_background_hovered: Color::rgb(70, 70, 80),
            titlebar_button_icon: Color::rgb(220, 220, 220),

            // Button colors
            button_background: Color::rgb(70, 70, 70),
            button_background_hovered: Color::rgb(85, 85, 85),
            button_background_pressed: Color::rgb(100, 100, 100),
            button_text: Color::rgb(220, 220, 220),
            button_border: Color::rgb(90, 90, 90),

            // Text colors
            text_primary: Color::rgb(220, 220, 220),
            text_secondary: Color::rgb(150, 150, 150),

            // Background colors
            background_primary: Color::rgb(45, 45, 45),
            background_secondary: Color::rgb(55, 55, 55),
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::light()
    }
}

/// Global theme storage
static CURRENT_THEME: Mutex<Option<Theme>> = Mutex::new(None);

/// Set the global theme
pub fn set_theme(theme: Theme) {
    let mut current = CURRENT_THEME.lock();
    *current = Some(theme);
}

/// Get the current global theme (defaults to Light theme if not set)
pub fn get_theme() -> Theme {
    let current = CURRENT_THEME.lock();
    match current.as_ref() {
        Some(theme) => theme.clone(),
        None => Theme::light(),
    }
}

/// Execute a closure with access to the current theme
///
/// This is the preferred way to access theme colors in components:
/// ```rust
/// use scarlet_ui::theme::with_theme;
///
/// let bg_color = with_theme(|theme| theme.window_background);
/// ```
pub fn with_theme<F, R>(f: F) -> R
where
    F: FnOnce(&Theme) -> R,
{
    let current = CURRENT_THEME.lock();
    let theme = match current.as_ref() {
        Some(t) => t,
        None => {
            // Drop lock before calling Theme::light() to avoid deadlock
            drop(current);
            // Set default theme and return it
            let default = Theme::light();
            let mut c = CURRENT_THEME.lock();
            *c = Some(default.clone());
            return f(&default);
        }
    };
    f(theme)
}

/// Initialize the theme system with a default theme
///
/// Called automatically when needed, but can be called explicitly
/// to set up the theme system early.
pub fn init() {
    let mut current = CURRENT_THEME.lock();
    if current.is_none() {
        *current = Some(Theme::light());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_theme_creation() {
        let light = Theme::light();
        assert_eq!(light.scheme, ColorScheme::Light);

        let dark = Theme::dark();
        assert_eq!(dark.scheme, ColorScheme::Dark);
    }

    #[test]
    fn test_theme_default() {
        let theme = Theme::default();
        assert_eq!(theme.scheme, ColorScheme::Light);
    }

    #[test]
    fn test_set_get_theme() {
        set_theme(Theme::light());
        let theme = get_theme();
        assert_eq!(theme.scheme, ColorScheme::Light);

        // Reset to dark
        set_theme(Theme::dark());
    }

    #[test]
    fn test_with_theme() {
        set_theme(Theme::light());

        let bg = with_theme(|theme| theme.window_background);
        let text = with_theme(|theme| theme.titlebar_text);

        assert_eq!(bg, Color::rgb(255, 255, 255));
        assert_eq!(text, Color::rgb(40, 40, 40));

        // Reset to light
        set_theme(Theme::light());
    }

    #[test]
    fn test_theme_colors_distinct() {
        let light = Theme::light();
        let dark = Theme::dark();

        // Light theme should have lighter backgrounds
        assert!(light.window_background.as_bgra()[2] > dark.window_background.as_bgra()[2]);

        // Dark theme should have lighter text
        assert!(dark.text_primary.as_bgra()[2] > light.text_primary.as_bgra()[2]);
    }

    #[test]
    fn test_init() {
        // Clear any existing theme
        {
            let mut current = CURRENT_THEME.lock().unwrap();
            *current = None;
        }

        // Init should set light theme
        init();
        let theme = get_theme();
        assert_eq!(theme.scheme, ColorScheme::Light);
    }
}
