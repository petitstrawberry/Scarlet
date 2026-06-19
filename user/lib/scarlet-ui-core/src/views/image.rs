//! Image View - Displays an image
//!
//! Image view renders pixel data from various sources.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::any::Any;

use crate::buffer::Buffer;
use crate::color::Color;
use crate::element::{Element, ElementRenderObject, RenderElement};
use crate::geometry::Size;
use crate::view::View;
use std::fs::File;
use std::io::Read;
use zune_jpeg::JpegDecoder;
use zune_jpeg::zune_core::bytestream::ZCursor;
use zune_jpeg::zune_core::colorspace::ColorSpace;
use zune_jpeg::zune_core::options::DecoderOptions;

/// Image source
#[derive(Clone)]
pub enum ImageSource {
    /// From decoded bitmap image data
    Bitmap(BitmapImage),
    /// Solid color placeholder
    Placeholder { width: u32, height: u32 },
}

/// Decoded bitmap image data.
#[derive(Clone)]
pub struct BitmapImage {
    data: Vec<u32>,
    width: u32,
    height: u32,
}

impl BitmapImage {
    /// Create a decoded bitmap from BGRA pixel data.
    pub fn from_bgra(data: Vec<u32>, width: u32, height: u32) -> Self {
        Self {
            data,
            width,
            height,
        }
    }

    /// Decode an image file from a path.
    ///
    /// JPEG/JPG files are currently supported.
    pub fn from_path(path: &str) -> Option<Self> {
        load_image_path(path)
    }

    /// Decode a JPEG image from memory.
    pub fn from_jpeg_bytes(bytes: &[u8]) -> Option<Self> {
        decode_jpeg(bytes)
    }

    /// Get the image width in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Get the image height in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Get BGRA pixels.
    pub fn pixels(&self) -> &[u32] {
        &self.data
    }
}

/// Image View - displays an image
#[derive(Clone)]
pub struct Image {
    source: ImageSource,
    fit_mode: ImageFit,
}

impl Image {
    /// Create a new Image by decoding an image file from a path.
    ///
    /// JPEG/JPG files are currently supported. Unsupported or unreadable files
    /// render as a placeholder.
    pub fn from_path(path: impl Into<String>) -> Self {
        Self::from_path_with_placeholder_size(path, 1, 1)
    }

    /// Create a new Image from a path with a placeholder size for failed loads.
    ///
    /// JPEG/JPG files are currently supported. Unsupported or unreadable files
    /// render as a placeholder of the supplied size.
    pub fn from_path_with_placeholder_size(
        path: impl Into<String>,
        placeholder_width: u32,
        placeholder_height: u32,
    ) -> Self {
        let path = path.into();
        match BitmapImage::from_path(&path) {
            Some(image) => Self::from_bitmap(image),
            None => Self::placeholder(placeholder_width, placeholder_height),
        }
    }

    /// Create a new Image from decoded bitmap data.
    pub fn from_bitmap(image: BitmapImage) -> Self {
        Self {
            source: ImageSource::Bitmap(image),
            fit_mode: ImageFit::Contain,
        }
    }

    /// Create a new Image from raw pixel data
    pub fn from_raw(data: Vec<u32>, width: u32, height: u32) -> Self {
        Self::from_bitmap(BitmapImage::from_bgra(data, width, height))
    }

    /// Create an image from optional raw pixel data, falling back to a placeholder.
    pub fn from_optional_raw(data: Option<Vec<u32>>, width: u32, height: u32) -> Self {
        match data {
            Some(data) => Self::from_raw(data, width, height),
            None => Self::placeholder(width, height),
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
            ImageSource::Bitmap(image) => Size {
                width: image.width() as f32,
                height: image.height() as f32,
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
            self.clone(),
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
    buffer: Option<Buffer>,
}

impl ImageRenderObject {
    /// Create a new ImageRenderObject
    pub fn new(source: ImageSource, fit_mode: ImageFit) -> Self {
        Self {
            source,
            fit_mode,
            size: Size::ZERO,
            buffer: None,
        }
    }

    /// Get the intrinsic size of the image
    fn intrinsic_size(&self) -> Size {
        match &self.source {
            ImageSource::Bitmap(image) => Size {
                width: image.width() as f32,
                height: image.height() as f32,
            },
            ImageSource::Placeholder { width, height } => Size {
                width: *width as f32,
                height: *height as f32,
            },
        }
    }

    /// Calculate size based on fit mode and constraints
    fn calculate_size(
        &self,
        intrinsic: Size,
        constraints: crate::element::LayoutConstraints,
    ) -> Size {
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
                    width: intrinsic.width.max(constraints.min_width).min(
                        if constraints.max_width > 0.0 {
                            constraints.max_width
                        } else {
                            intrinsic.width
                        },
                    ),
                    height: intrinsic.height.max(constraints.min_height).min(
                        if constraints.max_height > 0.0 {
                            constraints.max_height
                        } else {
                            intrinsic.height
                        },
                    ),
                }
            }
        }
    }
}

