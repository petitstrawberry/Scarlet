//! Graphics primitives and drawing utilities for ScarletUI
//!
//! Provides Canvas for drawing text, shapes, and managing glyph caches.

use alloc::boxed::Box;
use alloc::vec::Vec;

use ab_glyph::{point, Font, FontRef, Glyph, InvalidFont, PxScale, PxScaleFont, ScaleFont};

use crate::color::Color;
use scarlet_std::{println, sync::Mutex, fs::File};

/// Glyph cache key
#[derive(Clone, Copy, PartialEq, Eq)]
struct GlyphKey {
    codepoint: u32,
    size_px: u16,
}

/// Rasterized glyph mask
struct GlyphMask {
    key: GlyphKey,
    width: u32,
    height: u32,
    origin_x: i32,
    origin_y: i32,
    mask: Box<[u8]>,
}

/// Glyph cache state
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

const GLYPH_CACHE_CAP: usize = 256;

static GLYPH_CACHE: Mutex<GlyphCacheState> = Mutex::new(GlyphCacheState::new());

/// Text measurement cache key
#[derive(Clone, Copy, PartialEq, Eq)]
struct TextMetricsKey {
    text_len: usize,
    text_hash: u64,
    font_size: u32,
}

impl TextMetricsKey {
    fn from_text(text: &str, font_size: f32) -> Self {
        let mut hash = 0u64;
        for b in text.bytes() {
            hash = hash.wrapping_mul(31).wrapping_add(b as u64);
        }
        Self {
            text_len: text.len(),
            text_hash: hash,
            font_size: font_size.to_bits() as u32,
        }
    }
}

struct TextMetricsEntry {
    key: TextMetricsKey,
    value: (u32, u32),
}

struct TextMetricsCache {
    entries: Vec<TextMetricsEntry>,
    max_entries: usize,
}

impl TextMetricsCache {
    const fn new(max_entries: usize) -> Self {
        Self {
            entries: Vec::new(),
            max_entries,
        }
    }

    fn get_or_compute<F>(&mut self, text: &str, font_size: f32, compute: F) -> (u32, u32)
    where
        F: FnOnce() -> (u32, u32),
    {
        let key = TextMetricsKey::from_text(text, font_size);
        for entry in &self.entries {
            if entry.key == key {
                return entry.value;
            }
        }

        let result = compute();

        if self.entries.len() >= self.max_entries {
            let remove_count = self.max_entries / 4;
            if remove_count > 0 {
                self.entries.drain(0..remove_count.min(self.entries.len()));
            }
        }

        self.entries.push(TextMetricsEntry { key, value: result });
        result
    }
}

const TEXT_METRICS_CACHE_CAP: usize = 128;

static TEXT_METRICS_CACHE: Mutex<TextMetricsCache> = Mutex::new(TextMetricsCache::new(TEXT_METRICS_CACHE_CAP));

#[inline]
fn floor_i32(v: f32) -> i32 {
    let i = v as i32;
    if (i as f32) > v { i - 1 } else { i }
}

#[inline]
fn ceil_i32(v: f32) -> i32 {
    let i = v as i32;
    if (i as f32) < v { i + 1 } else { i }
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

const DEFAULT_FONT_PATH: &str = "/fonts/Mplus1-Regular.ttf";

#[derive(Clone)]
struct DefaultFontState {
    font: Option<FontRef<'static>>,
    load_attempted: bool,
}

static DEFAULT_FONT: Mutex<DefaultFontState> = Mutex::new(DefaultFontState {
    font: None,
    load_attempted: false,
});

/// Set the default UI font
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

    let mut file = match File::open(DEFAULT_FONT_PATH) {
        Ok(f) => f,
        Err(e) => {
            println!("[scarlet-ui] Failed to open default font '{}': {:?}", DEFAULT_FONT_PATH, e);
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

/// Measure text using the global default vector font
///
/// Returns `(width, height)` in pixels
pub fn measure_text_sized(text: &str, font_size_px: f32) -> (u32, u32) {
    TEXT_METRICS_CACHE.lock().get_or_compute(text, font_size_px, || {
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
    })
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
            let bgra_bytes = bgra.to_le_bytes();
            self.buffer[offset..offset + 4].copy_from_slice(&bgra_bytes);
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
        let a = self.buffer[offset + 3];
        Color::rgba(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, a as f32 / 255.0)
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

        let src_a = (alpha * (color.a as f32 / 255.0)).clamp(0.0, 1.0);
        let dst_a = (dst.a as f32 / 255.0).clamp(0.0, 1.0);
        let out_a = src_a + dst_a * (1.0 - src_a);

        if out_a <= 0.0 {
            self.put_pixel(x, y, Color::rgba(0.0, 0.0, 0.0, 0.0));
            return;
        }

        let out_r = (color.r * src_a + dst.r * dst_a * (1.0 - src_a)) / out_a;
        let out_g = (color.g * src_a + dst.g * dst_a * (1.0 - src_a)) / out_a;
        let out_b = (color.b * src_a + dst.b * dst_a * (1.0 - src_a)) / out_a;
        let out_a_f32 = out_a;

        self.put_pixel(x, y, Color::rgba(out_r, out_g, out_b, out_a_f32));
    }

    /// Fill a rectangle with a solid color
    pub fn fill_rect(&mut self, x: i32, y: i32, width: u32, height: u32, color: Color) {
        let bgra = color.to_bgra();
        let bgra_bytes = bgra.to_le_bytes();

        for dy in 0..height {
            for dx in 0..width {
                let px = x + dx as i32;
                let py = y + dy as i32;

                if px < 0 || px >= self.width as i32 || py < 0 || py >= self.height as i32 {
                    continue;
                }

                let offset = ((py as u32 * self.width + px as u32) * 4) as usize;
                if offset + 4 <= self.buffer.len() {
                    self.buffer[offset..offset + 4].copy_from_slice(&bgra_bytes);
                }
            }
        }
    }

    /// Draw text with explicit font size
    ///
    /// `x,y` is the **top-left** of the text line
    pub fn draw_text_sized(&mut self, x: i32, y: i32, text: &str, color: Color, font_size_px: f32) {
        let Some(font) = default_font() else {
            return;
        };

        let scale = PxScale::from(font_size_px);
        let scaled = font.as_scaled(scale);

        let mut caret_x = x as f32;
        let mut caret_y = y as f32 + scaled.ascent();

        for ch in text.chars() {
            if ch == '\n' {
                caret_x = x as f32;
                caret_y += scaled.height() + scaled.line_gap();
                continue;
            }

            let glyph_id = scaled.glyph_id(ch);
            if let Some((ox, oy, w, h, ptr)) = glyph_cache_get_or_rasterize(&scaled, ch) {
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
}
