//! Cursor management module

use framebuffer::Framebuffer;

/// Cursor bitmap - simple 0/1/2 format
const CURSOR_WIDTH: usize = 16;
const CURSOR_HEIGHT: usize = 24;
const CURSOR_DAMAGE_PADDING: i32 = 2;

/// Cursor color (white)
const CURSOR_COLOR: [u8; 4] = [255, 255, 255, 255]; // BGRA
/// Cursor border color (black)
const CURSOR_BORDER: [u8; 4] = [0, 0, 0, 255]; // BGRA

/// Arrow cursor bitmap (16x24 pixels)
/// 0 = transparent (don't draw), 1 = white (CURSOR_COLOR), 2 = black border (CURSOR_BORDER)
const CURSOR_BITMAP: [[u8; CURSOR_WIDTH]; CURSOR_HEIGHT] = [
    [2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [2, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [2, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [2, 1, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [2, 1, 1, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [2, 1, 1, 1, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [2, 1, 1, 1, 1, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [2, 1, 1, 1, 1, 1, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0],
    [2, 1, 1, 1, 1, 1, 1, 1, 2, 0, 0, 0, 0, 0, 0, 0],
    [2, 1, 1, 1, 1, 1, 1, 1, 1, 2, 0, 0, 0, 0, 0, 0],
    [2, 1, 1, 1, 1, 1, 1, 1, 1, 1, 2, 0, 0, 0, 0, 0],
    [2, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 2, 0, 0, 0, 0],
    [2, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, 0, 0, 0, 0],
    [2, 1, 1, 1, 1, 1, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0],
    [2, 1, 1, 2, 2, 1, 1, 1, 2, 0, 0, 0, 0, 0, 0, 0],
    [2, 1, 2, 0, 0, 2, 1, 1, 2, 0, 0, 0, 0, 0, 0, 0],
    [2, 2, 0, 0, 0, 0, 2, 1, 1, 2, 0, 0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0, 0, 2, 1, 1, 2, 0, 0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0, 0, 0, 2, 1, 1, 2, 0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0, 0, 0, 2, 1, 1, 2, 0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0, 0, 0, 0, 2, 1, 1, 2, 0, 0, 0, 0],
    [0, 0, 0, 0, 0, 0, 0, 0, 2, 1, 2, 0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 2, 0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0],
];

fn scale_milli_or_default(scale_milli: u32) -> u32 {
    scale_milli.max(1)
}

fn scaled_len(value: usize, scale_milli: u32) -> u32 {
    let scale_milli = scale_milli_or_default(scale_milli) as usize;
    ((value * scale_milli + 999) / 1000).max(1) as u32
}

fn scaled_cell_bounds(index: usize, scale_milli: u32) -> (u32, u32) {
    let scale_milli = scale_milli_or_default(scale_milli) as usize;
    let start = index * scale_milli / 1000;
    let end = ((index + 1) * scale_milli + 999) / 1000;
    (start as u32, end.max(start + 1) as u32)
}

/// Cursor state
pub struct Cursor {
    pub x: i32,
    pub y: i32,
    prev_x: i32,
    prev_y: i32,
    pub width: u32,
    pub height: u32,
    scale_milli: u32,
    needs_redraw: bool,
}

impl Cursor {
    /// Create a new cursor
    pub fn new(scale_milli: u32) -> Self {
        let scale_milli = scale_milli_or_default(scale_milli);
        Self {
            x: 0,
            y: 0,
            prev_x: 0,
            prev_y: 0,
            width: scaled_len(CURSOR_WIDTH, scale_milli),
            height: scaled_len(CURSOR_HEIGHT, scale_milli),
            scale_milli,
            needs_redraw: true,
        }
    }

    /// Set cursor position directly (absolute positioning)
    pub fn set_position(&mut self, x: i32, y: i32, screen_width: u32, screen_height: u32) -> bool {
        let old_x = self.x;
        let old_y = self.y;
        self.x = x.max(0).min(screen_width as i32 - 1);
        self.y = y.max(0).min(screen_height as i32 - 1);
        let moved = old_x != self.x || old_y != self.y;
        if moved {
            self.needs_redraw = true;
        }
        moved
    }

    /// Update cursor position with bounds checking (relative movement)
    pub fn update_position(
        &mut self,
        dx: i32,
        dy: i32,
        screen_width: u32,
        screen_height: u32,
    ) -> bool {
        let old_x = self.x;
        let old_y = self.y;
        self.x = (self.x + dx).max(0).min(screen_width as i32 - 1);
        self.y = (self.y + dy).max(0).min(screen_height as i32 - 1);
        let moved = old_x != self.x || old_y != self.y;
        if moved {
            self.needs_redraw = true;
        }
        moved
    }

    /// Check if cursor needs redraw
    pub fn needs_redraw(&self) -> bool {
        self.needs_redraw
    }

    /// Mark cursor as redrawn
    pub fn mark_drawn(&mut self) {
        self.needs_redraw = false;
        self.prev_x = self.x;
        self.prev_y = self.y;
    }

    /// Draw cursor from bitmap
    pub fn draw(&self, fb: &mut Framebuffer) {
        let cx = self.x as u32;
        let cy = self.y as u32;

        for y in 0..CURSOR_HEIGHT {
            for x in 0..CURSOR_WIDTH {
                let pixel = CURSOR_BITMAP[y][x];
                // Skip transparent pixels (0)
                if pixel == 0 {
                    continue;
                }

                let (x0, x1) = scaled_cell_bounds(x, self.scale_milli);
                let (y0, y1) = scaled_cell_bounds(y, self.scale_milli);

                let color = if pixel == 2 {
                    CURSOR_BORDER
                } else {
                    CURSOR_COLOR
                };

                for dy in y0..y1 {
                    for dx in x0..x1 {
                        let _ = fb.write_pixel(cx.saturating_add(dx), cy.saturating_add(dy), color);
                    }
                }
            }
        }
    }

    /// Draw cursor directly to a buffer (for compositing)
    pub fn draw_to_buffer(
        &self,
        buffer: &mut [u8],
        screen_width: u32,
        screen_height: u32,
        bytes_per_pixel: u32,
    ) {
        let stride = screen_width * bytes_per_pixel;
        self.draw_to_buffer_direct(buffer, screen_width, screen_height, bytes_per_pixel, stride);
    }

    /// Draw cursor directly to a buffer with custom stride
    pub fn draw_to_buffer_direct(
        &self,
        buffer: &mut [u8],
        screen_width: u32,
        screen_height: u32,
        bytes_per_pixel: u32,
        stride: u32,
    ) {
        self.draw_to_buffer_direct_clipped(
            buffer,
            screen_width,
            screen_height,
            bytes_per_pixel,
            stride,
            None,
        );
    }

    /// Draw cursor directly to a buffer with custom stride and optional clip.
    pub fn draw_to_buffer_direct_clipped(
        &self,
        buffer: &mut [u8],
        screen_width: u32,
        screen_height: u32,
        bytes_per_pixel: u32,
        stride: u32,
        clip_rect: Option<(i32, i32, u32, u32)>,
    ) {
        let cx = self.x;
        let cy = self.y;
        let clip = clip_rect
            .map(|(x, y, w, h)| (x, y, x.saturating_add(w as i32), y.saturating_add(h as i32)));

        for y in 0..CURSOR_HEIGHT {
            for x in 0..CURSOR_WIDTH {
                let pixel = CURSOR_BITMAP[y][x];
                // Skip transparent pixels (0)
                if pixel == 0 {
                    continue;
                }

                let color = if pixel == 2 {
                    CURSOR_BORDER
                } else {
                    CURSOR_COLOR
                };

                let (x0, x1) = scaled_cell_bounds(x, self.scale_milli);
                let (y0, y1) = scaled_cell_bounds(y, self.scale_milli);
                for dy in y0..y1 {
                    for dx in x0..x1 {
                        let screen_x = cx + dx as i32;
                        let screen_y = cy + dy as i32;

                        // Bounds check
                        if screen_x < 0
                            || screen_x >= screen_width as i32
                            || screen_y < 0
                            || screen_y >= screen_height as i32
                        {
                            continue;
                        }
                        if let Some((clip_x0, clip_y0, clip_x1, clip_y1)) = clip {
                            if screen_x < clip_x0
                                || screen_x >= clip_x1
                                || screen_y < clip_y0
                                || screen_y >= clip_y1
                            {
                                continue;
                            }
                        }

                        let offset = ((screen_y as u32 * stride)
                            + (screen_x as u32 * bytes_per_pixel))
                            as usize;

                        if offset + 4 <= buffer.len() {
                            buffer[offset] = color[0]; // B
                            buffer[offset + 1] = color[1]; // G
                            buffer[offset + 2] = color[2]; // R
                            buffer[offset + 3] = color[3]; // A
                        }
                    }
                }
            }
        }
    }

    /// Clear previous cursor by restoring saved pixels
    pub fn clear_prev(&self, fb: &mut Framebuffer, _bg_color: [u8; 4]) {
        // This is now a no-op since we redraw the dirty region
    }

    /// Get the dirty region that needs redrawing (union of prev and current cursor)
    pub fn get_dirty_region(&self) -> (i32, i32, u32, u32) {
        let min_x = self
            .prev_x
            .min(self.x)
            .saturating_sub(CURSOR_DAMAGE_PADDING);
        let min_y = self
            .prev_y
            .min(self.y)
            .saturating_sub(CURSOR_DAMAGE_PADDING);
        let max_x = (self.prev_x + self.width as i32)
            .max(self.x + self.width as i32)
            .saturating_add(CURSOR_DAMAGE_PADDING);
        let max_y = (self.prev_y + self.height as i32)
            .max(self.y + self.height as i32)
            .saturating_add(CURSOR_DAMAGE_PADDING);
        (min_x, min_y, (max_x - min_x) as u32, (max_y - min_y) as u32)
    }

    /// Update prev position to current
    pub fn update_prev(&mut self) {
        self.prev_x = self.x;
        self.prev_y = self.y;
    }
}
