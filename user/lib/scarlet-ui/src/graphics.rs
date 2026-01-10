//! Graphics primitives and drawing utilities

use crate::Color;

/// 2D point
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

impl Point {
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

    pub fn contains(&self, point: Point) -> bool {
        point.x >= self.x
            && point.x < self.x + self.width as i32
            && point.y >= self.y
            && point.y < self.y + self.height as i32
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

    /// Fill a rectangle with a solid color
    pub fn fill_rect(&mut self, rect: Rect, color: Color) {
        let bgra = color.to_bgra();

        for y in 0..rect.height {
            for x in 0..rect.width {
                let px = rect.x + x as i32;
                let py = rect.y + y as i32;

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

    /// Draw a rectangle outline
    pub fn draw_rect(&mut self, rect: Rect, color: Color) {
        // Top and bottom edges
        for x in 0..rect.width {
            self.put_pixel(rect.x + x as i32, rect.y, color);
            self.put_pixel(rect.x + x as i32, rect.y + rect.height as i32 - 1, color);
        }

        // Left and right edges
        for y in 0..rect.height {
            self.put_pixel(rect.x, rect.y + y as i32, color);
            self.put_pixel(rect.x + rect.width as i32 - 1, rect.y + y as i32, color);
        }
    }

    /// Draw a simple 8x8 character (ASCII only, very basic)
    pub fn draw_char(&mut self, x: i32, y: i32, ch: char, color: Color) {
        // Very basic 8x8 bitmap font for ASCII 32-126
        let bitmap = get_char_bitmap(ch);

        for row in 0..8 {
            for col in 0..8 {
                if (bitmap[row] >> (7 - col)) & 1 != 0 {
                    self.put_pixel(x + col as i32, y + row as i32, color);
                }
            }
        }
    }

    /// Draw text string
    pub fn draw_text(&mut self, x: i32, y: i32, text: &str, color: Color) {
        let mut offset_x = 0;
        for ch in text.chars() {
            self.draw_char(x + offset_x, y, ch, color);
            offset_x += 8;
        }
    }
}

/// Get 8x8 bitmap for a character (very basic ASCII font)
fn get_char_bitmap(ch: char) -> [u8; 8] {
    match ch {
        ' ' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
        'A' => [0x18, 0x24, 0x42, 0x42, 0x7E, 0x42, 0x42, 0x00],
        'B' => [0x7C, 0x42, 0x42, 0x7C, 0x42, 0x42, 0x7C, 0x00],
        'C' => [0x3C, 0x42, 0x40, 0x40, 0x40, 0x42, 0x3C, 0x00],
        'H' => [0x42, 0x42, 0x42, 0x7E, 0x42, 0x42, 0x42, 0x00],
        'e' => [0x00, 0x00, 0x3C, 0x42, 0x7E, 0x40, 0x3C, 0x00],
        'l' => [0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x0C, 0x00],
        'o' => [0x00, 0x00, 0x3C, 0x42, 0x42, 0x42, 0x3C, 0x00],
        'W' => [0x42, 0x42, 0x42, 0x5A, 0x66, 0x42, 0x42, 0x00],
        'r' => [0x00, 0x00, 0x5C, 0x62, 0x40, 0x40, 0x40, 0x00],
        'd' => [0x02, 0x02, 0x3E, 0x42, 0x42, 0x42, 0x3E, 0x00],
        '!' => [0x18, 0x18, 0x18, 0x18, 0x00, 0x18, 0x18, 0x00],
        'C' => [0x3C, 0x42, 0x40, 0x40, 0x40, 0x42, 0x3C, 0x00],
        'i' => [0x00, 0x18, 0x00, 0x18, 0x18, 0x18, 0x0C, 0x00],
        'c' => [0x00, 0x00, 0x3C, 0x40, 0x40, 0x40, 0x3C, 0x00],
        'k' => [0x40, 0x40, 0x44, 0x48, 0x70, 0x48, 0x44, 0x00],
        'm' => [0x00, 0x00, 0x7C, 0x52, 0x52, 0x52, 0x52, 0x00],
        _ => [0x7E, 0x42, 0x42, 0x42, 0x42, 0x42, 0x7E, 0x00], // Box for unknown chars
    }
}