impl ElementRenderObject for ImageRenderObject {
    fn layout(&mut self, constraints: crate::element::LayoutConstraints) -> Size {
        let intrinsic = self.intrinsic_size();
        self.size = self.calculate_size(intrinsic, constraints);
        let width = libm::ceilf(self.size.width.max(1.0)) as u32;
        let height = libm::ceilf(self.size.height.max(1.0)) as u32;
        let needs_resize = self.buffer.as_ref().map_or(true, |b| {
            b.logical_width() != width || b.logical_height() != height
        });
        if needs_resize {
            self.buffer = Some(Buffer::from_logical_dimensions(width, height));
        }
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

    fn render(&mut self) {
        if let Some(buffer) = self.buffer.as_mut() {
            match &self.source {
                ImageSource::Bitmap(image) => render_raw_image(
                    buffer,
                    image.pixels(),
                    image.width(),
                    image.height(),
                    self.fit_mode,
                ),
                ImageSource::Placeholder { .. } => buffer.clear(Color::rgb(226u8, 229u8, 235u8)),
            }
        }
    }

    fn get_buffer(&self) -> Option<&Buffer> {
        self.buffer.as_ref()
    }

    fn clear_buffer(&mut self) {
        self.buffer = None;
    }
}

fn render_raw_image(
    buffer: &mut Buffer,
    data: &[u32],
    source_width: u32,
    source_height: u32,
    fit_mode: ImageFit,
) {
    let dest_width = buffer.width();
    let dest_height = buffer.height();
    if source_width == 0 || source_height == 0 || dest_width == 0 || dest_height == 0 {
        buffer.clear(Color::TRANSPARENT);
        return;
    }

    let mut draw_x = 0u32;
    let mut draw_y = 0u32;
    let mut draw_width = dest_width;
    let mut draw_height = dest_height;

    match fit_mode {
        ImageFit::Contain | ImageFit::None => {
            let scale_x = dest_width as f32 / source_width as f32;
            let scale_y = dest_height as f32 / source_height as f32;
            let scale = scale_x.min(scale_y).max(0.001);
            draw_width = ((source_width as f32 * scale) as u32)
                .max(1)
                .min(dest_width);
            draw_height = ((source_height as f32 * scale) as u32)
                .max(1)
                .min(dest_height);
            draw_x = (dest_width - draw_width) / 2;
            draw_y = (dest_height - draw_height) / 2;
        }
        ImageFit::Cover => {
            let scale_x = dest_width as f32 / source_width as f32;
            let scale_y = dest_height as f32 / source_height as f32;
            let scale = scale_x.max(scale_y).max(0.001);
            draw_width = ((source_width as f32 * scale) as u32).max(1);
            draw_height = ((source_height as f32 * scale) as u32).max(1);
        }
        ImageFit::Fill => {}
    }

    buffer.clear(Color::TRANSPARENT);
    let dest = buffer.as_mut_slice();
    for y in 0..dest_height {
        for x in 0..dest_width {
            if x < draw_x || y < draw_y || x >= draw_x + draw_width || y >= draw_y + draw_height {
                continue;
            }
            let local_x = x - draw_x;
            let local_y = y - draw_y;
            let src_x = (local_x as u64 * source_width as u64 / draw_width as u64) as u32;
            let src_y = (local_y as u64 * source_height as u64 / draw_height as u64) as u32;
            let Some(pixel) = data.get((src_y * source_width + src_x) as usize).copied() else {
                continue;
            };
            dest[(y * dest_width + x) as usize] = pixel;
        }
    }
}

fn load_image_path(path: &str) -> Option<BitmapImage> {
    let mut file = File::open(path).ok()?;
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let read = Read::read(&mut file, &mut chunk).ok()?;
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
    decode_jpeg(&bytes)
}

fn decode_jpeg(bytes: &[u8]) -> Option<BitmapImage> {
    if !bytes.starts_with(&[0xff, 0xd8]) {
        return None;
    }
    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGBA);
    let mut decoder = JpegDecoder::new_with_options(ZCursor::new(bytes), options);
    decoder.decode_headers().ok()?;
    let (width, height) = decoder.dimensions()?;
    let rgba = decoder.decode().ok()?;
    let mut pixels = Vec::with_capacity(width.checked_mul(height)?);
    for pixel in rgba.chunks_exact(4) {
        pixels.push(
            (u32::from(pixel[3]) << 24)
                | (u32::from(pixel[0]) << 16)
                | (u32::from(pixel[1]) << 8)
                | u32::from(pixel[2]),
        );
    }
    Some(BitmapImage::from_bgra(pixels, width as u32, height as u32))
}
