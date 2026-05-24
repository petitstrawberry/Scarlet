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
    font_slot: u8,
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
    font_slot: u8,
) -> Option<(i32, i32, u32, u32, *const u8)> {
    let key = GlyphKey {
        codepoint: ch as u32,
        size_px: scaled.scale.y as u16,
        font_slot,
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
const FALLBACK_FONT_PATHS: &[&str] = &["/fonts/JetBrainsMonoNerdFontMono-Regular.ttf"];

#[derive(Clone)]
struct DefaultFontState {
    font: Option<FontRef<'static>>,
    fallback_fonts: Vec<FontRef<'static>>,
    load_attempted: bool,
}

static DEFAULT_FONT: Mutex<DefaultFontState> = Mutex::new(DefaultFontState {
    font: None,
    fallback_fonts: Vec::new(),
    load_attempted: false,
});

fn clear_text_caches() {
    let mut glyph_cache = GLYPH_CACHE.lock();
    glyph_cache.entries.clear();
    glyph_cache.next_evict = 0;
    TEXT_METRICS_CACHE.lock().entries.clear();
}

/// Set the default UI font
///
/// # Arguments
///
/// * `font_bytes` - Font bytes with static lifetime.
///
/// # Returns
///
/// `Ok(())` when the font was accepted.
pub fn set_default_font(font_bytes: &'static [u8]) -> Result<(), InvalidFont> {
    let font = FontRef::try_from_slice(font_bytes)?;
    let mut state = DEFAULT_FONT.lock();
    state.font = Some(font);
    drop(state);
    clear_text_caches();
    Ok(())
}

/// Add a fallback UI font.
///
/// Fallback fonts are consulted when the primary default font has no glyph for
/// a character. Later UI settings can use this API to make fonts configurable.
///
/// # Arguments
///
/// * `font_bytes` - Fallback font bytes with static lifetime.
///
/// # Returns
///
/// `Ok(())` when the font was accepted.
pub fn add_default_font_fallback(font_bytes: &'static [u8]) -> Result<(), InvalidFont> {
    let font = FontRef::try_from_slice(font_bytes)?;
    DEFAULT_FONT.lock().fallback_fonts.push(font);
    clear_text_caches();
    Ok(())
}

/// Clear all fallback UI fonts.
///
/// # Returns
///
/// Nothing.
pub fn clear_default_font_fallbacks() {
    DEFAULT_FONT.lock().fallback_fonts.clear();
    clear_text_caches();
}

fn set_default_font_owned(font_bytes: Vec<u8>) -> Result<(), InvalidFont> {
    let leaked: &'static [u8] = Box::leak(font_bytes.into_boxed_slice());
    set_default_font(leaked)
}

fn add_fallback_font_owned(font_bytes: Vec<u8>) -> Result<(), InvalidFont> {
    let leaked: &'static [u8] = Box::leak(font_bytes.into_boxed_slice());
    add_default_font_fallback(leaked)
}

fn read_font_file(path: &str) -> Option<Vec<u8>> {
    let mut file = match File::open(path) {
        Ok(f) => f,
        Err(e) => {
            if crate::debug::is_enabled() {
                println!("[scarlet-ui] Failed to open font '{}': {:?}", path, e);
            }
            return None;
        }
    };

    let mut bytes = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let n = match file.read(&mut buf) {
            Ok(n) => n,
            Err(_) => return None,
        };
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&buf[..n]);
    }

    Some(bytes)
}

fn load_fonts_from_rootfs_once() {
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

    if let Some(bytes) = read_font_file(DEFAULT_FONT_PATH) {
        let _ = set_default_font_owned(bytes);
    }
    for path in FALLBACK_FONT_PATHS {
        if let Some(bytes) = read_font_file(path) {
            let _ = add_fallback_font_owned(bytes);
        }
    }
}

fn loaded_fonts() -> Option<(FontRef<'static>, Vec<FontRef<'static>>)> {
    load_fonts_from_rootfs_once();
    let state = DEFAULT_FONT.lock();
    state
        .font
        .clone()
        .map(|font| (font, state.fallback_fonts.clone()))
}

