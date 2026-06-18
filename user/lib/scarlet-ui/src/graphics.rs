//! Graphics primitives and drawing utilities for ScarletUI
//!
//! Provides Canvas for drawing text, shapes, and managing glyph caches.

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use ab_glyph::{Font, FontRef, Glyph, InvalidFont, PxScale, PxScaleFont, ScaleFont, point};

use crate::buffer::Buffer;
use crate::color::Color;
use crate::logln as println;
#[cfg(feature = "std")]
use crate::os::Read;
use crate::os::{File, Mutex};

static CURRENT_SCALE_MILLI: AtomicU32 = AtomicU32::new(1000);

/// Return the current UI scale in milli-units.
pub fn current_scale_milli() -> u32 {
    CURRENT_SCALE_MILLI.load(Ordering::Relaxed).max(1)
}

/// Set the current UI scale in milli-units.
pub fn set_current_scale_milli(scale_milli: u32) {
    CURRENT_SCALE_MILLI.store(scale_milli.max(1), Ordering::Relaxed);
}

/// Glyph cache key
#[derive(Clone, Copy, PartialEq, Eq)]
struct GlyphKey {
    codepoint: u32,
    size_px: u16,
    font_stack_id: usize,
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
    font_stack_id: usize,
}

impl TextMetricsKey {
    fn from_text(text: &str, font_size: f32, font_stack_id: usize) -> Self {
        let mut hash = 0u64;
        for b in text.bytes() {
            hash = hash.wrapping_mul(31).wrapping_add(b as u64);
        }
        Self {
            text_len: text.len(),
            text_hash: hash,
            font_size: font_size.to_bits() as u32,
            font_stack_id,
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

    fn get_or_compute<F>(
        &mut self,
        text: &str,
        font_size: f32,
        font_stack_id: usize,
        compute: F,
    ) -> (u32, u32)
    where
        F: FnOnce() -> (u32, u32),
    {
        let key = TextMetricsKey::from_text(text, font_size, font_stack_id);
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

static TEXT_METRICS_CACHE: Mutex<TextMetricsCache> =
    Mutex::new(TextMetricsCache::new(TEXT_METRICS_CACHE_CAP));

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
    font_stack_id: usize,
    font_slot: u8,
) -> Option<(i32, i32, u32, u32, *const u8)> {
    let key = GlyphKey {
        codepoint: ch as u32,
        size_px: scaled.scale.y as u16,
        font_stack_id,
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

#[cfg(any(not(feature = "std"), all(feature = "std", target_os = "scarlet")))]
const DEFAULT_FONT_PATH: &str = "/fonts/Mplus1-Regular.ttf";
#[cfg(any(not(feature = "std"), all(feature = "std", target_os = "scarlet")))]
const FALLBACK_FONT_PATHS: &[&str] = &["/fonts/JetBrainsMonoNerdFontMono-Regular.ttf"];

#[cfg(all(feature = "std", target_os = "scarlet"))]
const STD_DEFAULT_FONT_PATHS: &[&str] = &[DEFAULT_FONT_PATH];

#[cfg(all(feature = "std", target_os = "scarlet"))]
const STD_FALLBACK_FONT_PATHS: &[&str] = FALLBACK_FONT_PATHS;

#[cfg(all(feature = "std", target_os = "macos"))]
const STD_DEFAULT_FONT_PATHS: &[&str] = &[
    concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../bundles/desktop/fs/system/scarlet/fonts/Mplus1-Regular.ttf"
    ),
    "/System/Library/Fonts/SFNS.ttf",
    "/Library/Fonts/OpenSans-Regular.ttf",
];

#[cfg(all(feature = "std", target_os = "macos"))]
const STD_FALLBACK_FONT_PATHS: &[&str] = &[
    concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../bundles/desktop/fs/system/scarlet/fonts/JetBrainsMonoNerdFontMono-Regular.ttf"
    ),
    "/System/Library/Fonts/SFNSMono.ttf",
    "/System/Library/Fonts/Apple Symbols.ttf",
];

#[cfg(all(feature = "std", not(any(target_os = "scarlet", target_os = "macos"))))]
const STD_DEFAULT_FONT_PATHS: &[&str] = &[
    concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../bundles/desktop/fs/system/scarlet/fonts/Mplus1-Regular.ttf"
    ),
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
];

#[cfg(all(feature = "std", not(any(target_os = "scarlet", target_os = "macos"))))]
const STD_FALLBACK_FONT_PATHS: &[&str] = &[
    concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../bundles/desktop/fs/system/scarlet/fonts/JetBrainsMonoNerdFontMono-Regular.ttf"
    ),
    "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
    "/usr/share/fonts/truetype/noto/NotoSansMono-Regular.ttf",
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
];

static NEXT_FONT_STACK_ID: AtomicUsize = AtomicUsize::new(1);

fn next_font_stack_id() -> usize {
    NEXT_FONT_STACK_ID.fetch_add(1, Ordering::Relaxed)
}

/// Ordered font collection used for text rendering.
///
/// The first font is the primary face. Fallback fonts are consulted only when
/// the primary font does not provide a glyph for a character.
#[derive(Clone)]
pub struct FontStack {
    primary: FontRef<'static>,
    fallbacks: Vec<FontRef<'static>>,
    cache_id: usize,
}

impl FontStack {
    /// Create a font stack with a primary font.
    ///
    /// # Arguments
    ///
    /// * `font_bytes` - Primary font bytes with static lifetime.
    ///
    /// # Returns
    ///
    /// A font stack if the font bytes are valid.
    pub fn new(font_bytes: &'static [u8]) -> Result<Self, InvalidFont> {
        let primary = FontRef::try_from_slice(font_bytes)?;
        Ok(Self {
            primary,
            fallbacks: Vec::new(),
            cache_id: next_font_stack_id(),
        })
    }

