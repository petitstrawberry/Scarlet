//! Graphics primitives and drawing utilities

use std::println;

use ab_glyph::{point, Font, FontRef, Glyph, InvalidFont, PxScale, PxScaleFont, ScaleFont};
use crate::Color;

extern crate scarlet_std as std;
extern crate alloc;

use alloc::{boxed::Box, vec::Vec};

#[derive(Clone, Copy, PartialEq, Eq)]
struct GlyphKey {
    codepoint: u32,
    size_px: u16,
}

struct GlyphMask {
    key: GlyphKey,
    width: u32,
    height: u32,
    origin_x: i32,
    origin_y: i32,
    mask: Box<[u8]>,
}

struct GlyphCacheState {
    entries: Vec<GlyphMask>,
    next_evict: usize,
}

impl GlyphCacheState {
    const fn new() -> Self {
        Self {
            entries: Vec::new(),
            next_evict: 0,
        }
    }
}

// Keep this modest; UI text tends to reuse glyphs.
const GLYPH_CACHE_CAP: usize = 256;

static GLYPH_CACHE: std::sync::Mutex<GlyphCacheState> =
    std::sync::Mutex::new(GlyphCacheState::new());

#[inline]
fn floor_i32(v: f32) -> i32 {
    let i = v as i32;
    if (i as f32) > v {
        i - 1
    } else {
        i
    }
}

#[inline]
fn ceil_i32(v: f32) -> i32 {
    let i = v as i32;
    if (i as f32) < v {
        i + 1
    } else {
        i
    }
}

fn glyph_cache_get_or_rasterize(
    scaled: &PxScaleFont<&FontRef<'static>>,
    ch: char,
) -> Option<(i32, i32, u32, u32, *const u8)> {
    let key = GlyphKey {
        codepoint: ch as u32,
        size_px: scaled.scale.y as u16,
    };

    let mut cache = GLYPH_CACHE.lock();
    if let Some(found) = cache.entries.iter().find(|e| e.key == key) {
        return Some((
            found.origin_x,
            found.origin_y,
            found.width,
            found.height,
            found.mask.as_ptr(),
        ));
    }

    // Rasterize at a stable origin where the baseline is y=0.
    // The draw path already positions each glyph at its caret baseline.
    let glyph_id = scaled.glyph_id(ch);
    let glyph: Glyph = glyph_id.with_scale_and_position(scaled.scale, point(0.0, 0.0));
    let outlined = scaled.font.outline_glyph(glyph)?;
    let bounds = outlined.px_bounds();

    let min_x = floor_i32(bounds.min.x);
    let min_y = floor_i32(bounds.min.y);
    let max_x = ceil_i32(bounds.max.x);
    let max_y = ceil_i32(bounds.max.y);

    let width = (max_x - min_x).max(0) as u32;
    let height = (max_y - min_y).max(0) as u32;
    if width == 0 || height == 0 {
        return None;
    }

    let mut mask = alloc::vec![0u8; (width as usize) * (height as usize)];
    outlined.draw(|gx, gy, coverage| {
        let idx = (gy as usize) * (width as usize) + (gx as usize);
        if idx < mask.len() {
            let a = (coverage * 255.0) as u8;
            if a > mask[idx] {
                mask[idx] = a;
            }
        }
    });

    let entry = GlyphMask {
        key,
        width,
        height,
        origin_x: min_x,
        origin_y: min_y,
        mask: mask.into_boxed_slice(),
    };

    if cache.entries.len() < GLYPH_CACHE_CAP {
        cache.entries.push(entry);
        let last = cache.entries.last().unwrap();
        Some((
            last.origin_x,
            last.origin_y,
            last.width,
            last.height,
            last.mask.as_ptr(),
        ))
    } else {
        let idx = cache.next_evict % GLYPH_CACHE_CAP;
        cache.next_evict = cache.next_evict.wrapping_add(1);
        cache.entries[idx] = entry;
        let e = &cache.entries[idx];
        Some((e.origin_x, e.origin_y, e.width, e.height, e.mask.as_ptr()))
    }
}

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

