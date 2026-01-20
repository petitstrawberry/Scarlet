//! Graphics primitives and text rendering utilities
//!
//! Based on deprecated/scarlet-ui/src/graphics.rs

use crate::geometry::{Point, Rect, Size};
use ab_glyph::{point, Font, FontRef, Glyph, PxScale, PxScaleFont, ScaleFont};
use std::sync::Mutex;
use std::vec::Vec;

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

const GLYPH_CACHE_CAP: usize = 256;

static GLYPH_CACHE: Mutex<GlyphCacheState> =
    Mutex::new(GlyphCacheState::new());

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

    // Rasterize glyph
    let glyph_id = scaled.glyph_id(ch);
    let glyph = glyph_id.with_scale_and_position(scaled.scale, point(0.0, 0.0));
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

    let mut mask = vec![0u8; (width as usize) * (height as usize)];
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

// Rootfs-provided default font
const DEFAULT_FONT_PATH: &str = "/fonts/Mplus1-Regular.ttf";

#[derive(Clone)]
struct DefaultFontState {
    font: Option<FontRef<'static>>,
    load_attempted: bool,
}

static DEFAULT_FONT: Mutex<DefaultFontState> =
    Mutex::new(DefaultFontState {
        font: None,
        load_attempted: false,
    });

/// Set the default UI font
pub fn set_default_font(font_bytes: &'static [u8]) -> Result<(), ab_glyph::InvalidFont> {
    let font = FontRef::try_from_slice(font_bytes)?;
    let mut state = DEFAULT_FONT.lock();
    state.font = Some(font);
    Ok(())
}

fn set_default_font_owned(font_bytes: Vec<u8>) -> Result<(), ab_glyph::InvalidFont> {
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

/// Measure text using the default font
///
/// Returns `(width, height)` in pixels
pub fn measure_text(text: &str, font_size_px: f32) -> (u32, u32) {
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
        // Fallback estimation
        let fs = font_size_px.max(1.0);
        let char_w = ceil_i32(fs * 0.60).max(1) as u32;
        let w = (text.chars().count() as u32).saturating_mul(char_w);
        let h = ceil_i32(fs).max(1) as u32;
        (w, h)
    }
}

/// Draw text to a buffer slice
///
/// # Arguments
/// * `buffer` - Mutable slice of RGBA pixel data
/// * `buffer_width` - Width of the buffer in pixels
/// * `buffer_height` - Height of the buffer in pixels
/// * `text` - Text to draw
/// * `x` - X position
/// * `y` - Y position
/// * `font_size` - Font size in pixels
/// * `color` - RGBA color
pub fn draw_text(
    buffer: &mut [u8],
    buffer_width: usize,
    buffer_height: usize,
    text: &str,
    mut x: i32,
    mut y: i32,
    font_size: f32,
    color: [u8; 4],
) {
    if let Some(font) = default_font() {
        let scale = PxScale::from(font_size);
        let scaled = font.as_scaled(scale);

        // Vertical adjustment for baseline
        y += scaled.ascent().ceil() as i32;

        for ch in text.chars() {
            if let Some((origin_x, origin_y, width, height, mask_ptr)) =
                glyph_cache_get_or_rasterize(&scaled, ch)
            {
                let gx = x + origin_x;
                let gy = y + origin_y;

                // Clip to buffer bounds
                if gx < 0 || gy < 0 {
                    x += scaled.h_advance(scaled.glyph_id(ch)).ceil() as i32;
                    continue;
                }

                let gx = gx as usize;
                let gy = gy as usize;
                let width = width as usize;
                let height = height as usize;

                // Check bounds
                if gx >= buffer_width || gy >= buffer_height {
                    x += scaled.h_advance(scaled.glyph_id(ch)).ceil() as i32;
                    continue;
                }

                let draw_width = width.min(buffer_width - gx);
                let draw_height = height.min(buffer_height - gy);

                unsafe {
                    let mask = std::slice::from_raw_parts(mask_ptr, width * height);

                    for dy in 0..draw_height {
                        for dx in 0..draw_width {
                            let alpha = mask[dy * width + dx] as u32;
                            if alpha == 0 {
                                continue;
                            }

                            let buf_idx = ((gy + dy) * buffer_width + (gx + dx)) * 4;
                            if buf_idx + 3 < buffer.len() {
                                // Alpha blending
                                let alpha = alpha as u32;
                                let inv_alpha = 255 - alpha;

                                buffer[buf_idx + 0] =
                                    ((buffer[buf_idx + 0] as u32 * inv_alpha + color[0] as u32 * alpha) / 255) as u8;
                                buffer[buf_idx + 1] =
                                    ((buffer[buf_idx + 1] as u32 * inv_alpha + color[1] as u32 * alpha) / 255) as u8;
                                buffer[buf_idx + 2] =
                                    ((buffer[buf_idx + 2] as u32 * inv_alpha + color[2] as u32 * alpha) / 255) as u8;
                                buffer[buf_idx + 3] =
                                    ((buffer[buf_idx + 3] as u32 * inv_alpha + color[3] as u32 * alpha) / 255) as u8;
                            }
                        }
                    }
                }

                x += scaled.h_advance(scaled.glyph_id(ch)).ceil() as i32;
            }
        }
    }
}