    /// Add a fallback font to this stack.
    ///
    /// # Arguments
    ///
    /// * `font_bytes` - Fallback font bytes with static lifetime.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the fallback was accepted.
    pub fn add_fallback(&mut self, font_bytes: &'static [u8]) -> Result<(), InvalidFont> {
        let font = FontRef::try_from_slice(font_bytes)?;
        self.fallbacks.push(font);
        self.cache_id = next_font_stack_id();
        Ok(())
    }

    /// Return a copy of this stack with an additional fallback font.
    ///
    /// # Arguments
    ///
    /// * `font_bytes` - Fallback font bytes with static lifetime.
    ///
    /// # Returns
    ///
    /// The updated font stack if the fallback was accepted.
    pub fn with_fallback(mut self, font_bytes: &'static [u8]) -> Result<Self, InvalidFont> {
        self.add_fallback(font_bytes)?;
        Ok(self)
    }

    /// Return the cache identity for this stack.
    ///
    /// # Returns
    ///
    /// A stable identity for the current primary/fallback sequence.
    pub fn cache_id(&self) -> usize {
        self.cache_id
    }
}

#[derive(Clone)]
struct DefaultFontState {
    font: Option<FontRef<'static>>,
    fallback_fonts: Vec<FontRef<'static>>,
    cache_id: usize,
    load_attempted: bool,
}

static DEFAULT_FONT: Mutex<DefaultFontState> = Mutex::new(DefaultFontState {
    font: None,
    fallback_fonts: Vec::new(),
    cache_id: 0,
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
    state.cache_id = next_font_stack_id();
    drop(state);
    clear_text_caches();
    Ok(())
}

/// Replace the system-wide default UI font stack.
///
/// Existing widgets continue to use the default stack through regular text
/// drawing APIs. Widgets that need app-local fonts can keep their own
/// [`FontStack`] and pass it to the explicit font-stack APIs.
///
/// # Arguments
///
/// * `font_stack` - New system-wide default font stack.
///
/// # Returns
///
/// Nothing.
pub fn set_default_font_stack(font_stack: FontStack) {
    let FontStack {
        primary,
        fallbacks,
        cache_id,
    } = font_stack;
    let mut state = DEFAULT_FONT.lock();
    state.font = Some(primary);
    state.fallback_fonts = fallbacks;
    state.cache_id = cache_id;
    drop(state);
    clear_text_caches();
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
    let mut state = DEFAULT_FONT.lock();
    state.fallback_fonts.push(font);
    state.cache_id = next_font_stack_id();
    drop(state);
    clear_text_caches();
    Ok(())
}

/// Clear all fallback UI fonts.
///
/// # Returns
///
/// Nothing.
pub fn clear_default_font_fallbacks() {
    let mut state = DEFAULT_FONT.lock();
    state.fallback_fonts.clear();
    state.cache_id = next_font_stack_id();
    drop(state);
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

    for path in default_font_paths() {
        if let Some(bytes) = read_font_file(path)
            && set_default_font_owned(bytes).is_ok()
        {
            break;
        }
    }

    if DEFAULT_FONT.lock().font.is_none() {
        return;
    }

    for path in fallback_font_paths() {
        if let Some(bytes) = read_font_file(path)
            && add_fallback_font_owned(bytes).is_ok()
        {
            continue;
        }
    }
}

fn default_font_paths() -> &'static [&'static str] {
    #[cfg(feature = "std")]
    {
        STD_DEFAULT_FONT_PATHS
    }
    #[cfg(not(feature = "std"))]
    {
        &[DEFAULT_FONT_PATH]
    }
}

fn fallback_font_paths() -> &'static [&'static str] {
    #[cfg(feature = "std")]
    {
        STD_FALLBACK_FONT_PATHS
    }
    #[cfg(not(feature = "std"))]
    {
        FALLBACK_FONT_PATHS
    }
}

