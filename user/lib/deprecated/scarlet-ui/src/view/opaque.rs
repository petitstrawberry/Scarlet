//! Opaque view implementations
//!
//! This module provides implementations of the Opaque trait for common view types.
//! Opaque views draw an opaque background, which allows the framework to skip
//! drawing the background behind them. This is an important optimization.

use crate::color::Color;

/// Helper for determining if a color is opaque
pub fn color_is_opaque(color: &Color) -> bool {
    color.a == 255
}

/// Helper for determining if a background color makes a view opaque
pub fn background_is_opaque(background: Option<&Color>) -> bool {
    match background {
        None => false,
        Some(color) => color_is_opaque(color),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_color_opaque() {
        let opaque = Color::rgb(255, 0, 0);
        assert!(opaque.is_opaque());
    }

    #[test]
    fn test_color_transparent() {
        let transparent = Color::rgba(255, 0, 0, 128);
        assert!(!transparent.is_opaque());
    }

    #[test]
    fn test_color_fully_transparent() {
        let fully_transparent = Color::rgba(255, 0, 0, 0);
        assert!(!fully_transparent.is_opaque());
    }

    #[test]
    fn test_background_is_opaque_some() {
        let background = Some(Color::rgb(255, 255, 255));
        assert!(background_is_opaque(background.as_ref()));
    }

    #[test]
    fn test_background_is_opaque_none() {
        assert!(!background_is_opaque(None));
    }

    #[test]
    fn test_background_is_opaque_transparent() {
        let background = Some(Color::rgba(255, 255, 255, 128));
        assert!(!background_is_opaque(background.as_ref()));
    }
}