/// Measure text using the global default vector font.
///
/// Returns `(width, height)` in pixels for a single-line text layout.
/// If the default font is not available, falls back to a rough estimate.
pub fn measure_text_sized(text: &str, font_size_px: f32) -> (u32, u32) {
    if let Some(font) = default_font() {
        let scale = PxScale::from(font_size_px);
        let scaled = font.as_scaled(scale);

        let mut max_line_w: f32 = 0.0;
        let mut line_w: f32 = 0.0;
        let mut lines: u32 = 1;

        for ch in text.chars() {
            if ch == '\n' {
                if line_w > max_line_w {
                    max_line_w = line_w;
                }
                line_w = 0.0;
                lines = lines.saturating_add(1);
                continue;
            }
            let glyph_id = scaled.glyph_id(ch);
            line_w += scaled.h_advance(glyph_id);
        }

        if line_w > max_line_w {
            max_line_w = line_w;
        }

        let line_h = scaled.height() + scaled.line_gap();
        let total_h = if lines <= 1 {
            scaled.height()
        } else {
            scaled.height() + (lines.saturating_sub(1) as f32) * line_h
        };

        let w = ceil_i32(max_line_w).max(0) as u32;
        let h = ceil_i32(total_h).max(0) as u32;
        (w, h)
    } else {
        let fs = font_size_px.max(1.0);
        let char_w = ceil_i32(fs * 0.60).max(1) as u32;
        let w = (text.chars().count() as u32).saturating_mul(char_w);
        let h = ceil_i32(fs).max(1) as u32;
        (w, h)
    }
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
            if let Some((ox, oy, w, h, ptr)) = glyph_cache_get_or_rasterize(&scaled, ch) {
                // Position this glyph at the current caret baseline.
                let base_x = caret_x as i32;
                let base_y = caret_y as i32;
                let mask = unsafe { core::slice::from_raw_parts(ptr, (w as usize) * (h as usize)) };
                for gy in 0..h {
                    let row = (gy as usize) * (w as usize);
                    for gx in 0..w {
                        let a = mask[row + gx as usize];
                        if a == 0 {
                            continue;
                        }
                        let alpha = (a as f32) / 255.0;
                        let px = base_x + ox + gx as i32;
                        let py = base_y + oy + gy as i32;
                        self.put_pixel_alpha(px, py, color, alpha);
                    }
                }
            }

            caret_x += scaled.h_advance(glyph_id);
        }
    }

    /// Draw a rounded rectangle
    pub fn fill_rounded_rect(&mut self, x: i32, y: i32, width: u32, height: u32, radius: u32, color: Color) {
        let radius = radius.min(width / 2).min(height / 2);
        
        // Fill center rectangle
        if width > radius * 2 {
            self.fill_rect(x + radius as i32, y, width - radius * 2, height, color);
        }
        
        // Fill left and right vertical strips
        if height > radius * 2 {
            self.fill_rect(x, y + radius as i32, radius, height - radius * 2, color);
            self.fill_rect(x + (width - radius) as i32, y + radius as i32, radius, height - radius * 2, color);
        }
        
        // Draw rounded corners
        let r_sq = (radius * radius) as i32;
        for dy in 0..radius {
            for dx in 0..radius {
                let dist_sq = (dx * dx + dy * dy) as i32;
                if dist_sq <= r_sq {
                    // Top-left
                    self.put_pixel(x + (radius - dx - 1) as i32, y + (radius - dy - 1) as i32, color);
                    // Top-right
                    self.put_pixel(x + (width - radius + dx) as i32, y + (radius - dy - 1) as i32, color);
                    // Bottom-left
                    self.put_pixel(x + (radius - dx - 1) as i32, y + (height - radius + dy) as i32, color);
                    // Bottom-right
                    self.put_pixel(x + (width - radius + dx) as i32, y + (height - radius + dy) as i32, color);
                }
            }
        }
    }

    /// Draw a rounded rectangle outline
    pub fn draw_rounded_rect(&mut self, x: i32, y: i32, width: u32, height: u32, radius: u32, color: Color) {
        let radius = radius.min(width / 2).min(height / 2);
        
        // Draw straight edges
        // Top
        for dx in radius..(width - radius) {
            self.put_pixel(x + dx as i32, y, color);
            self.put_pixel(x + dx as i32, y + height as i32 - 1, color);
        }
        // Sides
        for dy in radius..(height - radius) {
            self.put_pixel(x, y + dy as i32, color);
            self.put_pixel(x + width as i32 - 1, y + dy as i32, color);
        }
        
        // Draw rounded corners (circle approximation)
        let r_sq = (radius * radius) as i32;
        let inner_r_sq = ((radius - 1) * (radius - 1)) as i32;
        
        for dy in 0..radius {
            for dx in 0..radius {
                let dist_sq = (dx * dx + dy * dy) as i32;
                if dist_sq <= r_sq && dist_sq >= inner_r_sq {
                    // Top-left
                    self.put_pixel(x + (radius - dx - 1) as i32, y + (radius - dy - 1) as i32, color);
                    // Top-right
                    self.put_pixel(x + (width - radius + dx) as i32, y + (radius - dy - 1) as i32, color);
                    // Bottom-left
                    self.put_pixel(x + (radius - dx - 1) as i32, y + (height - radius + dy) as i32, color);
                    // Bottom-right
                    self.put_pixel(x + (width - radius + dx) as i32, y + (height - radius + dy) as i32, color);
                }
            }
        }
    }
}