fn select_font_for_char(
    ch: char,
    primary: &FontRef<'static>,
    fallbacks: &[FontRef<'static>],
) -> (FontRef<'static>, u8) {
    if primary.glyph_id(ch).0 != 0 {
        return (primary.clone(), 0);
    }
    for (index, font) in fallbacks.iter().enumerate() {
        if font.glyph_id(ch).0 != 0 {
            return (font.clone(), index.saturating_add(1) as u8);
        }
    }
    (primary.clone(), 0)
}

/// Measure text using the global default vector font
///
/// Returns `(width, height)` in pixels
pub fn measure_text_sized(text: &str, font_size_px: f32) -> (u32, u32) {
    TEXT_METRICS_CACHE.lock().get_or_compute(text, font_size_px, || {
        if let Some((font, fallbacks)) = loaded_fonts() {
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
                let selected = select_font_for_char(ch, &font, &fallbacks).0;
                let selected_scaled = selected.as_scaled(scale);
                let glyph_id = selected_scaled.glyph_id(ch);
                line_w += selected_scaled.h_advance(glyph_id);
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

    /// Return the canvas width.
    ///
    /// # Returns
    ///
    /// Width in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Return the canvas height.
    ///
    /// # Returns
    ///
    /// Height in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Draw a single pixel
    pub fn put_pixel(&mut self, x: i32, y: i32, color: Color) {
        if x < 0 || x >= self.width as i32 || y < 0 || y >= self.height as i32 {
            return;
        }

        let offset = ((y as u32 * self.width + x as u32) * 4) as usize;
        if offset + 4 <= self.buffer.len() {
            // Convert to BGRA and use little-endian bytes
            // to_bgra() produces 0xAARRGGBB which becomes [BB, GG, RR, AA] in little-endian
            let bgra = color.to_bgra();
            self.buffer[offset..offset + 4].copy_from_slice(&bgra.to_le_bytes());
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
        // Read BGRA bytes as little-endian u32 and convert using from_bgra
        let bgra_bytes = [self.buffer[offset], self.buffer[offset + 1],
                          self.buffer[offset + 2], self.buffer[offset + 3]];
        Color::from_bgra(u32::from_le_bytes(bgra_bytes))
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

        // color.a is already in 0.0-1.0 range, not 0-255
        let src_a = (alpha * color.a).clamp(0.0, 1.0);
        let dst_a = dst.a.clamp(0.0, 1.0);
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
        // Convert to BGRA and use little-endian bytes
        // to_bgra() produces 0xAARRGGBB which becomes [BB, GG, RR, AA] in little-endian
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
        let Some((font, fallbacks)) = loaded_fonts() else {
            return;
        };

        let scale = PxScale::from(font_size_px);
        let base_scaled = font.as_scaled(scale);

        let mut caret_x = x as f32;
        let mut caret_y = y as f32 + base_scaled.ascent();

        for ch in text.chars() {
            if ch == '\n' {
                caret_x = x as f32;
                caret_y += base_scaled.height() + base_scaled.line_gap();
                continue;
            }

            let (selected_font, font_slot) = select_font_for_char(ch, &font, &fallbacks);
            let scaled = selected_font.as_scaled(scale);
            let glyph_id = scaled.glyph_id(ch);
            if let Some((ox, oy, w, h, ptr)) =
                glyph_cache_get_or_rasterize(&scaled, ch, font_slot)
            {
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

    /// Draw rectangle outline (1px border)
    pub fn draw_rect(&mut self, x: i32, y: i32, width: u32, height: u32, color: Color) {
        if width == 0 || height == 0 {
            return;
        }

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

    /// Draw line using Bresenham's algorithm
    pub fn draw_line(&mut self, mut x0: i32, mut y0: i32, x1: i32, y1: i32, color: Color) {
        let dx = (x1 - x0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let dy = -(y1 - y0).abs();
        let sy = if y0 < y1 { 1 } else { -1 };

        let mut err = dx + dy;
        loop {
            self.put_pixel(x0, y0, color);
            if x0 == x1 && y0 == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x0 += sx;
            }
            if e2 <= dx {
                err += dx;
                y0 += sy;
            }
        }
    }
}
