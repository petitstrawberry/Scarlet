//! Image View - Displays an image
//!
//! Image view renders pixel data from various sources.

use alloc::vec::Vec;
use core::any::Any;
use crate::view::View;
use crate::element::{Element, RenderElement, ElementRenderObject};
use crate::geometry::Size;
use alloc::boxed::Box;

/// Image source
#[derive(Clone)]
pub enum ImageSource {
    /// From raw pixel data (BGRA format)
    Raw { data: Vec<u32>, width: u32, height: u32 },
    /// Solid color placeholder
    Placeholder { width: u32, height: u32 },
}

/// Image View - displays an image
#[derive(Clone)]
pub struct Image {
    source: ImageSource,
    fit_mode: ImageFit,
}

impl Image {
    /// Create a new Image from raw pixel data
    pub fn from_raw(data: Vec<u32>, width: u32, height: u32) -> Self {
        Self {
            source: ImageSource::Raw { data, width, height },
            fit_mode: ImageFit::Contain,
        }
    }

    /// Create a placeholder image with specified dimensions
    pub fn placeholder(width: u32, height: u32) -> Self {
        Self {
            source: ImageSource::Placeholder { width, height },
            fit_mode: ImageFit::Contain,
        }
    }

    /// Set how the image should fit within its frame
    pub fn fit_mode(mut self, mode: ImageFit) -> Self {
        self.fit_mode = mode;
        self
    }

    /// Get the intrinsic size of the image
    fn intrinsic_size(&self) -> Size {
        match &self.source {
            ImageSource::Raw { width, height, .. } => Size {
                width: *width as f32,
                height: *height as f32,
            },
            ImageSource::Placeholder { width, height } => Size {
                width: *width as f32,
                height: *height as f32,
            },
        }
    }
}

impl View for Image {
    fn create_element(&self) -> Box<dyn Element> {
        Box::new(RenderElement::new(
            ImageRenderObject::new(self.source.clone(), self.fit_mode),
        ))
    }

    fn listenables(&self) -> alloc::vec::Vec<&dyn crate::state::Listenable> {
        alloc::vec::Vec::new()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// How the image should fit within its frame
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ImageFit {
    /// Scale the image to fit within the frame while preserving aspect ratio
    Contain,
    /// Scale the image to fill the frame while preserving aspect ratio
    Cover,
    /// Stretch the image to fill the frame
    Fill,
    /// Display the image at its intrinsic size
    None,
}

/// Image RenderObject - handles image rendering
pub struct ImageRenderObject {
    source: ImageSource,
    fit_mode: ImageFit,
    size: Size,
}

impl ImageRenderObject {
    /// Create a new ImageRenderObject
    pub fn new(source: ImageSource, fit_mode: ImageFit) -> Self {
        Self {
            source,
            fit_mode,
            size: Size::ZERO,
        }
    }

    /// Get the intrinsic size of the image
    fn intrinsic_size(&self) -> Size {
        match &self.source {
            ImageSource::Raw { width, height, .. } => Size {
                width: *width as f32,
                height: *height as f32,
            },
            ImageSource::Placeholder { width, height } => Size {
                width: *width as f32,
                height: *height as f32,
            },
        }
    }

    /// Calculate size based on fit mode and constraints
    fn calculate_size(&self, intrinsic: Size, constraints: crate::element::LayoutConstraints) -> Size {
        match self.fit_mode {
            ImageFit::Contain => {
                // Scale to fit within constraints while preserving aspect ratio
                if constraints.max_width > 0.0 && constraints.max_height > 0.0 {
                    let scale_x = constraints.max_width / intrinsic.width;
                    let scale_y = constraints.max_height / intrinsic.height;
                    let scale = scale_x.min(scale_y).min(1.0);

                    Size {
                        width: (intrinsic.width * scale).max(constraints.min_width),
                        height: (intrinsic.height * scale).max(constraints.min_height),
                    }
                } else {
                    Size {
                        width: intrinsic.width.max(constraints.min_width),
                        height: intrinsic.height.max(constraints.min_height),
                    }
                }
            }
            ImageFit::Cover => {
                // Scale to cover constraints while preserving aspect ratio
                if constraints.max_width > 0.0 && constraints.max_height > 0.0 {
                    let scale_x = constraints.max_width / intrinsic.width;
                    let scale_y = constraints.max_height / intrinsic.height;
                    let scale = scale_x.max(scale_y).max(1.0);

                    Size {
                        width: (intrinsic.width * scale).max(constraints.min_width),
                        height: (intrinsic.height * scale).max(constraints.min_height),
                    }
                } else {
                    Size {
                        width: intrinsic.width.max(constraints.min_width),
                        height: intrinsic.height.max(constraints.min_height),
                    }
                }
            }
            ImageFit::Fill => {
                // Stretch to fill constraints
                Size {
                    width: if constraints.max_width > 0.0 {
                        constraints.max_width.max(constraints.min_width)
                    } else {
                        constraints.min_width.max(intrinsic.width)
                    },
                    height: if constraints.max_height > 0.0 {
                        constraints.max_height.max(constraints.min_height)
                    } else {
                        constraints.min_height.max(intrinsic.height)
                    },
                }
            }
            ImageFit::None => {
                // Use intrinsic size, clamped to constraints
                Size {
                    width: intrinsic.width
                        .max(constraints.min_width)
                        .min(if constraints.max_width > 0.0 { constraints.max_width } else { intrinsic.width }),
                    height: intrinsic.height
                        .max(constraints.min_height)
                        .min(if constraints.max_height > 0.0 { constraints.max_height } else { intrinsic.height }),
                }
            }
        }
    }
}

impl ElementRenderObject for ImageRenderObject {
    fn layout(&mut self, constraints: crate::element::LayoutConstraints) -> Size {
        let intrinsic = self.intrinsic_size();
        self.size = self.calculate_size(intrinsic, constraints);
        self.size
    }

    fn size(&self) -> Size {
        self.size
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