/// Return the current system-wide default font stack.
///
/// # Returns
///
/// The default font stack when a primary font is available.
pub fn default_font_stack() -> Option<FontStack> {
    load_fonts_from_rootfs_once();
    let state = DEFAULT_FONT.lock();
    state.font.clone().map(|primary| FontStack {
        primary,
        fallbacks: state.fallback_fonts.clone(),
        cache_id: state.cache_id,
    })
}

fn fullwidth_ascii_fallback(ch: char) -> Option<char> {
    let code = ch as u32;
    if (0xff01..=0xff5e).contains(&code) {
        core::char::from_u32(code - 0xfee0)
    } else if code == 0x3000 {
        Some(' ')
    } else {
        None
    }
}

fn select_font_for_char(ch: char, font_stack: &FontStack) -> (FontRef<'static>, u8, char) {
    if font_stack.primary.glyph_id(ch).0 != 0 {
        return (font_stack.primary.clone(), 0, ch);
    }
    for (index, font) in font_stack.fallbacks.iter().enumerate() {
        if font.glyph_id(ch).0 != 0 {
            return (font.clone(), index.saturating_add(1) as u8, ch);
        }
    }

    if let Some(fallback_ch) = fullwidth_ascii_fallback(ch) {
        if font_stack.primary.glyph_id(fallback_ch).0 != 0 {
            return (font_stack.primary.clone(), 0, fallback_ch);
        }
        for (index, font) in font_stack.fallbacks.iter().enumerate() {
            if font.glyph_id(fallback_ch).0 != 0 {
                return (font.clone(), index.saturating_add(1) as u8, fallback_ch);
            }
        }
    }

    (font_stack.primary.clone(), 0, ch)
}

/// Measure text using the global default vector font
///
/// Returns `(width, height)` in pixels
pub fn measure_text_sized(text: &str, font_size_px: f32) -> (u32, u32) {
    if let Some(font_stack) = default_font_stack() {
        measure_text_sized_with_font_stack(text, font_size_px, &font_stack)
    } else {
        fallback_text_metrics(text, font_size_px)
    }
}

/// Measure text using an explicit font stack.
///
/// # Arguments
///
/// * `text` - Text to measure.
/// * `font_size_px` - Font size in pixels.
/// * `font_stack` - Font stack to use.
///
/// # Returns
///
/// `(width, height)` in pixels.
pub fn measure_text_sized_with_font_stack(
    text: &str,
    font_size_px: f32,
    font_stack: &FontStack,
) -> (u32, u32) {
    TEXT_METRICS_CACHE
        .lock()
        .get_or_compute(text, font_size_px, font_stack.cache_id(), || {
            measure_text_uncached(text, font_size_px, font_stack)
        })
}

fn fallback_text_metrics(text: &str, font_size_px: f32) -> (u32, u32) {
    let fs = font_size_px.max(1.0);
    let char_w = ceil_i32(fs * 0.60).max(1) as u32;
    let w = (text.chars().count() as u32).saturating_mul(char_w);
    let h = ceil_i32(fs).max(1) as u32;
    (w, h)
}

