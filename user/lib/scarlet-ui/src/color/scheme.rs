//! Color scheme enumeration

/// Color scheme for the application theme
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum ColorScheme {
    /// Light color scheme (default)
    Light,
    /// Dark color scheme
    Dark,
}

impl Default for ColorScheme {
    fn default() -> Self {
        Self::Light
    }
}
