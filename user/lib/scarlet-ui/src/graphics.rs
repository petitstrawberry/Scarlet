//! Graphics primitives and drawing utilities

use std::println;

use ab_glyph::{point, Font, FontRef, Glyph, InvalidFont, PxScale, ScaleFont};
use crate::Color;

extern crate scarlet_std as std;
extern crate alloc;

use alloc::{boxed::Box, vec::Vec};

// Rootfs-provided default font (MPLUS_FONTS, OFL-1.1).
const DEFAULT_FONT_PATH: &str = "/fonts/Mplus1-Regular.ttf";

#[derive(Clone)]
struct DefaultFontState {
    font: Option<FontRef<'static>>,
    load_attempted: bool,
}

static DEFAULT_FONT: std::sync::Mutex<DefaultFontState> =
    std::sync::Mutex::new(DefaultFontState {
        font: None,
        load_attempted: false,
    });

/// Set the default UI font used by [`Canvas::draw_text`] and widgets like `Label`.
///
/// Scarlet UI keeps the font bytes alive for the rest of the process.
pub fn set_default_font(font_bytes: &'static [u8]) -> Result<(), InvalidFont> {
    let font = FontRef::try_from_slice(font_bytes)?;
    let mut state = DEFAULT_FONT.lock();
    state.font = Some(font);
    Ok(())
}

fn set_default_font_owned(font_bytes: Vec<u8>) -> Result<(), InvalidFont> {
    let leaked: &'static [u8] = Box::leak(font_bytes.into_boxed_slice());
    set_default_font(leaked)
}

fn load_default_font_from_rootfs_once() {
    let should_try = {
        let mut state = DEFAULT_FONT.lock();
        if state.font.is_some() || state.load_attempted {
            false
        } else {
            state.load_attempted = true;
            true
        }
    };

    if !should_try {
        return;
    }

    let mut file = match std::fs::File::open(DEFAULT_FONT_PATH) {
        Ok(f) => f,
        Err(e) => {
            println!("scarlet-ui: Failed to open default font '{}': {:?}", DEFAULT_FONT_PATH, e);
            return;
        }
    };

    let mut bytes = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let n = match file.read(&mut buf) {
            Ok(n) => n,
            Err(_) => return,
        };
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&buf[..n]);
    }

    let _ = set_default_font_owned(bytes);
}

fn default_font() -> Option<FontRef<'static>> {
    load_default_font_from_rootfs_once();
    DEFAULT_FONT.lock().font.clone()
}

/// 2D point
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

impl Point {
    /// The origin point (0, 0)
    pub const ZERO: Self = Self { x: 0, y: 0 };

    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}

/// Rectangle
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x
            && x < self.x + self.width as i32
            && y >= self.y
            && y < self.y + self.height as i32
    }

    pub fn contains_point(&self, point: Point) -> bool {
        self.contains(point.x, point.y)
    }
}

/// Canvas for drawing operations
pub struct Canvas<'a> {
    buffer: &'a mut [u8],
    width: u32,
    height: u32,
}

impl<'a> Canvas<'a> {
    /// Create a new canvas from a BGRA buffer
    pub fn new(buffer: &'a mut [u8], width: u32, height: u32) -> Self {
        Self {
            buffer,
            width,
            height,
        }
    }

    /// Draw a single pixel
    pub fn put_pixel(&mut self, x: i32, y: i32, color: Color) {
        if x < 0 || x >= self.width as i32 || y < 0 || y >= self.height as i32 {
            return;
        }

        let offset = ((y as u32 * self.width + x as u32) * 4) as usize;
        if offset + 4 <= self.buffer.len() {
            let bgra = color.to_bgra();
            self.buffer[offset..offset + 4].copy_from_slice(&bgra);
        }
    }

    fn get_pixel(&self, x: i32, y: i32) -> Color {
        if x < 0 || x >= self.width as i32 || y < 0 || y >= self.height as i32 {
            return Color::BLACK;
        }
        let offset = ((y as u32 * self.width + x as u32) * 4) as usize;
        if offset + 4 > self.buffer.len() {
            return Color::BLACK;
        }
        let b = self.buffer[offset];
        let g = self.buffer[offset + 1];
        let r = self.buffer[offset + 2];
        Color::rgb(r, g, b)
    }

