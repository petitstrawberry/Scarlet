//! Basic UI controls
//!
//! This module provides fundamental UI components like Text, Button, Image, etc.

extern crate alloc;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::boxed::Box;

use crate::context::{ControlFlow, EventCtx, LayoutCtx, PaintCtx, UpdateCtx};
use crate::event::{Event, EventKind, MouseButton};
use crate::graphics::{Canvas, Point, Rect};
use crate::layout::{LayoutConstraints, Size};
use crate::view::id::ViewId;
use crate::view::traits::View;
use crate::color::Color;
use scarlet_std::fmt;

pub mod text;
pub mod button;
pub mod image;
pub mod text_field;
pub mod toggle;
pub mod slider;

pub use text::Text;
pub use button::{Button, ButtonAction};
pub use image::{Image, ImageData, ImageScaling};
pub use text_field::TextField;
pub use toggle::{Toggle, ToggleStyle};
pub use slider::Slider;

/// Common font configuration
#[derive(Clone, Debug)]
pub struct FontConfig {
    /// Font size in points
    pub size: u32,
    /// Font family
    pub family: String,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            size: 14,
            family: String::from("system-ui"),
        }
    }
}

/// Text alignment options
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextAlignment {
    Left,
    Center,
    Right,
}

impl Default for TextAlignment {
    fn default() -> Self {
        Self::Left
    }
}
