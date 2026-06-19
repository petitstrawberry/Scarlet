//! macOS-style system colors

use crate::color::Color;

/// macOS-style system color palette
///
/// Provides predefined color categories similar to macOS system colors.
/// These colors are adapted for both light and dark schemes.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct SystemColors {
    /// Gray scale colors
    pub gray: GrayColors,
    /// Blue scale colors
    pub blue: BlueColors,
    /// Green scale colors
    pub green: GreenColors,
    /// Orange scale colors
    pub orange: OrangeColors,
    /// Pink scale colors
    pub pink: PinkColors,
    /// Purple scale colors
    pub purple: PurpleColors,
    /// Red scale colors
    pub red: RedColors,
    /// Yellow scale colors
    pub yellow: YellowColors,
}

/// Gray scale colors
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct GrayColors {
    pub base: Color,
    pub light: Color,
    pub dark: Color,
}

/// Blue scale colors
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct BlueColors {
    pub base: Color,
    pub light: Color,
    pub dark: Color,
}

/// Green scale colors
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct GreenColors {
    pub base: Color,
    pub light: Color,
    pub dark: Color,
}

/// Orange scale colors
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct OrangeColors {
    pub base: Color,
    pub light: Color,
    pub dark: Color,
}

/// Pink scale colors
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct PinkColors {
    pub base: Color,
    pub light: Color,
    pub dark: Color,
}

/// Purple scale colors
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct PurpleColors {
    pub base: Color,
    pub light: Color,
    pub dark: Color,
}

/// Red scale colors
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct RedColors {
    pub base: Color,
    pub light: Color,
    pub dark: Color,
}

/// Yellow scale colors
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct YellowColors {
    pub base: Color,
    pub light: Color,
    pub dark: Color,
}

impl SystemColors {
    /// Create system colors for light scheme
    pub fn light() -> Self {
        Self {
            gray: GrayColors {
                base: Color::rgb(0.56, 0.56, 0.58),
                light: Color::rgb(0.78, 0.78, 0.78),
                dark: Color::rgb(0.31, 0.31, 0.31),
            },
            blue: BlueColors {
                base: Color::rgb(0.0, 0.48, 1.0),
                light: Color::rgb(0.39, 0.71, 1.0),
                dark: Color::rgb(0.0, 0.31, 0.71),
            },
            green: GreenColors {
                base: Color::rgb(0.2, 0.78, 0.35),
                light: Color::rgb(0.47, 0.9, 0.55),
                dark: Color::rgb(0.12, 0.59, 0.24),
            },
            orange: OrangeColors {
                base: Color::rgb(1.0, 0.58, 0.0),
                light: Color::rgb(1.0, 0.75, 0.39),
                dark: Color::rgb(0.78, 0.43, 0.0),
            },
            pink: PinkColors {
                base: Color::rgb(1.0, 0.18, 0.33),
                light: Color::rgb(1.0, 0.47, 0.59),
                dark: Color::rgb(0.78, 0.12, 0.24),
            },
            purple: PurpleColors {
                base: Color::rgb(0.69, 0.32, 0.87),
                light: Color::rgb(0.82, 0.55, 0.94),
                dark: Color::rgb(0.51, 0.2, 0.71),
            },
            red: RedColors {
                base: Color::rgb(1.0, 0.23, 0.19),
                light: Color::rgb(1.0, 0.51, 0.47),
                dark: Color::rgb(0.78, 0.16, 0.12),
            },
            yellow: YellowColors {
                base: Color::rgb(1.0, 0.8, 0.0),
                light: Color::rgb(1.0, 0.9, 0.47),
                dark: Color::rgb(0.78, 0.63, 0.0),
            },
        }
    }

    /// Create system colors for dark scheme
    pub fn dark() -> Self {
        Self {
            gray: GrayColors {
                base: Color::rgb(0.55, 0.55, 0.57),
                light: Color::rgb(0.71, 0.71, 0.73),
                dark: Color::rgb(0.35, 0.35, 0.37),
            },
            blue: BlueColors {
                base: Color::rgb(0.04, 0.52, 1.0),
                light: Color::rgb(0.39, 0.71, 1.0),
                dark: Color::rgb(0.0, 0.35, 0.78),
            },
            green: GreenColors {
                base: Color::rgb(0.27, 0.84, 0.39),
                light: Color::rgb(0.51, 0.92, 0.59),
                dark: Color::rgb(0.16, 0.67, 0.27),
            },
            orange: OrangeColors {
                base: Color::rgb(1.0, 0.65, 0.16),
                light: Color::rgb(1.0, 0.78, 0.47),
                dark: Color::rgb(0.82, 0.51, 0.08),
            },
            pink: PinkColors {
                base: Color::rgb(1.0, 0.25, 0.39),
                light: Color::rgb(1.0, 0.51, 0.63),
                dark: Color::rgb(0.82, 0.16, 0.29),
            },
            purple: PurpleColors {
                base: Color::rgb(0.73, 0.36, 0.91),
                light: Color::rgb(0.86, 0.59, 0.96),
                dark: Color::rgb(0.55, 0.24, 0.75),
            },
            red: RedColors {
                base: Color::rgb(1.0, 0.31, 0.27),
                light: Color::rgb(1.0, 0.55, 0.51),
                dark: Color::rgb(0.82, 0.2, 0.16),
            },
            yellow: YellowColors {
                base: Color::rgb(1.0, 0.84, 0.12),
                light: Color::rgb(1.0, 0.92, 0.55),
                dark: Color::rgb(0.82, 0.71, 0.08),
            },
        }
    }
}
