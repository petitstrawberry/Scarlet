//! Image control
//!
//! Displays an image with various sizing options.

extern crate alloc;
use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::context::{ControlFlow, EventCtx, LayoutCtx, PaintCtx, UpdateCtx};
use crate::event::Event;
use crate::graphics::Rect;
use crate::layout::{LayoutConstraints, Size};
use crate::view::id::ViewId;
use crate::view::traits::View;
use scarlet_std::fmt;

/// Image data
#[derive(Clone)]
pub struct ImageData {
    /// Pixel data in RGBA format
    pub data: Vec<u8>,
    /// Image width
    pub width: u32,
    /// Image height
    pub height: u32,
}

impl ImageData {
    /// Create new image data
    pub fn new(data: Vec<u8>, width: u32, height: u32) -> Self {
        Self { data, width, height }
    }

    /// Get image size
    pub fn size(&self) -> Size {
        Size::new(self.width, self.height)
    }
}

/// Image scaling mode
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageScaling {
    /// Scale to fill, maintaining aspect ratio
    Fit,
    /// Scale to fill, cropping if necessary
    Fill,
    /// Stretch to fill exactly
    Stretch,
    /// Center without scaling
    None,
}

impl Default for ImageScaling {
    fn default() -> Self {
        Self::Fit
    }
}

/// Image view
pub struct Image {
    id: ViewId,
    image_data: Option<ImageData>,
    scaling: ImageScaling,
    cached_size: Size,
}

impl Image {
    /// Create a new empty image view
    pub fn new() -> Self {
        Self {
            id: ViewId::new(),
            image_data: None,
            scaling: ImageScaling::default(),
            cached_size: Size::ZERO,
        }
    }

    /// Create an image view with data
    pub fn with_data(data: ImageData) -> Self {
        let size = data.size();
        Self {
            id: ViewId::new(),
            image_data: Some(data),
            scaling: ImageScaling::default(),
            cached_size: size,
        }
    }

    /// Set the image data
    pub fn set_data(&mut self, data: ImageData) {
        self.image_data = Some(data);
    }

    /// Clear the image data
    pub fn clear(&mut self) {
        self.image_data = None;
        self.cached_size = Size::ZERO;
    }

    /// Get the image data
    pub fn data(&self) -> Option<&ImageData> {
        self.image_data.as_ref()
    }

    /// Set the scaling mode
    pub fn set_scaling(&mut self, scaling: ImageScaling) {
        self.scaling = scaling;
    }

    /// Get the scaling mode
    pub fn scaling(&self) -> ImageScaling {
        self.scaling
    }

    /// Calculate the destination rect for the image
    fn calculate_dest_rect(&self, frame: Rect) -> Rect {
        let Some(image_data) = &self.image_data else {
            return frame;
        };

        let image_size = image_data.size();
        let _frame_size = Size::new(frame.width, frame.height);

        match self.scaling {
            ImageScaling::Stretch => frame,
            ImageScaling::None => {
                // Center the image without scaling
                let x = frame.x + (frame.width as i32 - image_size.width as i32).max(0) / 2;
                let y = frame.y + (frame.height as i32 - image_size.height as i32).max(0) / 2;
                Rect::new(x, y, image_size.width, image_size.height)
            }
            ImageScaling::Fit => {
                // Scale to fit, maintaining aspect ratio
                let scale_x = frame.width as f32 / image_size.width as f32;
                let scale_y = frame.height as f32 / image_size.height as f32;
                let scale = scale_x.min(scale_y);

                let scaled_width = (image_size.width as f32 * scale) as u32;
                let scaled_height = (image_size.height as f32 * scale) as u32;

                let x = frame.x + (frame.width as i32 - scaled_width as i32).max(0) / 2;
                let y = frame.y + (frame.height as i32 - scaled_height as i32).max(0) / 2;

                Rect::new(x, y, scaled_width, scaled_height)
            }
            ImageScaling::Fill => {
                // Scale to fill, maintaining aspect ratio, cropping if necessary
                let scale_x = frame.width as f32 / image_size.width as f32;
                let scale_y = frame.height as f32 / image_size.height as f32;
                let scale = scale_x.max(scale_y);

                let scaled_width = (image_size.width as f32 * scale) as u32;
                let scaled_height = (image_size.height as f32 * scale) as u32;

                let x = frame.x - (scaled_width as i32 - frame.width as i32).min(0) / 2;
                let y = frame.y - (scaled_height as i32 - frame.height as i32).min(0) / 2;

                Rect::new(x, y, scaled_width, scaled_height)
            }
        }
    }
}

impl Default for Image {
    fn default() -> Self {
        Self::new()
    }
}

impl crate::view::render::RenderObject for Image {
    fn id(&self) -> ViewId {
        self.id
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn layout(&mut self, _ctx: &mut LayoutCtx, constraints: LayoutConstraints) -> Size {
        if let Some(image_data) = &self.image_data {
            let image_size = image_data.size();

            // Calculate size based on constraints
            let width = if constraints.max_width > 0 {
                image_size.width.min(constraints.max_width)
            } else {
                image_size.width
            };

            let height = if constraints.max_height > 0 {
                image_size.height.min(constraints.max_height)
            } else {
                image_size.height
            };

            let size = Size::new(
                width.max(constraints.min_width),
                height.max(constraints.min_height),
            );

            self.cached_size = size;
            size
        } else {
            let size = Size::new(
                constraints.min_width.max(0),
                constraints.min_height.max(0),
            );
            self.cached_size = size;
            size
        }
    }

    fn draw(&self, ctx: &mut PaintCtx, frame: Rect) {
        // TODO: Implement actual image drawing
        let _ = (ctx, frame);

        if self.image_data.is_some() {
            let dest_rect = self.calculate_dest_rect(frame);
            let _ = dest_rect;
        }
    }

    fn event(&mut self, _ctx: &mut EventCtx, _event: &Event) -> ControlFlow {
        ControlFlow::Continue
    }

    fn update(&mut self, _ctx: &mut UpdateCtx) {
        // Image doesn't need periodic updates
    }
}

impl fmt::Debug for Image {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Image")
            .field("has_data", &self.image_data.is_some())
            .field("scaling", &self.scaling)
            .field("cached_size", &self.cached_size)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_image_new() {
        let image = Image::new();
        assert!(image.data().is_none());
    }

    #[test]
    fn test_image_with_data() {
        let data = vec![0; 100 * 100 * 4];
        let image_data = ImageData::new(data, 100, 100);
        let image = Image::with_data(image_data.clone());

        assert!(image.data().is_some());
        assert_eq!(image.data().unwrap().width, 100);
        assert_eq!(image.data().unwrap().height, 100);
    }

    #[test]
    fn test_image_clear() {
        let data = vec![0; 100 * 100 * 4];
        let image_data = ImageData::new(data, 100, 100);
        let mut image = Image::with_data(image_data);

        image.clear();
        assert!(image.data().is_none());
    }

    #[test]
    fn test_image_scaling() {
        let mut image = Image::new();
        image.set_scaling(ImageScaling::Stretch);
        assert_eq!(image.scaling(), ImageScaling::Stretch);
    }
}