    fn put_pixel_alpha(&mut self, x: i32, y: i32, color: Color, alpha: f32) {
        if alpha <= 0.0 {
            return;
        }
        if alpha >= 1.0 {
            self.put_pixel(x, y, color);
            return;
        }
        let dst = self.get_pixel(x, y);
        let inv = 1.0 - alpha;
        let r = (dst.r as f32 * inv + color.r as f32 * alpha) as u8;
        let g = (dst.g as f32 * inv + color.g as f32 * alpha) as u8;
        let b = (dst.b as f32 * inv + color.b as f32 * alpha) as u8;
        self.put_pixel(x, y, Color::rgb(r, g, b));
    }

    /// Fill a rectangle with a solid color
    pub fn fill_rect(&mut self, x: i32, y: i32, width: u32, height: u32, color: Color) {
        let bgra = color.to_bgra();

        for dy in 0..height {
            for dx in 0..width {
                let px = x + dx as i32;
                let py = y + dy as i32;

                if px < 0 || px >= self.width as i32 || py < 0 || py >= self.height as i32 {
                    continue;
                }

                let offset = ((py as u32 * self.width + px as u32) * 4) as usize;
                if offset + 4 <= self.buffer.len() {
                    self.buffer[offset..offset + 4].copy_from_slice(&bgra);
                }
            }
        }
    }

    /// Fill a Rect with a solid color
    pub fn fill(&mut self, rect: Rect, color: Color) {
        self.fill_rect(rect.x, rect.y, rect.width, rect.height, color);
    }

    /// Draw a rectangle outline
    pub fn draw_rect(&mut self, x: i32, y: i32, width: u32, height: u32, color: Color) {
        // Top and bottom edges
        for dx in 0..width {
            self.put_pixel(x + dx as i32, y, color);
            self.put_pixel(x + dx as i32, y + height as i32 - 1, color);
        }

        // Left and right edges
        for dy in 0..height {
            self.put_pixel(x, y + dy as i32, color);
            self.put_pixel(x + width as i32 - 1, y + dy as i32, color);
        }
    }

    /// Draw a Rect outline
    pub fn stroke(&mut self, rect: Rect, color: Color) {
        self.draw_rect(rect.x, rect.y, rect.width, rect.height, color);
    }

    /// Draw text using the global default vector font.
    ///
    /// `x,y` is the **top-left** of the text line.
    pub fn draw_text(&mut self, x: i32, y: i32, text: &str, color: Color) {
        self.draw_text_sized(x, y, text, color, 16.0);
    }

    /// Draw text using the global default vector font with an explicit pixel size.
    ///
    /// `x,y` is the **top-left** of the text line.
    pub fn draw_text_sized(&mut self, x: i32, y: i32, text: &str, color: Color, font_size_px: f32) {
        let Some(font) = default_font() else {
            // No font configured; nothing to draw.
            return;
        };

        let scale = PxScale::from(font_size_px);
        let scaled = font.as_scaled(scale);

        let mut caret_x = x as f32;
        // Convert top-left y to baseline y.
        let mut caret_y = y as f32 + scaled.ascent();

        for ch in text.chars() {
            if ch == '\n' {
                caret_x = x as f32;
                caret_y += scaled.height() + scaled.line_gap();
                continue;
            }

            let glyph_id = scaled.glyph_id(ch);
            let glyph: Glyph = glyph_id.with_scale_and_position(scale, point(caret_x, caret_y));

            if let Some(outlined) = font.outline_glyph(glyph) {
                let bounds = outlined.px_bounds();
                // Avoid f32::{floor,ceil} in no_std; truncation is acceptable for now.
                let origin_x = bounds.min.x as i32;
                let origin_y = bounds.min.y as i32;
                outlined.draw(|gx, gy, coverage| {
                    let px = origin_x + gx as i32;
                    let py = origin_y + gy as i32;
                    self.put_pixel_alpha(px, py, color, coverage);
                });
            }

            caret_x += scaled.h_advance(glyph_id);
        }
    }
}
