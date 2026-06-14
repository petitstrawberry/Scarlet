//! Color types and utilities for ScarletUI

/// Trait for types that can be converted to a color component (0.0 - 1.0)
pub trait ColorComponent: Copy {
    /// Convert to a color component in the range 0.0 - 1.0
    fn into_color_component(self) -> f32;
}

impl ColorComponent for u8 {
    fn into_color_component(self) -> f32 {
        self as f32 / 255.0
    }
}

impl ColorComponent for i32 {
    fn into_color_component(self) -> f32 {
        (self.clamp(0, 255) as f32) / 255.0
    }
}

impl ColorComponent for f32 {
    fn into_color_component(self) -> f32 {
        self
    }
}

/// RGBA color with floating-point components (0.0 - 1.0)
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    /// Create a new RGB color (alpha = 1.0)
    /// Accepts u8 (0-255) or f32 (0.0-1.0) via ColorComponent trait
    pub fn rgb<T1: ColorComponent, T2: ColorComponent, T3: ColorComponent>(
        r: T1,
        g: T2,
        b: T3,
    ) -> Self {
        Self {
            r: r.into_color_component(),
            g: g.into_color_component(),
            b: b.into_color_component(),
            a: 1.0,
        }
    }

    /// Create a new RGB color (alpha = 1.0) - const fn version for f32 only
    pub const fn rgb_f32(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b, a: 1.0 }
    }

    /// Create a new RGBA color
    /// Accepts u8 (0-255) or f32 (0.0-1.0) via ColorComponent trait
    pub fn rgba<T1: ColorComponent, T2: ColorComponent, T3: ColorComponent, T4: ColorComponent>(
        r: T1,
        g: T2,
        b: T3,
        a: T4,
    ) -> Self {
        Self {
            r: r.into_color_component(),
            g: g.into_color_component(),
            b: b.into_color_component(),
            a: a.into_color_component(),
        }
    }

    /// Create a new RGBA color - const fn version for f32 only
    pub const fn rgba_f32(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    /// Create a grayscale color
    pub const fn gray(gray: f32) -> Self {
        Self {
            r: gray,
            g: gray,
            b: gray,
            a: 1.0,
        }
    }

    /// Black color
    pub const BLACK: Self = Self::rgb_f32(0.0, 0.0, 0.0);

    /// White color
    pub const WHITE: Self = Self::rgb_f32(1.0, 1.0, 1.0);

    /// Red color
    pub const RED: Self = Self::rgb_f32(1.0, 0.0, 0.0);

    /// Green color
    pub const GREEN: Self = Self::rgb_f32(0.0, 1.0, 0.0);

    /// Blue color
    pub const BLUE: Self = Self::rgb_f32(0.0, 0.0, 1.0);

    /// Yellow color
    pub const YELLOW: Self = Self::rgb_f32(1.0, 1.0, 0.0);

    /// Cyan color
    pub const CYAN: Self = Self::rgb_f32(0.0, 1.0, 1.0);

    /// Magenta color
    pub const MAGENTA: Self = Self::rgb_f32(1.0, 0.0, 1.0);

    /// Transparent color
    pub const TRANSPARENT: Self = Self::rgba_f32(0.0, 0.0, 0.0, 0.0);

    /// Clear/transparent color (alias for TRANSPARENT)
    pub const CLEAR: Self = Self::TRANSPARENT;

    /// Convert color to 32-bit BGRA format (for framebuffers)
    pub fn to_bgra(&self) -> u32 {
        let r = (self.r * 255.0).clamp(0.0, 255.0) as u8;
        let g = (self.g * 255.0).clamp(0.0, 255.0) as u8;
        let b = (self.b * 255.0).clamp(0.0, 255.0) as u8;
        let a = (self.a * 255.0).clamp(0.0, 255.0) as u8;

        ((a as u32) << 24) | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
    }

    /// Convert color to 32-bit RGBA format
    pub fn to_rgba(&self) -> u32 {
        let r = (self.r * 255.0).clamp(0.0, 255.0) as u8;
        let g = (self.g * 255.0).clamp(0.0, 255.0) as u8;
        let b = (self.b * 255.0).clamp(0.0, 255.0) as u8;
        let a = (self.a * 255.0).clamp(0.0, 255.0) as u8;

        ((r as u32) << 24) | ((g as u32) << 16) | ((b as u32) << 8) | (a as u32)
    }

    /// Convert from 32-bit BGRA format
    pub fn from_bgra(bgra: u32) -> Self {
        let a = ((bgra >> 24) & 0xFF) as u8;
        let r = ((bgra >> 16) & 0xFF) as u8;
        let g = ((bgra >> 8) & 0xFF) as u8;
        let b = (bgra & 0xFF) as u8;

        Self {
            r: r as f32 / 255.0,
            g: g as f32 / 255.0,
            b: b as f32 / 255.0,
            a: a as f32 / 255.0,
        }
    }

    /// Apply alpha blending over a background color
    pub fn blend_over(&self, background: Color) -> Color {
        let src_a = self.a;
        let dst_a = background.a;

        if src_a >= 1.0 {
            *self
        } else if src_a <= 0.0 {
            background
        } else {
            let out_a = src_a + dst_a * (1.0 - src_a);
            let src_r = self.r * src_a;
            let src_g = self.g * src_a;
            let src_b = self.b * src_a;
            let dst_r = background.r * dst_a;
            let dst_g = background.g * dst_a;
            let dst_b = background.b * dst_a;

            Color {
                r: (src_r + dst_r * (1.0 - src_a)) / out_a,
                g: (src_g + dst_g * (1.0 - src_a)) / out_a,
                b: (src_b + dst_b * (1.0 - src_a)) / out_a,
                a: out_a,
            }
        }
    }

    /// Lighten the color by a factor (0.0 - 1.0)
    pub fn lighten(&self, factor: f32) -> Color {
        Color {
            r: (self.r + factor).clamp(0.0, 1.0),
            g: (self.g + factor).clamp(0.0, 1.0),
            b: (self.b + factor).clamp(0.0, 1.0),
            a: self.a,
        }
    }

    /// Darken the color by a factor (0.0 - 1.0)
    pub fn darken(&self, factor: f32) -> Color {
        Color {
            r: (self.r - factor).clamp(0.0, 1.0),
            g: (self.g - factor).clamp(0.0, 1.0),
            b: (self.b - factor).clamp(0.0, 1.0),
            a: self.a,
        }
    }

    /// Get color opacity
    pub fn opacity(&self) -> f32 {
        self.a
    }

    /// Set color opacity
    pub fn with_opacity(&self, opacity: f32) -> Color {
        Color {
            r: self.r,
            g: self.g,
            b: self.b,
            a: opacity.clamp(0.0, 1.0),
        }
    }
}
