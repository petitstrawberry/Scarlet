//! Color type with internal BGRA representation
//!
//! The internal representation is BGRA to match the framebuffer format,
//! while the interface uses RGBA for intuition and compatibility.

/// Color with internal BGRA representation
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Color {
    // Internal BGRA representation (matches framebuffer format)
    b: u8,
    g: u8,
    r: u8,
    a: u8,
}

impl Color {
    /// Create color from RGBA components
    pub fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { b, g, r, a }
    }

    /// Create color from RGB components (alpha = 255)
    pub fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { b, g, r, a: 255 }
    }

    /// Get as BGRA array (internal representation)
    pub fn as_bgra(&self) -> [u8; 4] {
        [self.b, self.g, self.r, self.a]
    }

    // Common colors
    pub const BLACK: Color = Color { b: 0, g: 0, r: 0, a: 255 };
    pub const WHITE: Color = Color { b: 255, g: 255, r: 255, a: 255 };
    pub const RED: Color = Color { b: 0, g: 0, r: 255, a: 255 };
    pub const GREEN: Color = Color { b: 0, g: 255, r: 0, a: 255 };
    pub const BLUE: Color = Color { b: 255, g: 0, r: 0, a: 255 };
    pub const GRAY: Color = Color { b: 128, g: 128, r: 128, a: 255 };

    /// Light gray background (from design system)
    pub const WINDOW_BG: Color = Color { b: 247, g: 242, r: 242, a: 255 };
}

impl Default for Color {
    fn default() -> Self {
        Self::BLACK
    }
}

// Conversion from [u8; 4] RGBA array to Color
impl From<[u8; 4]> for Color {
    fn from(rgba: [u8; 4]) -> Self {
        Self::rgba(rgba[0], rgba[1], rgba[2], rgba[3])
    }
}
