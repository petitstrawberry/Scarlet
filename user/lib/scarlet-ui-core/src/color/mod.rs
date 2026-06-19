//! Color palette system for ScarletUI
//!
//! This module provides a comprehensive color palette system with:
//! - Light/Dark mode support
//! - Semantic colors for UI elements
//! - macOS-style system colors
//! - Window-specific colors

pub mod base;
pub mod palette;
pub mod scheme;
pub mod semantic;
pub mod system;

pub use base::{Color, ColorComponent};
pub use palette::ColorPalette;
pub use scheme::ColorScheme;
pub use semantic::SemanticColor;
pub use system::SystemColors;