fn measure_text_uncached(text: &str, font_size_px: f32, font_stack: &FontStack) -> (u32, u32) {
    let scale = PxScale::from(font_size_px);
    let scaled = font_stack.primary.as_scaled(scale);

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
        let (selected, _, render_ch) = select_font_for_char(ch, font_stack);
        let selected_scaled = selected.as_scaled(scale);
        let glyph_id = selected_scaled.glyph_id(render_ch);
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
}

/// Canvas for drawing operations
pub struct Canvas<'a> {
    buffer: &'a mut [u8],
    width: u32,
    height: u32,
    physical_width: u32,
    physical_height: u32,
    scale_milli: u32,
}

impl<'a> Canvas<'a> {
    /// Create a new canvas from a BGRA buffer
    pub fn new(buffer: &'a mut [u8], width: u32, height: u32) -> Self {
        Self {
            buffer,
            width,
            height,
            physical_width: width,
            physical_height: height,
            scale_milli: 1000,
        }
    }

    /// Create a canvas that draws logical coordinates into a scaled buffer.
    pub fn for_buffer(buffer: &'a mut Buffer) -> Self {
        let width = buffer.logical_width();
        let height = buffer.logical_height();
        let physical_width = buffer.width();
        let physical_height = buffer.height();
        let scale_milli = buffer.scale_milli();
        Self {
            buffer: buffer.data_mut(),
            width,
            height,
            physical_width,
            physical_height,
            scale_milli,
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

    fn scale_floor_i32(&self, value: i32) -> i32 {
        ((value as i64).saturating_mul(self.scale_milli as i64) / 1000) as i32
    }

    fn scale_ceil_i32(&self, value: i32) -> i32 {
        ((value as i64)
            .saturating_mul(self.scale_milli as i64)
            .saturating_add(999)
            / 1000) as i32
    }

    fn scale_len_u32(&self, value: u32) -> u32 {
        ((value as u64)
            .saturating_mul(self.scale_milli as u64)
            .saturating_add(999)
            / 1000)
            .max(1) as u32
    }

    /// Draw a single pixel
    pub fn put_pixel(&mut self, x: i32, y: i32, color: Color) {
        let px = self.scale_floor_i32(x);
        let py = self.scale_floor_i32(y);
        let w = self.scale_len_u32(1);
        let h = self.scale_len_u32(1);
        self.fill_physical_rect(px, py, w, h, color);
    }

    fn put_pixel_physical(&mut self, x: i32, y: i32, color: Color) {
        if x < 0 || x >= self.physical_width as i32 || y < 0 || y >= self.physical_height as i32 {
            return;
        }

        let offset = ((y as u32 * self.physical_width + x as u32) * 4) as usize;
        if offset + 4 <= self.buffer.len() {
            // Convert to BGRA and use little-endian bytes
            // to_bgra() produces 0xAARRGGBB which becomes [BB, GG, RR, AA] in little-endian
            let bgra = color.to_bgra();
            self.buffer[offset..offset + 4].copy_from_slice(&bgra.to_le_bytes());
        }
    }

    fn get_pixel_physical(&self, x: i32, y: i32) -> Color {
        if x < 0 || x >= self.physical_width as i32 || y < 0 || y >= self.physical_height as i32 {
            return Color::BLACK;
        }
        let offset = ((y as u32 * self.physical_width + x as u32) * 4) as usize;
        if offset + 4 > self.buffer.len() {
            return Color::BLACK;
        }
        // Read BGRA bytes as little-endian u32 and convert using from_bgra
        let bgra_bytes = [
            self.buffer[offset],
            self.buffer[offset + 1],
            self.buffer[offset + 2],
            self.buffer[offset + 3],
        ];
        Color::from_bgra(u32::from_le_bytes(bgra_bytes))
    }

    fn put_pixel_physical_alpha(&mut self, x: i32, y: i32, color: Color, alpha: f32) {
        if alpha <= 0.0 {
            return;
        }
        if alpha >= 1.0 {
            self.put_pixel_physical(x, y, color);
            return;
        }

        let dst = self.get_pixel_physical(x, y);

        // color.a is already in 0.0-1.0 range, not 0-255
        let src_a = (alpha * color.a).clamp(0.0, 1.0);
        let dst_a = dst.a.clamp(0.0, 1.0);
        let out_a = src_a + dst_a * (1.0 - src_a);

        if out_a <= 0.0 {
            self.put_pixel_physical(x, y, Color::rgba(0.0, 0.0, 0.0, 0.0));
            return;
        }

        let out_r = (color.r * src_a + dst.r * dst_a * (1.0 - src_a)) / out_a;
        let out_g = (color.g * src_a + dst.g * dst_a * (1.0 - src_a)) / out_a;
        let out_b = (color.b * src_a + dst.b * dst_a * (1.0 - src_a)) / out_a;
        let out_a_f32 = out_a;

        self.put_pixel_physical(x, y, Color::rgba(out_r, out_g, out_b, out_a_f32));
    }

    /// Fill a rectangle with a solid color
    pub fn fill_rect(&mut self, x: i32, y: i32, width: u32, height: u32, color: Color) {
        let x0 = self.scale_floor_i32(x);
        let y0 = self.scale_floor_i32(y);
        let x1 = self.scale_ceil_i32(x.saturating_add(width as i32));
        let y1 = self.scale_ceil_i32(y.saturating_add(height as i32));
        let physical_width = (x1 - x0).max(0) as u32;
        let physical_height = (y1 - y0).max(0) as u32;
        self.fill_physical_rect(x0, y0, physical_width, physical_height, color);
    }

    fn fill_physical_rect(&mut self, x: i32, y: i32, width: u32, height: u32, color: Color) {
        // Convert to BGRA and use little-endian bytes
        // to_bgra() produces 0xAARRGGBB which becomes [BB, GG, RR, AA] in little-endian
        let bgra = color.to_bgra();
        let bgra_bytes = bgra.to_le_bytes();

        for dy in 0..height {
            for dx in 0..width {
                let px = x + dx as i32;
                let py = y + dy as i32;

                if px < 0
                    || px >= self.physical_width as i32
                    || py < 0
                    || py >= self.physical_height as i32
                {
                    continue;
                }

                let offset = ((py as u32 * self.physical_width + px as u32) * 4) as usize;
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
        let Some(font_stack) = default_font_stack() else {
            return;
        };
        self.draw_text_sized_with_font_stack(x, y, text, color, font_size_px, &font_stack);
    }

    /// Draw text with explicit font size and font stack.
    ///
    /// `x,y` is the **top-left** of the text line.
    ///
    /// # Arguments
    ///
    /// * `x` - Left position in pixels.
    /// * `y` - Top position in pixels.
    /// * `text` - Text to draw.
    /// * `color` - Text color.
    /// * `font_size_px` - Font size in pixels.
    /// * `font_stack` - Font stack to use.
    ///
    /// # Returns
    ///
    /// Nothing.
    pub fn draw_text_sized_with_font_stack(
        &mut self,
        x: i32,
        y: i32,
        text: &str,
        color: Color,
        font_size_px: f32,
        font_stack: &FontStack,
    ) {
        let ui_scale = (self.scale_milli as f32) / 1000.0;
        let scale = PxScale::from(font_size_px * ui_scale);
        let base_scaled = font_stack.primary.as_scaled(scale);

        let origin_x = self.scale_floor_i32(x) as f32;
        let mut caret_x = origin_x;
        let mut caret_y = self.scale_floor_i32(y) as f32 + base_scaled.ascent();

        for ch in text.chars() {
            if ch == '\n' {
                caret_x = origin_x;
                caret_y += base_scaled.height() + base_scaled.line_gap();
                continue;
            }

            let (selected_font, font_slot, render_ch) = select_font_for_char(ch, font_stack);
            let scaled = selected_font.as_scaled(scale);
            let glyph_id = scaled.glyph_id(render_ch);
            if let Some((ox, oy, w, h, ptr)) =
                glyph_cache_get_or_rasterize(&scaled, render_ch, font_stack.cache_id(), font_slot)
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
                        self.put_pixel_physical_alpha(px, py, color, alpha);
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

        self.fill_rect(x, y, width, 1, color);
        self.fill_rect(x, y + height as i32 - 1, width, 1, color);
        self.fill_rect(x, y, 1, height, color);
        self.fill_rect(x + width as i32 - 1, y, 1, height, color);
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
