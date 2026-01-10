//! Color definitions and utilities

/// RGBA color
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// Convert to BGRA format (native framebuffer format)
    pub fn to_bgra(&self) -> [u8; 4] {
        [self.b, self.g, self.r, self.a]
    }

    // Common colors
    pub const BLACK: Color = Color::rgb(0, 0, 0);
    pub const WHITE: Color = Color::rgb(255, 255, 255);
    pub const RED: Color = Color::rgb(255, 0, 0);
    pub const GREEN: Color = Color::rgb(0, 255, 0);
    pub const BLUE: Color = Color::rgb(0, 0, 255);
    pub const GRAY: Color = Color::rgb(128, 128, 128);
    pub const LIGHT_GRAY: Color = Color::rgb(192, 192, 192);
    pub const DARK_GRAY: Color = Color::rgb(64, 64, 64);
    
    // UI theme colors
    pub const WINDOW_BG: Color = Color::rgb(240, 240, 240);
    pub const TITLEBAR: Color = Color::rgb(45, 125, 210);
    pub const TITLEBAR_TEXT: Color = Color::WHITE;
    pub const BORDER: Color = Color::rgb(100, 100, 100);
    pub const BUTTON_NORMAL: Color = Color::rgb(225, 225, 225);
    pub const BUTTON_HOVER: Color = Color::rgb(210, 210, 210);
    pub const BUTTON_PRESSED: Color = Color::rgb(180, 180, 180);
    pub const TEXT: Color = Color::BLACK;
}
