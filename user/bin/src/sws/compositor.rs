//! Compositor module - manages window composition and rendering

use super::cursor::Cursor;
use super::input::{CompositorInputEvent, InputManager, key_codes};
use super::ipc::{IpcEvent, IpcServer, send_message_to_window, send_message_to_client};
use super::window::WindowManager;
use framebuffer::Framebuffer;
use std::println;
use std::thread::yield_now;
use std::vec::Vec;
use sws_protocol;

// NOTE: The compositor intentionally does NOT manage a manual VRAM mmap mapping.
// Rendering goes through the framebuffer library (which may internally use mmap).

// Debug: dump VRAM mmap range and window buffer ranges.
// This helps confirm whether corruption is caused by virtual-address overlap.
const LOG_MEMORY_LAYOUT: bool = true;

// Debug: validate that compositor output in VRAM matches what we expect
// from window buffers (helps catch stride/offset/blit bugs).
const LOG_RENDER_VALIDATION: bool = false;

// Feature flag: Enable dirty rect optimization (false = always full redraw)
// Disable this if you suspect partial redraw is causing rendering artifacts
const ENABLE_DIRTY_RECT: bool = true;

/// Compositor - the main window server with proper layer compositing
pub struct Compositor {
    framebuffer: Framebuffer,
    window_manager: WindowManager,
    ipc_server: IpcServer,
    cursor: Cursor,
    screen_width: u32,
    screen_height: u32,
    bg_color: [u8; 4],
    bytes_per_pixel: u32,
    backbuffer: Vec<u8>,
    backbuffer_stride: u32,
    full_redraw_needed: bool,
    pending_damage: Option<(i32, i32, u32, u32)>,
    event_counter: u64,
    left_button_down: bool,
    move_drag: Option<MoveDragState>,
    resize_drag: Option<ResizeDragState>,
    resize_outline: Option<(i32, i32, u32, u32)>,
    workarea: Option<(i32, i32, u32, u32)>,
}

#[derive(Debug, Clone, Copy)]
struct MoveDragState {
    window_id: u32,
    grab_cursor_x: i32,
    grab_cursor_y: i32,
    start_window_x: i32,
    start_window_y: i32,
}

#[derive(Debug, Clone, Copy)]
struct ResizeDragState {
    window_id: u32,
    grab_cursor_x: i32,
    grab_cursor_y: i32,
    start_width: u32,
    start_height: u32,
    last_width: u32,
    last_height: u32,
}

const RESIZE_GRIP_PX: i32 = 8;
const MIN_WINDOW_WIDTH: u32 = 64;
const MIN_WINDOW_HEIGHT: u32 = 64;

impl Compositor {
    /// Create a new compositor
    pub fn new() -> Result<Self, &'static str> {
        println!("[Compositor] Starting initialization...");

        // Open framebuffer
        let framebuffer =
            Framebuffer::open("/dev/fb0").map_err(|_| "Failed to open framebuffer")?;

        // Get screen dimensions
        let var_info = framebuffer
            .get_var_screen_info()
            .map_err(|_| "Failed to get screen info")?;

        let fix_info = framebuffer
            .get_fix_screen_info()
            .map_err(|_| "Failed to get fixed screen info")?;

        let screen_width = var_info.xres;
        let screen_height = var_info.yres;
        let bytes_per_pixel = 4; // BGRA

        println!("[Compositor] Screen: {}x{}", screen_width, screen_height);
        println!(
            "[Compositor] Framebuffer: bpp={} line_length={} smem_len={}",
            var_info.bits_per_pixel, fix_info.line_length, fix_info.smem_len
        );

        // Start input thread
        InputManager::start_input_thread(screen_width, screen_height)?;

        // Initialize IPC server
        let mut ipc_server = IpcServer::new("/tmp/sws.sock")?;
        ipc_server.listen()?;

        // Initialize window manager
        let window_manager = WindowManager::new();

        // Initialize cursor at center
        let mut cursor = Cursor::new();
        cursor.x = (screen_width / 2) as i32;
        cursor.y = (screen_height / 2) as i32;
        // Keep prev position consistent to avoid an oversized first dirty region.
        cursor.mark_drawn();

        // Slightly desaturated charcoal background to better fit desktop surfaces.
        let bg_color = [24, 28, 36, 255];

        let backbuffer_stride = screen_width * bytes_per_pixel;
        let buffer_size = (screen_width * screen_height * bytes_per_pixel) as usize;
        let mut backbuffer = Vec::with_capacity(buffer_size);
        backbuffer.resize(buffer_size, 0);

        Ok(Self {
            framebuffer,
            window_manager,
            ipc_server,
            cursor,
            screen_width,
            screen_height,
            bg_color,
            bytes_per_pixel,
            backbuffer,
            backbuffer_stride,
            full_redraw_needed: true,
            pending_damage: None,
            event_counter: 0,
            left_button_down: false,
            move_drag: None,
            resize_drag: None,
            resize_outline: None,
            workarea: None,
        })
    }

    fn draw_outline_rect_to_buffer(
        screen_width: u32,
        screen_height: u32,
        bytes_per_pixel: u32,
        buffer: &mut [u8],
        stride: u32,
        rect: (i32, i32, u32, u32),
        clip_rect: Option<(i32, i32, u32, u32)>,
    ) {
        let (x, y, w, h) = rect;
        if w == 0 || h == 0 {
            return;
        }

        // High-contrast outline (outer black, inner white).
        // This stays visible regardless of the window background.
        let outer = [0u8, 0u8, 0u8, 255u8];
        let inner = [255u8, 255u8, 255u8, 255u8];

        let x0 = x;
        let y0 = y;
        let x1 = x.saturating_add(w as i32).saturating_sub(1);
        let y1 = y.saturating_add(h as i32).saturating_sub(1);

        let mut draw_outline = |rx0: i32, ry0: i32, rx1: i32, ry1: i32, color: [u8; 4]| {
            if rx1 < rx0 || ry1 < ry0 {
                return;
            }

            // Top/bottom
            for sx in rx0..=rx1 {
                for sy in [ry0, ry1] {
                    if sx < 0 || sx >= screen_width as i32 || sy < 0 || sy >= screen_height as i32 {
                        continue;
                    }
                    if let Some((clip_x, clip_y, clip_w, clip_h)) = clip_rect {
                        if sx < clip_x
                            || sx >= clip_x + clip_w as i32
                            || sy < clip_y
                            || sy >= clip_y + clip_h as i32
                        {
                            continue;
                        }
                    }
                    let off = ((sy as u32 * stride) + (sx as u32 * bytes_per_pixel)) as usize;
                    if off + 4 <= buffer.len() {
                        buffer[off] = color[0];
                        buffer[off + 1] = color[1];
                        buffer[off + 2] = color[2];
                        buffer[off + 3] = color[3];
                    }
                }
            }

            // Left/right
            for sy in ry0..=ry1 {
                for sx in [rx0, rx1] {
                    if sx < 0 || sx >= screen_width as i32 || sy < 0 || sy >= screen_height as i32 {
                        continue;
                    }
                    if let Some((clip_x, clip_y, clip_w, clip_h)) = clip_rect {
                        if sx < clip_x
                            || sx >= clip_x + clip_w as i32
                            || sy < clip_y
                            || sy >= clip_y + clip_h as i32
                        {
                            continue;
                        }
                    }
                    let off = ((sy as u32 * stride) + (sx as u32 * bytes_per_pixel)) as usize;
                    if off + 4 <= buffer.len() {
                        buffer[off] = color[0];
                        buffer[off + 1] = color[1];
                        buffer[off + 2] = color[2];
                        buffer[off + 3] = color[3];
                    }
                }
            }
        };

        // Outer black outline.
        draw_outline(x0, y0, x1, y1, outer);

        // Inner white outline (1px inset) when possible.
        if w > 2 && h > 2 {
            draw_outline(x0 + 1, y0 + 1, x1 - 1, y1 - 1, inner);
        }
    }

    /// Initialize display (clear screen and draw cursor)
    pub fn init_display(&mut self) -> Result<(), &'static str> {
        println!("[Compositor] Initializing display...");

        println!("[Compositor] No debug windows created (clean desktop startup)");

        self.dump_memory_layout("after init_display (empty)");

        // Initial full composite
        self.full_redraw_needed = true;
        self.composite_and_present()?;

        println!("[Compositor] Display initialized");

        Ok(())
    }

    fn dump_memory_layout(&self, reason: &str) {
        if !LOG_MEMORY_LAYOUT {
            return;
        }

        println!("[Compositor] === Memory layout dump: {} ===", reason);

        // Backbuffer lives on the heap; log its virtual range and fingerprint.
        // This helps detect accidental aliasing/corruption and confirms it doesn't overlap VRAM.
        let bb_start = self.backbuffer.as_ptr() as usize;
        let bb_len = self.backbuffer.len();
        let bb_end = bb_start.saturating_add(bb_len);
        let bb_fp = Self::buffer_fingerprint(&self.backbuffer);
        println!(
            "[Compositor] backbuffer: 0x{:x}..0x{:x} ({} bytes) stride={} fp=0x{:08x}",
            bb_start, bb_end, bb_len, self.backbuffer_stride, bb_fp
        );

        // Best-effort stack location hint: address of a local variable.
        // We don't know the full stack range here, but if this falls inside VRAM it is a red flag.
        let stack_marker: u8 = 0;
        let sp_hint = (&stack_marker as *const u8) as usize;
        println!("[Compositor] stack marker addr: 0x{:x}", sp_hint);

        if let Some((addr, size)) = self.framebuffer.get_mapping_info() {
            let vram_start = addr;
            let vram_end = addr.saturating_add(size);
            println!(
                "[Compositor] VRAM mmap (framebuffer lib): 0x{:x}..0x{:x} ({} bytes)",
                vram_start, vram_end, size
            );

            let bb_overlap = bb_start < vram_end && vram_start < bb_end;
            if bb_overlap {
                println!("[Compositor] WARNING: backbuffer overlaps framebuffer mapping!");
            }

            if sp_hint >= vram_start && sp_hint < vram_end {
                println!("[Compositor] WARNING: stack marker is inside framebuffer mapping!");
            }
        } else {
            println!("[Compositor] VRAM mmap (framebuffer lib): (unavailable)");
        }

        let mut ranges: Vec<(u32, usize, usize, usize)> = Vec::new();
        for w in self.window_manager.get_windows() {
            // Check for SHM-backed window
            if let Some(shm_addr) = w.shm_mapped_addr {
                let buffer_size = (w.width as usize)
                    .saturating_mul(w.height as usize)
                    .saturating_mul(4);
                let end = shm_addr.saturating_add(buffer_size);
                ranges.push((w.id, shm_addr, end, buffer_size));

                println!(
                    "[Compositor] window #{} SHM: 0x{:x}..0x{:x} ({} bytes) [SHM-backed]",
                    w.id, shm_addr, end, buffer_size
                );

                if let Some((vram_start, vram_size)) = self.framebuffer.get_mapping_info() {
                    let vram_end = vram_start.saturating_add(vram_size);
                    let overlap = shm_addr < vram_end && vram_start < end;
                    if overlap {
                        println!(
                            "[Compositor] WARNING: window #{} SHM overlaps framebuffer mapping!",
                            w.id
                        );
                    }
                }

                let overlap_bb = shm_addr < bb_end && bb_start < end;
                if overlap_bb {
                    println!(
                        "[Compositor] WARNING: window #{} SHM overlaps backbuffer!",
                        w.id
                    );
                }
            } else if let Some(ref buf) = w.buffer {
                // Legacy Vec-backed window
                let start = buf.as_ptr() as usize;
                let len = buf.len();
                let end = start.saturating_add(len);
                ranges.push((w.id, start, end, len));

                let fp = Self::buffer_fingerprint(buf);
                println!(
                    "[Compositor] window #{} buffer: 0x{:x}..0x{:x} ({} bytes) fp=0x{:08x}",
                    w.id, start, end, len, fp
                );

                if let Some((vram_start, vram_size)) = self.framebuffer.get_mapping_info() {
                    let vram_end = vram_start.saturating_add(vram_size);
                    let overlap = start < vram_end && vram_start < end;
                    if overlap {
                        println!(
                            "[Compositor] WARNING: window #{} buffer overlaps framebuffer mapping!",
                            w.id
                        );
                    }
                }

                let overlap_bb = start < bb_end && bb_start < end;
                if overlap_bb {
                    println!(
                        "[Compositor] WARNING: window #{} buffer overlaps backbuffer!",
                        w.id
                    );
                }
            } else {
                println!("[Compositor] window #{} buffer: (none)", w.id);
            }
        }

        // Check overlap between window buffers themselves (should never happen).
        if ranges.len() >= 2 {
            ranges.sort_by_key(|(_id, start, _end, _len)| *start);
            for i in 1..ranges.len() {
                let (prev_id, prev_start, prev_end, _prev_len) = ranges[i - 1];
                let (id, start, _end, _len) = ranges[i];
                if start < prev_end {
                    println!(
                        "[Compositor] WARNING: window buffers overlap: #{} (0x{:x}..0x{:x}) and #{} (starts 0x{:x})",
                        prev_id, prev_start, prev_end, id, start
                    );
                }
            }
        }

        // Best-effort: print current program break (sbrk(0)).
        // This is useful to see if heap grows towards the VRAM mapping.
        {
            use std::syscall::{Syscall, syscall1};
            let brk_now = syscall1(Syscall::Sbrk, 0);
            println!("[Compositor] sbrk(0) -> 0x{:x}", brk_now);
        }
    }

    fn buffer_fingerprint(buf: &[u8]) -> u32 {
        // Cheap fingerprint to detect unexpected buffer mutations.
        // Mix a small prefix + suffix and a stride sample to reduce overhead.
        let mut x: u32 = 0x811c_9dc5;

        let take = core::cmp::min(256, buf.len());
        for &b in &buf[..take] {
            x = x.rotate_left(5) ^ (b as u32);
        }

        if buf.len() > 256 {
            let tail_take = core::cmp::min(256, buf.len());
            for &b in &buf[buf.len() - tail_take..] {
                x = x.rotate_left(5) ^ (b as u32);
            }
        }

        // Sample every ~4KB to catch larger-scale corruption.
        let mut i = 0usize;
        while i < buf.len() {
            x = x.rotate_left(5) ^ (buf[i] as u32);
            i = i.saturating_add(4096);
        }

        x
    }

    /// Fill buffer with gradient (for testing, static method)
    #[allow(dead_code)]
    fn fill_buffer_gradient(buffer: &mut [u8], width: u32, height: u32, base_color: [u8; 4]) {
        for y in 0..height {
            for x in 0..width {
                let offset = ((y * width + x) * 4) as usize;
                if offset + 4 <= buffer.len() {
                    // Create gradient effect
                    let intensity =
                        (x as f32 / width as f32 * 0.5 + y as f32 / height as f32 * 0.5) as u8;
                    buffer[offset] = base_color[0].saturating_sub(intensity); // B
                    buffer[offset + 1] = base_color[1].saturating_sub(intensity); // G
                    buffer[offset + 2] = base_color[2].saturating_sub(intensity); // R
                    buffer[offset + 3] = base_color[3]; // A
                }
            }
        }
    }

    fn clamp_rect_to_screen(&self, rect: (i32, i32, u32, u32)) -> Option<(i32, i32, u32, u32)> {
        let (x, y, w, h) = rect;
        if w == 0 || h == 0 {
            return None;
        }

        let sx0 = x.max(0).min(self.screen_width as i32);
        let sy0 = y.max(0).min(self.screen_height as i32);
        let sx1 = (x.saturating_add(w as i32))
            .max(0)
            .min(self.screen_width as i32);
        let sy1 = (y.saturating_add(h as i32))
            .max(0)
            .min(self.screen_height as i32);

        let cw = (sx1 - sx0).max(0) as u32;
        let ch = (sy1 - sy0).max(0) as u32;
        if cw == 0 || ch == 0 {
            None
        } else {
            Some((sx0, sy0, cw, ch))
        }
    }

    fn add_pending_damage(&mut self, rect: (i32, i32, u32, u32)) {
        if !ENABLE_DIRTY_RECT {
            self.full_redraw_needed = true;
            return;
        }

        let Some((sx0, sy0, w, h)) = self.clamp_rect_to_screen(rect) else {
            return;
        };

        self.pending_damage = match self.pending_damage {
            None => Some((sx0, sy0, w, h)),
            Some((px, py, pw, ph)) => {
                let px1 = (px as i64).saturating_add(pw as i64);
                let py1 = (py as i64).saturating_add(ph as i64);
                let nx1 = (sx0 as i64).saturating_add(w as i64);
                let ny1 = (sy0 as i64).saturating_add(h as i64);
                let x0 = core::cmp::min(px as i64, sx0 as i64);
                let y0 = core::cmp::min(py as i64, sy0 as i64);
                let x1 = core::cmp::max(px1, nx1);
                let y1 = core::cmp::max(py1, ny1);
                let uw = (x1 - x0).max(0) as u32;
                let uh = (y1 - y0).max(0) as u32;
                Some((x0 as i32, y0 as i32, uw, uh))
            }
        };
    }

    /// Composite all layers directly to VRAM (or framebuffer as fallback)
    fn composite_and_present(&mut self) -> Result<(), &'static str> {
        fn union_rect(
            a: Option<(i32, i32, u32, u32)>,
            b: Option<(i32, i32, u32, u32)>,
        ) -> Option<(i32, i32, u32, u32)> {
            match (a, b) {
                (None, None) => None,
                (Some(r), None) | (None, Some(r)) => Some(r),
                (Some((ax, ay, aw, ah)), Some((bx, by, bw, bh))) => {
                    if aw == 0 || ah == 0 {
                        return Some((bx, by, bw, bh));
                    }
                    if bw == 0 || bh == 0 {
                        return Some((ax, ay, aw, ah));
                    }

                    let ax1 = (ax as i64).saturating_add(aw as i64);
                    let ay1 = (ay as i64).saturating_add(ah as i64);
                    let bx1 = (bx as i64).saturating_add(bw as i64);
                    let by1 = (by as i64).saturating_add(bh as i64);

                    let x0 = core::cmp::min(ax as i64, bx as i64);
                    let y0 = core::cmp::min(ay as i64, by as i64);
                    let x1 = core::cmp::max(ax1, bx1);
                    let y1 = core::cmp::max(ay1, by1);
                    let w = (x1 - x0).max(0) as u32;
                    let h = (y1 - y0).max(0) as u32;
                    Some((x0 as i32, y0 as i32, w, h))
                }
            }
        }

        let dirty = if !ENABLE_DIRTY_RECT {
            // Force full redraw when dirty rect optimization is disabled
            None
        } else if self.full_redraw_needed {
            None
        } else {
            let cursor_dirty = if self.cursor.needs_redraw() {
                Some(self.cursor.get_dirty_region())
            } else {
                None
            };
            union_rect(self.pending_damage, cursor_dirty)
        };

        // Always use framebuffer API for presentation.
        // This avoids compositor-managed mmap and keeps correctness centralized.
        self.composite_via_framebuffer(dirty)?;

        // Flush to display
        self.framebuffer
            .flush()
            .map_err(|_| "Failed to flush framebuffer")?;

        self.full_redraw_needed = false;
        self.pending_damage = None;
        Ok(())
    }

    fn validate_vram_samples(
        &self,
        vram: &[u8],
        stride: u32,
        dirty: Option<(i32, i32, u32, u32)>,
        reason: &str,
    ) {
        if !LOG_RENDER_VALIDATION {
            return;
        }

        // Pick coordinates that avoid the default cursor position (center) and
        // sample both corners and the center for sanity.
        let mut samples: Vec<(u32, u32, &'static str)> = Vec::new();
        samples.push((0, 0, "bg top-left"));
        samples.push((10, 10, "bg near top-left"));
        samples.push((self.screen_width / 2, self.screen_height / 2, "bg center"));
        samples.push((
            self.screen_width.saturating_sub(20),
            self.screen_height / 2,
            "bg mid-right",
        ));
        samples.push((
            self.screen_width.saturating_sub(20),
            self.screen_height.saturating_sub(20),
            "bg bottom-right",
        ));

        // For incremental redraw, also validate a point inside the dirty region.
        if let Some((dx, dy, dw, dh)) = dirty {
            if dw > 0 && dh > 0 {
                let cx = (dx + (dw as i32 / 2)).max(0) as u32;
                let cy = (dy + (dh as i32 / 2)).max(0) as u32;
                samples.push((cx, cy, "inside dirty region center"));
            }
        }

        match dirty {
            Some((dx, dy, dw, dh)) => {
                println!(
                    "[Compositor] === VRAM sample validation: {} (dirty=({}, {}) {}x{}) ===",
                    reason, dx, dy, dw, dh
                );
            }
            None => {
                println!("[Compositor] === VRAM sample validation: {} ===", reason);
            }
        }

        for (x, y, label) in samples {
            if x >= self.screen_width || y >= self.screen_height {
                continue;
            }

            // Cursor is an overlay; expected_pixel_at_with_source does not account for it.
            // Skip samples that fall within the cursor bounding box to avoid false mismatches.
            let cx0 = self.cursor.x;
            let cy0 = self.cursor.y;
            let cx1 = cx0.saturating_add(self.cursor.width as i32);
            let cy1 = cy0.saturating_add(self.cursor.height as i32);
            let xi = x as i32;
            let yi = y as i32;
            if xi >= cx0 && xi < cx1 && yi >= cy0 && yi < cy1 {
                println!("[Compositor] skip {} ({},{}) under cursor", label, x, y);
                continue;
            }

            if let Some((dx, dy, dw, dh)) = dirty {
                let inside = xi >= dx && xi < dx + dw as i32 && yi >= dy && yi < dy + dh as i32;
                if !inside {
                    println!(
                        "[Compositor] skip {} ({},{}) outside dirty region",
                        label, x, y
                    );
                    continue;
                }
            }

            let off = (y as usize)
                .saturating_mul(stride as usize)
                .saturating_add((x as usize).saturating_mul(self.bytes_per_pixel as usize));
            if off + 4 > vram.len() {
                println!(
                    "[Compositor] skip {} ({},{}) out of VRAM range off=0x{:x}",
                    label, x, y, off
                );
                continue;
            }

            let actual = [vram[off], vram[off + 1], vram[off + 2], vram[off + 3]];
            let (expected, src) = self.expected_pixel_at_with_source(x, y);

            if actual != expected {
                println!(
                    "[Compositor] MISMATCH {} ({},{}) actual={:?} expected={:?} src={}",
                    label, x, y, actual, expected, src
                );
            } else {
                println!(
                    "[Compositor] ok {} ({},{}) value={:?} src={}",
                    label, x, y, actual, src
                );
            }
        }
    }

    fn expected_pixel_at_with_source(&self, x: u32, y: u32) -> ([u8; 4], std::string::String) {
        let sx = x as i32;
        let sy = y as i32;

        // Top-most window wins.
        if let Some(window) = self
            .window_manager
            .get_windows()
            .iter()
            .rev()
            .find(|w| w.visible && w.contains_point(sx, sy))
        {
            let local_x = (sx - window.x) as u32;
            let local_y = (sy - window.y) as u32;
            let is_border = local_x == 0
                || local_y == 0
                || local_x + 1 == window.width
                || local_y + 1 == window.height;

            if is_border {
                if window.focused {
                    (
                        [50, 50, 150, 255],
                        std::format!("window#{} border(focused)", window.id),
                    )
                } else {
                    (
                        [100, 100, 100, 255],
                        std::format!("window#{} border", window.id),
                    )
                }
            } else if let Some(shm_addr) = window.shm_mapped_addr {
                // SHM-backed window
                let wo = ((local_y as usize)
                    .saturating_mul(window.width as usize)
                    .saturating_add(local_x as usize))
                .saturating_mul(4);

                let buffer_size = (window.width as usize)
                    .saturating_mul(window.height as usize)
                    .saturating_mul(4);

                if wo + 4 <= buffer_size {
                    unsafe {
                        let ptr = shm_addr as *const u8;
                        (
                            [
                                *ptr.add(wo),
                                *ptr.add(wo + 1),
                                *ptr.add(wo + 2),
                                *ptr.add(wo + 3),
                            ],
                            std::format!(
                                "window#{} SHM local=({}, {}) off=0x{:x}",
                                window.id,
                                local_x,
                                local_y,
                                wo
                            ),
                        )
                    }
                } else {
                    (self.bg_color, std::format!("window#{} SHM OOB", window.id))
                }
            } else if let Some(ref buf) = window.buffer {
                // Legacy Vec-backed window
                let wo = ((local_y as usize)
                    .saturating_mul(window.width as usize)
                    .saturating_add(local_x as usize))
                .saturating_mul(4);

                if wo + 4 <= buf.len() {
                    (
                        [buf[wo], buf[wo + 1], buf[wo + 2], buf[wo + 3]],
                        std::format!(
                            "window#{} buffer local=({}, {}) off=0x{:x}",
                            window.id,
                            local_x,
                            local_y,
                            wo
                        ),
                    )
                } else {
                    (
                        self.bg_color,
                        std::format!("window#{} buffer OOB", window.id),
                    )
                }
            } else if window.focused {
                (
                    [150, 150, 200, 255],
                    std::format!("window#{} placeholder(focused)", window.id),
                )
            } else {
                (
                    [180, 180, 180, 255],
                    std::format!("window#{} placeholder", window.id),
                )
            }
        } else {
            (self.bg_color, std::string::String::from("bg"))
        }
    }

    /// clip_rect: (x, y, width, height) in screen coordinates
    fn draw_window_to_buffer_clipped(
        screen_width: u32,
        screen_height: u32,
        bytes_per_pixel: u32,
        window: &super::window::Window,
        buffer: &mut [u8],
        stride: u32,
        clip_rect: Option<(i32, i32, u32, u32)>,
    ) {
        // Check if window uses SHM or Vec buffer
        if let Some(shm_addr) = window.shm_mapped_addr {
            // SHM-backed window: read from mapped memory
            let buffer_size = if window.shm_size != 0 {
                window.shm_size
            } else {
                (window.width as usize)
                    .saturating_mul(window.height as usize)
                    .saturating_mul(4)
            };

            // Create slice from mapped address
            let window_buffer =
                unsafe { core::slice::from_raw_parts(shm_addr as *const u8, buffer_size) };

            Self::draw_window_from_buffer(
                screen_width,
                screen_height,
                bytes_per_pixel,
                window,
                window_buffer,
                buffer,
                stride,
                clip_rect,
            );
        } else if let Some(ref window_buffer) = window.buffer {
            // Legacy Vec-backed window
            Self::draw_window_from_buffer(
                screen_width,
                screen_height,
                bytes_per_pixel,
                window,
                window_buffer,
                buffer,
                stride,
                clip_rect,
            );
        } else {
            // No buffer: draw placeholder
            Self::draw_window_placeholder(
                screen_width,
                screen_height,
                bytes_per_pixel,
                window,
                buffer,
                stride,
                clip_rect,
            );
        }
    }

    /// Draw window from its shared memory buffer
    fn draw_window_from_buffer(
        screen_width: u32,
        screen_height: u32,
        bytes_per_pixel: u32,
        window: &super::window::Window,
        window_buffer: &[u8],
        screen_buffer: &mut [u8],
        stride: u32,
        clip_rect: Option<(i32, i32, u32, u32)>,
    ) {
        let win_x0 = window.x;
        let win_y0 = window.y;
        let win_x1 = win_x0.saturating_add(window.width as i32);
        let win_y1 = win_y0.saturating_add(window.height as i32);

        let mut x0 = win_x0.max(0);
        let mut y0 = win_y0.max(0);
        let mut x1 = win_x1.min(screen_width as i32);
        let mut y1 = win_y1.min(screen_height as i32);

        if let Some((clip_x, clip_y, clip_w, clip_h)) = clip_rect {
            let clip_x1 = clip_x.saturating_add(clip_w as i32);
            let clip_y1 = clip_y.saturating_add(clip_h as i32);
            x0 = x0.max(clip_x);
            y0 = y0.max(clip_y);
            x1 = x1.min(clip_x1);
            y1 = y1.min(clip_y1);
        }

        if x1 <= x0 || y1 <= y0 {
            return;
        }

        // Check if window has transparency (opacity < 1.0)
        let has_transparency = window.opacity < 1.0;

        // Copy BGRA pixels from the window buffer into the screen buffer.
        // Apply alpha blending if window has transparency.
        for sy in y0..y1 {
            let wy = (sy - win_y0) as u32;
            let screen_row_off = (sy as u32).saturating_mul(stride as u32) as usize;
            for sx in x0..x1 {
                let wx = (sx - win_x0) as u32;
                let window_offset = ((wy * window.width + wx) * 4) as usize;
                let screen_offset =
                    screen_row_off + (sx as u32).saturating_mul(bytes_per_pixel) as usize;

                if window_offset + 4 <= window_buffer.len()
                    && screen_offset + 4 <= screen_buffer.len()
                {
                    if has_transparency {
                        // Alpha blending: BGRA format
                        let src_b = window_buffer[window_offset] as u32;
                        let src_g = window_buffer[window_offset + 1] as u32;
                        let src_r = window_buffer[window_offset + 2] as u32;
                        let src_a = window_buffer[window_offset + 3] as u32;

                        // Apply window opacity to pixel alpha
                        let effective_alpha = ((src_a as f32 * window.opacity) as u32).min(255);

                        let dst_b = screen_buffer[screen_offset] as u32;
                        let dst_g = screen_buffer[screen_offset + 1] as u32;
                        let dst_r = screen_buffer[screen_offset + 2] as u32;

                        // Alpha blending formula: dst = src * alpha + dst * (1 - alpha)
                        let inv_alpha = 255 - effective_alpha;
                        let out_b = ((src_b * effective_alpha + dst_b * inv_alpha) / 255) as u8;
                        let out_g = ((src_g * effective_alpha + dst_g * inv_alpha) / 255) as u8;
                        let out_r = ((src_r * effective_alpha + dst_r * inv_alpha) / 255) as u8;

                        screen_buffer[screen_offset] = out_b;
                        screen_buffer[screen_offset + 1] = out_g;
                        screen_buffer[screen_offset + 2] = out_r;
                        screen_buffer[screen_offset + 3] = 255; // Output is always opaque
                    } else {
                        // No transparency: direct copy
                        screen_buffer[screen_offset..screen_offset + 4]
                            .copy_from_slice(&window_buffer[window_offset..window_offset + 4]);
                    }
                }
            }
        }
    }

    /// Draw placeholder window (for windows without buffers yet)
    fn draw_window_placeholder(
        screen_width: u32,
        screen_height: u32,
        bytes_per_pixel: u32,
        window: &super::window::Window,
        buffer: &mut [u8],
        stride: u32,
        clip_rect: Option<(i32, i32, u32, u32)>,
    ) {
        let window_color = if window.focused {
            [150, 150, 200, 255]
        } else {
            [180, 180, 180, 255]
        };

        for y in 0..window.height {
            for x in 0..window.width {
                let screen_x = window.x + x as i32;
                let screen_y = window.y + y as i32;

                // Screen bounds check
                if screen_x < 0
                    || screen_x >= screen_width as i32
                    || screen_y < 0
                    || screen_y >= screen_height as i32
                {
                    continue;
                }

                // Clip rect check
                if let Some((clip_x, clip_y, clip_w, clip_h)) = clip_rect {
                    if screen_x < clip_x
                        || screen_x >= clip_x + clip_w as i32
                        || screen_y < clip_y
                        || screen_y >= clip_y + clip_h as i32
                    {
                        continue;
                    }
                }

                let offset =
                    ((screen_y as u32 * stride) + (screen_x as u32 * bytes_per_pixel)) as usize;

                // Fill with window color (no borders)
                if offset + 4 <= buffer.len() {
                    buffer[offset] = window_color[0];
                    buffer[offset + 1] = window_color[1];
                    buffer[offset + 2] = window_color[2];
                    buffer[offset + 3] = window_color[3];
                }
            }
        }
    }

    /// Composite into the persistent backbuffer, then present the affected region.
    fn composite_via_framebuffer(
        &mut self,
        dirty: Option<(i32, i32, u32, u32)>,
    ) -> Result<(), &'static str> {
        let backbuffer_len = self.backbuffer.len();
        let stride = self.backbuffer_stride;

        // Clip dirty region to screen bounds.
        let (x0, y0, w, h) = match dirty {
            None => (0i32, 0i32, self.screen_width, self.screen_height),
            Some((dx, dy, dw, dh)) => {
                let sx0 = dx.max(0).min(self.screen_width as i32);
                let sy0 = dy.max(0).min(self.screen_height as i32);
                let sx1 = (dx.saturating_add(dw as i32))
                    .max(0)
                    .min(self.screen_width as i32);
                let sy1 = (dy.saturating_add(dh as i32))
                    .max(0)
                    .min(self.screen_height as i32);
                let cw = (sx1 - sx0).max(0) as u32;
                let ch = (sy1 - sy0).max(0) as u32;
                (sx0, sy0, cw, ch)
            }
        };

        if w == 0 || h == 0 {
            // Nothing to redraw.
            self.cursor.mark_drawn();
            return Ok(());
        }

        // Mutate backbuffer within a limited scope so we can immutably borrow `self`
        // afterwards for validation/present.
        {
            let backbuffer = &mut self.backbuffer;

            // Layer 1: Fill background (only within dirty region).
            for yy in 0..h {
                let sy = (y0 as u32).saturating_add(yy);
                let row_off = (sy as usize)
                    .saturating_mul(stride as usize)
                    .saturating_add((x0 as usize).saturating_mul(self.bytes_per_pixel as usize));
                let row_len = (w as usize).saturating_mul(self.bytes_per_pixel as usize);
                if row_off.saturating_add(row_len) > backbuffer_len {
                    continue;
                }
                let row = &mut backbuffer[row_off..row_off + row_len];
                for px in row.chunks_exact_mut(4) {
                    px.copy_from_slice(&self.bg_color);
                }
            }

            // Layer 2: Draw windows (clipped to dirty region).
            let clip = if dirty.is_some() {
                Some((x0, y0, w, h))
            } else {
                None
            };
            let screen_width = self.screen_width;
            let screen_height = self.screen_height;
            let bytes_per_pixel = self.bytes_per_pixel;
            for window in self.window_manager.get_windows() {
                if !window.visible {
                    continue;
                }
                Self::draw_window_to_buffer_clipped(
                    screen_width,
                    screen_height,
                    bytes_per_pixel,
                    window,
                    backbuffer,
                    stride,
                    clip,
                );
            }

            // Layer 2.5: Draw interactive resize outline (if any)
            if let Some(rect) = self.resize_outline {
                Self::draw_outline_rect_to_buffer(
                    screen_width,
                    screen_height,
                    bytes_per_pixel,
                    backbuffer,
                    stride,
                    rect,
                    clip,
                );
            }

            // Layer 3: Draw cursor
            let cursor = &self.cursor;
            cursor.draw_to_buffer_direct(
                backbuffer,
                screen_width,
                screen_height,
                bytes_per_pixel,
                stride,
            );
        }

        // Validate composition against expected pixels before presenting.
        self.validate_vram_samples(
            &self.backbuffer,
            stride,
            dirty,
            "after framebuffer composite",
        );

        // Present only the dirty region when available.
        let src_off = (y0 as usize)
            .saturating_mul(stride as usize)
            .saturating_add((x0 as usize).saturating_mul(self.bytes_per_pixel as usize));
        if src_off >= backbuffer_len {
            return Err("Backbuffer offset out of range");
        }
        let src = &self.backbuffer[src_off..];

        self.framebuffer
            .write_block_strided(x0 as u32, y0 as u32, w, h, src, stride as usize)
            .map_err(|_| "Failed to write backbuffer")?;

        self.cursor.mark_drawn();
        Ok(())
    }

    /// Process input events
    /// Handle mouse click (for window focus)
    fn handle_click(&mut self) -> Result<(), &'static str> {
        let click_x = self.cursor.x;
        let click_y = self.cursor.y;

        // Find topmost window at click position
        if let Some(win_id) = self.window_manager.window_at_point(click_x, click_y) {
            println!("[Compositor] Clicked on window #{}", win_id);

            // Change focus and bring to front
            self.window_manager.set_focus(win_id);
            self.window_manager.raise_to_top_with_type(win_id);

            // Need full redraw when Z-order changes
            self.full_redraw_needed = true;
        }

        Ok(())
    }

    /// Check if cursor is within the bounds of a window
    /// Returns window-local coordinates if inside, None if outside
    fn cursor_position_in_window(&self, window: &super::window::Window) -> Option<(i32, i32)> {
        let window_x = self.cursor.x - window.x;
        let window_y = self.cursor.y - window.y;

        // println!("[Boundary Check] Window #{}: cursor=({}, {}), window pos=({}, {}), size={}x{}, window_local=({}, {})",
        //     window_id, self.cursor.x, self.cursor.y, window.x, window.y, window.width, window.height, window_x, window_y);

        if window_x >= 0
            && window_x < window.width as i32
            && window_y >= 0
            && window_y < window.height as i32
        {
            // println!("[Boundary Check] -> INSIDE");
            Some((window_x, window_y))
        } else {
            // println!("[Boundary Check] -> OUTSIDE");
            None
        }
    }

    /// Send mouse position event to a window
    fn send_mouse_position_to_window(&self, window_id: u32, window: &super::window::Window) {
        if let Some((window_x, window_y)) = self.cursor_position_in_window(window) {
            super::ipc::send_input_to_window(
                window_id,
                0,
                super::input::event_types::EV_ABS,
                super::input::abs_codes::ABS_X,
                window_x,
            );
            super::ipc::send_input_to_window(
                window_id,
                0,
                super::input::event_types::EV_ABS,
                super::input::abs_codes::ABS_Y,
                window_y,
            );
            super::ipc::send_input_to_window(window_id, 0, super::input::event_types::EV_SYN, 0, 0);
        }
    }

    /// Main event loop
    pub fn run(&mut self) -> Result<(), &'static str> {
        println!("[Compositor] Starting main loop (multithreaded)");

        loop {
            let mut needs_redraw = false;

            // Process IPC events from global queue (non-blocking)
            let ipc_events = self.ipc_server.process_messages()?;
            // if !ipc_events.is_empty() {
            //     println!("[Compositor] Processing {} IPC events", ipc_events.len());
            // }
            for event in ipc_events {
                if self.handle_ipc_event(event)? {
                    needs_redraw = true;
                }
            }

            // Process input events from global queue (non-blocking)
            let input_events = super::input::pop_all_input_events();
            if !input_events.is_empty() {
                for event in input_events {
                    if self.handle_input_event(event)? {
                        needs_redraw = true;
                    }
                }
            }

            // Re-composite and present if needed
            if needs_redraw
                || self.full_redraw_needed
                || self.pending_damage.is_some()
                || self.cursor.needs_redraw()
            {
                if self.full_redraw_needed {
                    println!("[Compositor] Full redraw triggered");
                }
                self.composite_and_present()?;
                self.event_counter += 1;
            }

            // Sleep briefly to limit frame rate and reduce CPU usage
            // 16ms = ~60fps, adjust as needed
            // std::thread::sleep(core::time::Duration::from_millis(16));
            yield_now();

            // Periodically print Z-order (every 100 redraws)
            if self.event_counter % 100 == 0 && self.event_counter > 0 {
                use std::print;
                print!("[Compositor] Z-order check #{}: ", self.event_counter);
                for window in self.window_manager.get_windows() {
                    print!("#{}{} ", window.id, if window.focused { "(F)" } else { "" });
                }
                println!();
            }
        }
    }

    /// Handle input event from input thread
    fn handle_input_event(&mut self, event: CompositorInputEvent) -> Result<bool, &'static str> {
        match event {
            CompositorInputEvent::MouseMove { dx, dy } => {
                self.cursor
                    .update_position(dx, dy, self.screen_width, self.screen_height);

                if self.left_button_down {
                    if let Some(mut state) = self.resize_drag {
                        let old_outline = self.resize_outline;
                        let delta_x = self.cursor.x - state.grab_cursor_x;
                        let delta_y = self.cursor.y - state.grab_cursor_y;

                        let new_w = (state.start_width as i32 + delta_x)
                            .max(MIN_WINDOW_WIDTH as i32)
                            as u32;
                        let new_h = (state.start_height as i32 + delta_y)
                            .max(MIN_WINDOW_HEIGHT as i32)
                            as u32;
                        let (new_w, new_h) = self.window_manager.clamp_size_for_window(
                            state.window_id,
                            new_w,
                            new_h,
                        );
                        state.last_width = new_w;
                        state.last_height = new_h;
                        self.resize_drag = Some(state);

                        if let Some(window) = self.window_manager.get_window(state.window_id) {
                            self.resize_outline = Some((window.x, window.y, new_w, new_h));
                        }

                        if let Some(r) = old_outline {
                            self.add_pending_damage(r);
                        }
                        if let Some(r) = self.resize_outline {
                            self.add_pending_damage(r);
                        }
                        // While resizing, compositor grabs the pointer.
                        return Ok(true);
                    }
                }

                // If a window move is in progress, update the window position before
                // converting cursor coordinates into window-local space.
                if self.left_button_down {
                    if let Some(state) = self.move_drag {
                        let old_rect = self
                            .window_manager
                            .get_window(state.window_id)
                            .map(|w| (w.x, w.y, w.width, w.height));
                        let new_x = state.start_window_x + (self.cursor.x - state.grab_cursor_x);
                        let new_y = state.start_window_y + (self.cursor.y - state.grab_cursor_y);
                        self.window_manager
                            .set_window_position(state.window_id, new_x, new_y);

                        if let Some(r) = old_rect {
                            self.add_pending_damage(r);
                        }
                        if let Some(w) = self.window_manager.get_window(state.window_id) {
                            self.add_pending_damage((w.x, w.y, w.width, w.height));
                        }

                        // While moving a window, the compositor "grabs" the pointer.
                        // Avoid routing mouse moves to the currently focused client.
                        return Ok(true);
                    }
                }

                // Route mouse move to focused window (converted to absolute coordinates)
                if let Some(focused_id) = self.window_manager.get_focused_window_id() {
                    if let Some(window) = self.window_manager.get_window(focused_id) {
                        self.send_mouse_position_to_window(focused_id, window);
                    }
                }

                Ok(true)
            }
            CompositorInputEvent::MouseAbsolute { x, y } => {
                self.cursor
                    .set_position(x, y, self.screen_width, self.screen_height);

                if self.left_button_down {
                    if let Some(mut state) = self.resize_drag {
                        let old_outline = self.resize_outline;
                        let delta_x = self.cursor.x - state.grab_cursor_x;
                        let delta_y = self.cursor.y - state.grab_cursor_y;

                        let new_w = (state.start_width as i32 + delta_x)
                            .max(MIN_WINDOW_WIDTH as i32)
                            as u32;
                        let new_h = (state.start_height as i32 + delta_y)
                            .max(MIN_WINDOW_HEIGHT as i32)
                            as u32;
                        let (new_w, new_h) = self.window_manager.clamp_size_for_window(
                            state.window_id,
                            new_w,
                            new_h,
                        );
                        state.last_width = new_w;
                        state.last_height = new_h;
                        self.resize_drag = Some(state);

                        if let Some(window) = self.window_manager.get_window(state.window_id) {
                            self.resize_outline = Some((window.x, window.y, new_w, new_h));
                        }

                        if let Some(r) = old_outline {
                            self.add_pending_damage(r);
                        }
                        if let Some(r) = self.resize_outline {
                            self.add_pending_damage(r);
                        }
                        return Ok(true);
                    }
                }

                if self.left_button_down {
                    if let Some(state) = self.move_drag {
                        let old_rect = self
                            .window_manager
                            .get_window(state.window_id)
                            .map(|w| (w.x, w.y, w.width, w.height));
                        let new_x = state.start_window_x + (self.cursor.x - state.grab_cursor_x);
                        let new_y = state.start_window_y + (self.cursor.y - state.grab_cursor_y);
                        self.window_manager
                            .set_window_position(state.window_id, new_x, new_y);

                        if let Some(r) = old_rect {
                            self.add_pending_damage(r);
                        }
                        if let Some(w) = self.window_manager.get_window(state.window_id) {
                            self.add_pending_damage((w.x, w.y, w.width, w.height));
                        }

                        // While moving a window, the compositor "grabs" the pointer.
                        // Avoid routing mouse moves to the currently focused client.
                        return Ok(true);
                    }
                }

                // Route mouse position to focused window
                if let Some(focused_id) = self.window_manager.get_focused_window_id() {
                    if let Some(window) = self.window_manager.get_window(focused_id) {
                        self.send_mouse_position_to_window(focused_id, window);
                    }
                }

                Ok(true)
            }
            CompositorInputEvent::MouseButton { button, pressed } => {
                if button == key_codes::BTN_LEFT {
                    self.left_button_down = pressed;
                    if !pressed {
                        // Always exit move mode on left button release.
                        if self.move_drag.take().is_some() {
                            // No special redraw needed: the last drag motion already queued damage.
                        }

                        // Finalize resize on left button release.
                        if let Some(state) = self.resize_drag.take() {
                            let old_outline = self.resize_outline;
                            self.resize_outline = None;
                            if let Some(r) = old_outline {
                                self.add_pending_damage(r);
                            }

                            // Ask client to resize once (outline-only during drag).
                            let (width, height) = self.window_manager.clamp_size_for_window(
                                state.window_id,
                                state.last_width,
                                state.last_height,
                            );
                            let payload = sws_protocol::payload_window_configure(
                                state.window_id,
                                width,
                                height,
                            );
                            super::ipc::send_message_to_window(
                                state.window_id,
                                sws_protocol::server_msg::WINDOW_CONFIGURE,
                                payload.to_vec(),
                            );
                        }
                    }
                }

                if button == key_codes::BTN_LEFT && pressed {
                    // Determine target window under cursor.
                    if let Some(win_id) = self
                        .window_manager
                        .window_at_point(self.cursor.x, self.cursor.y)
                    {
                        self.window_manager.set_focus(win_id);
                        self.window_manager.raise_to_top_with_type(win_id);
                        self.full_redraw_needed = true;

                        // Start interactive resize if we're near the bottom/right edge.
                        if let Some(window) = self.window_manager.get_window(win_id) {
                            if let Some((wx, wy)) = self.cursor_position_in_window(window) {
                                let near_right = wx >= window.width as i32 - RESIZE_GRIP_PX;
                                let near_bottom = wy >= window.height as i32 - RESIZE_GRIP_PX;
                                // Only allow resize if window is marked as resizable
                                if (near_right || near_bottom) && window.resizable {
                                    self.move_drag = None;
                                    self.resize_drag = Some(ResizeDragState {
                                        window_id: win_id,
                                        grab_cursor_x: self.cursor.x,
                                        grab_cursor_y: self.cursor.y,
                                        start_width: window.width,
                                        start_height: window.height,
                                        last_width: window.width,
                                        last_height: window.height,
                                    });
                                    self.resize_outline =
                                        Some((window.x, window.y, window.width, window.height));
                                    return Ok(true);
                                }
                            }
                        }
                    }

                    // Normal click behavior (focus/raise).
                    self.handle_click()?;
                }

                // Route button event to focused window only if cursor is within window bounds
                if let Some(focused_id) = self.window_manager.get_focused_window_id() {
                    let window = self
                        .window_manager
                        .get_window(focused_id)
                        .ok_or("Focused window not found")?;
                    if self.cursor_position_in_window(window).is_some() {
                        super::ipc::send_input_to_window(
                            focused_id,
                            0,
                            super::input::event_types::EV_KEY,
                            button,
                            if pressed { 1 } else { 0 },
                        );
                        super::ipc::send_input_to_window(
                            focused_id,
                            0,
                            super::input::event_types::EV_SYN,
                            0,
                            0,
                        );
                    }
                }

                Ok(true)
            }
        }
    }

    /// Handle IPC events from clients
    ///
    /// Returns `Ok(true)` if an immediate redraw is required (e.g., window created/destroyed).
    /// Returns `Ok(false)` if only damage was accumulated (redraw via `pending_damage`).
    fn handle_ipc_event(&mut self, event: IpcEvent) -> Result<bool, &'static str> {
        match event {
            IpcEvent::CreateWindow {
                client_id,
                window_id,
                width,
                height,
                shm,
                shm_mapped_addr,
                shm_size,
            } => {
                println!(
                    "[Compositor] Client {} creating window #{} ({}x{})",
                    client_id, window_id, width, height
                );

                // Check if SHM was provided (modern path)
                if let Some(shm_obj) = shm {
                    println!(
                        "[Compositor] Window #{} uses SHM at 0x{:x?}",
                        window_id, shm_mapped_addr
                    );

                    // Create window with SHM ownership
                    match self.window_manager.create_window_with_shm_from_event(
                        window_id,
                        0,
                        0,
                        width,
                        height,
                        shm_obj,
                        shm_mapped_addr,
                        shm_size,
                    ) {
                        Ok(_) => {
                            println!("[Compositor] Window #{} with SHM created", window_id);
                        }
                        Err(e) => {
                            println!("[Compositor] Failed to create SHM window: {}", e);
                        }
                    }
                } else {
                    // Fallback: legacy Vec-backed window (for test windows)
                    println!("[Compositor] Window #{} uses legacy Vec buffer", window_id);
                    self.window_manager
                        .create_window_with_id(window_id, 0, 0, width, height);
                }

                // Don't trigger redraw yet - wait for client to draw and send UPDATE_BUFFER
                // self.full_redraw_needed = true;

                self.dump_memory_layout("after IPC CreateWindow");
            }
            IpcEvent::DestroyWindow {
                client_id,
                window_id,
            } => {
                println!(
                    "[Compositor] Client {} destroying window #{}",
                    client_id, window_id
                );
                self.window_manager.close_window(window_id);
                self.full_redraw_needed = true;

                self.dump_memory_layout("after IPC DestroyWindow");
            }
            IpcEvent::BufferUpdated {
                window_id,
                damage_x,
                damage_y,
                damage_width,
                damage_height,
            } => {
                if damage_width == 0 || damage_height == 0 {
                    // Ignore empty damage to avoid pointless redraws and potential edge-case bugs.
                    println!(
                        "[Compositor] Window #{} buffer updated with empty damage: ({},{}) {}x{} (ignored)",
                        window_id, damage_x, damage_y, damage_width, damage_height
                    );
                    return Ok(false);
                }

                let (win_x, win_y) = match self.window_manager.get_window(window_id) {
                    Some(w) => (w.x, w.y),
                    None => {
                        println!(
                            "[Compositor] Window #{} buffer updated but window not found (ignored)",
                            window_id
                        );
                        return Ok(false);
                    }
                };

                // Convert window-local damage -> screen-space rect and clamp to screen.
                let rx0 = win_x.saturating_add(damage_x);
                let ry0 = win_y.saturating_add(damage_y);
                let rx1 = rx0.saturating_add(damage_width as i32);
                let ry1 = ry0.saturating_add(damage_height as i32);

                let sx0 = rx0.max(0).min(self.screen_width as i32);
                let sy0 = ry0.max(0).min(self.screen_height as i32);
                let sx1 = rx1.max(0).min(self.screen_width as i32);
                let sy1 = ry1.max(0).min(self.screen_height as i32);
                let w = (sx1 - sx0).max(0) as u32;
                let h = (sy1 - sy0).max(0) as u32;
                if w == 0 || h == 0 {
                    println!(
                        "[Compositor] Window #{} buffer updated but damage out of bounds: ({},{}) {}x{} (ignored)",
                        window_id, damage_x, damage_y, damage_width, damage_height
                    );
                    return Ok(false);
                }

                // println!(
                //     "[Compositor] Window #{} buffer updated: ({},{}) {}x{} -> screen ({},{}) {}x{}",
                //     window_id, damage_x, damage_y, damage_width, damage_height, sx0, sy0, w, h
                // );

                self.add_pending_damage((sx0, sy0, w, h));
            }
            IpcEvent::RequestMove { window_id } => {
                println!("[Compositor] Window #{} requested move", window_id);

                let (start_window_x, start_window_y) =
                    match self.window_manager.get_window(window_id) {
                        Some(w) => (w.x, w.y),
                        None => return Ok(false),
                    };

                // Bring the window to front for the drag (focus is handled by click routing).
                self.window_manager.raise_to_top_with_type(window_id);

                self.move_drag = Some(MoveDragState {
                    window_id,
                    grab_cursor_x: self.cursor.x,
                    grab_cursor_y: self.cursor.y,
                    start_window_x,
                    start_window_y,
                });
            }
            IpcEvent::MoveWindow { window_id, x, y } => {
                println!(
                    "[Compositor] Moving window #{} to ({}, {})",
                    window_id, x, y
                );
                let old_rect = self
                    .window_manager
                    .get_window(window_id)
                    .map(|w| (w.x, w.y, w.width, w.height));
                self.window_manager.set_window_position(window_id, x, y);
                if let Some(r) = old_rect {
                    self.add_pending_damage(r);
                }
                if let Some(w) = self.window_manager.get_window(window_id) {
                    self.add_pending_damage((w.x, w.y, w.width, w.height));
                }
            }
            IpcEvent::SetWindowParent {
                window_id,
                parent_id,
            } => {
                let parent = if parent_id == 0 {
                    None
                } else {
                    Some(parent_id)
                };
                println!(
                    "[Compositor] Setting parent of window #{} to {:?}",
                    window_id, parent
                );

                if self.window_manager.set_window_parent(window_id, parent) {
                    // Keep transient children above their parent by raising the group.
                    self.window_manager.raise_to_top_with_type(window_id);
                    self.full_redraw_needed = true;
                }
            }
            IpcEvent::SetWindowTransientFlags { window_id, flags } => {
                println!(
                    "[Compositor] Setting transient flags of window #{} to 0x{:x}",
                    window_id, flags
                );
                if self
                    .window_manager
                    .set_window_transient_flags(window_id, flags)
                {
                    // If raise policy is enabled, re-raise the group.
                    if (flags & sws_protocol::transient_flags::RAISE_WITH_PARENT) != 0 {
                        self.window_manager.raise_to_top_with_type(window_id);
                    }
                    self.full_redraw_needed = true;
                }
            }
            IpcEvent::SetWindowSizeLimits {
                window_id,
                min_width,
                min_height,
                max_width,
                max_height,
            } => {
                println!(
                    "[Compositor] Setting size limits of window #{} to min={}x{} max={}x{}",
                    window_id, min_width, min_height, max_width, max_height
                );
                if self
                    .window_manager
                    .set_window_size_limits(window_id, min_width, min_height, max_width, max_height)
                {
                    // Size limits affect interactive resize behavior.
                    self.full_redraw_needed = true;
                }
            }
            IpcEvent::ResizeWindow {
                window_id,
                width,
                height,
                shm,
                shm_mapped_addr,
                shm_size,
            } => {
                println!(
                    "[Compositor] Resizing window #{} to {}x{} (shm_mapped=0x{:x?})",
                    window_id, width, height, shm_mapped_addr
                );
                let old_rect = self
                    .window_manager
                    .get_window(window_id)
                    .map(|w| (w.x, w.y, w.width, w.height));
                if let Some(shm) = shm {
                    if self.window_manager.resize_window_with_shm(
                        window_id,
                        width,
                        height,
                        shm,
                        shm_mapped_addr,
                        shm_size,
                    ) {
                        if let Some(r) = old_rect {
                            self.add_pending_damage(r);
                        }
                        if let Some(w) = self.window_manager.get_window(window_id) {
                            let rect = (w.x, w.y, w.width, w.height);
                            self.add_pending_damage(rect);
                        }
                    }
                }
            }
            IpcEvent::MinimizeWindow { window_id } => {
                println!("[Compositor] Minimizing window #{}", window_id);
                let old_rect = self
                    .window_manager
                    .get_window(window_id)
                    .map(|w| (w.x, w.y, w.width, w.height));
                if self.window_manager.minimize_window(window_id) {
                    if let Some(r) = old_rect {
                        self.add_pending_damage(r);
                    }
                    self.full_redraw_needed = true;
                }
            }
            IpcEvent::MaximizeWindow { window_id } => {
                println!("[Compositor] Maximizing window #{}", window_id);
                let old_rect = self
                    .window_manager
                    .get_window(window_id)
                    .map(|w| (w.x, w.y, w.width, w.height));

                // Use workarea for Normal windows only
                let (max_w, max_h, max_x, max_y) = if let Some(window) =
                    self.window_manager.get_window(window_id)
                {
                    if window.window_type == super::window::WindowType::Normal {
                        match self.workarea {
                            Some((wx, wy, ww, wh)) => {
                                // Maximize within workarea
                                let padding = 10i32;
                                let max_w = ww.saturating_sub(padding as u32 * 2).max(1);
                                let max_h = wh.saturating_sub(padding as u32 * 2).max(1);
                                let max_x = wx + padding;
                                let max_y = wy + padding;
                                println!(
                                    "[Compositor] Maximizing Normal window #{} within workarea: ({}, {}) {}x{}",
                                    window_id, max_x, max_y, max_w, max_h
                                );
                                (max_w, max_h, Some(max_x), Some(max_y))
                            }
                            None => (self.screen_width, self.screen_height, None, None),
                        }
                    } else {
                        (self.screen_width, self.screen_height, None, None)
                    }
                } else {
                    (self.screen_width, self.screen_height, None, None)
                };

                if self.window_manager.maximize_window(window_id, max_w, max_h) {
                    // Set position for Normal windows within workarea
                    if let (Some(max_x), Some(max_y)) = (max_x, max_y) {
                        if let Some(window) = self.window_manager.get_window(window_id) {
                            if window.window_type == super::window::WindowType::Normal {
                                self.window_manager
                                    .set_window_position(window_id, max_x, max_y);
                            }
                        }
                    }

                    if let Some(r) = old_rect {
                        self.add_pending_damage(r);
                    }
                    if let Some(w) = self.window_manager.get_window(window_id) {
                        let (x, y, width, height) = (w.x, w.y, w.width, w.height);
                        self.add_pending_damage((x, y, width, height));

                        // Ask the client to resize its buffer to match the new geometry.
                        let payload =
                            sws_protocol::payload_window_configure(window_id, width, height);
                        super::ipc::send_message_to_window(
                            window_id,
                            sws_protocol::server_msg::WINDOW_CONFIGURE,
                            payload.to_vec(),
                        );
                    }
                    self.full_redraw_needed = true;
                }
            }
            IpcEvent::RestoreWindow { window_id } => {
                println!("[Compositor] Restoring window #{}", window_id);
                let old_rect = self
                    .window_manager
                    .get_window(window_id)
                    .map(|w| (w.x, w.y, w.width, w.height));
                if self.window_manager.restore_window(window_id) {
                    if let Some(r) = old_rect {
                        self.add_pending_damage(r);
                    }
                    if let Some(w) = self.window_manager.get_window(window_id) {
                        let (x, y, width, height) = (w.x, w.y, w.width, w.height);
                        self.add_pending_damage((x, y, width, height));

                        // If geometry changed (e.g. restored from maximized), ask the client
                        // to resize its buffer.
                        if let Some((_ox, _oy, ow, oh)) = old_rect {
                            if ow != width || oh != height {
                                let payload = sws_protocol::payload_window_configure(
                                    window_id, width, height,
                                );
                                super::ipc::send_message_to_window(
                                    window_id,
                                    sws_protocol::server_msg::WINDOW_CONFIGURE,
                                    payload.to_vec(),
                                );
                            }
                        }
                    }
                    self.full_redraw_needed = true;
                }
            }
            IpcEvent::FocusWindow { window_id } => {
                println!("[Compositor] Focusing window #{}", window_id);
                // Restore if minimized
                if self.window_manager.is_minimized(window_id) {
                    let old_rect = self
                        .window_manager
                        .get_window(window_id)
                        .map(|w| (w.x, w.y, w.width, w.height));
                    if self.window_manager.restore_window(window_id) {
                        if let Some(r) = old_rect {
                            self.add_pending_damage(r);
                        }
                    }
                }
                // Focus and raise the window
                self.window_manager.focus_window(window_id);
                self.window_manager.raise_to_top(window_id);
                self.full_redraw_needed = true;
            }
            IpcEvent::SetWindowType {
                window_id,
                window_type,
            } => {
                println!(
                    "[Compositor] Setting window #{} type to {}",
                    window_id, window_type
                );
                use sws_protocol::window_types;
                let wtype = match window_type {
                    window_types::NORMAL => super::window::WindowType::Normal,
                    window_types::ALWAYS_ON_TOP => super::window::WindowType::AlwaysOnTop,
                    window_types::TASKBAR => super::window::WindowType::Taskbar,
                    window_types::DESKTOP => super::window::WindowType::Desktop,
                    _ => {
                        println!("[Compositor] Invalid window type {}, ignoring", window_type);
                        return Ok(false);
                    }
                };
                if self.window_manager.set_window_type(window_id, wtype) {
                    // For Normal windows, adjust position to workarea if available
                    if wtype == super::window::WindowType::Normal {
                        if let Some(window) = self.window_manager.get_window(window_id) {
                            let (default_x, default_y) = self
                                .window_manager
                                .calculate_default_position(window.width, window.height);
                            // Only adjust if window is at default position (0,0)
                            if window.x == 0 && window.y == 0 {
                                println!(
                                    "[Compositor] Adjusting Normal window #{} position to workarea: ({}, {})",
                                    window_id, default_x, default_y
                                );
                                self.window_manager
                                    .set_window_position(window_id, default_x, default_y);
                            }
                        }
                    }
                    // Re-raise to update Z-order based on window type
                    self.window_manager.raise_to_top_with_type(window_id);
                    self.full_redraw_needed = true;
                }
            }
            IpcEvent::SetWindowOpacity { window_id, opacity } => {
                println!(
                    "[Compositor] Setting window #{} opacity to {}",
                    window_id, opacity
                );
                let opacity_f = (opacity as f32) / 255.0;
                if self.window_manager.set_window_opacity(window_id, opacity_f) {
                    if let Some(w) = self.window_manager.get_window(window_id) {
                        self.add_pending_damage((w.x, w.y, w.width, w.height));
                    }
                }
            }
            IpcEvent::SetWorkarea {
                x,
                y,
                width,
                height,
            } => {
                println!(
                    "[Compositor] Workarea set: x={}, y={}, width={}, height={}",
                    x, y, width, height
                );
                self.workarea = Some((x, y, width, height));
                // Notify window manager about workarea change
                self.window_manager.set_workarea(x, y, width, height);
                self.full_redraw_needed = true;
            }
            IpcEvent::SetWindowResizable {
                window_id,
                resizable,
            } => {
                println!(
                    "[Compositor] Setting window #{} resizable to {}",
                    window_id, resizable
                );
                if self
                    .window_manager
                    .set_window_resizable(window_id, resizable)
                {
                    // No redraw needed, just state change
                }
            }
            IpcEvent::GetScreenSize { client_id } => {
                println!(
                    "[Compositor] GetScreenSize request from client {}",
                    client_id
                );
                let width = self.screen_width;
                let height = self.screen_height;
                // Send SCREEN_SIZE response to the client
                // Use the first window (if any) to send the response
                if let Some(first_window_id) = self.window_manager.get_first_window_id() {
                    let payload = sws_protocol::payload_screen_size(width, height);
                    let _ = send_message_to_window(
                        first_window_id,
                        sws_protocol::server_msg::SCREEN_SIZE,
                        payload.to_vec(),
                    );
                    println!(
                        "[Compositor] Sent SCREEN_SIZE: {}x{} to client {} (via window {})",
                        width, height, client_id, first_window_id
                    );
                }
            }
            IpcEvent::GetWindowList { client_id } => {
                println!(
                    "[Compositor] GetWindowList request from client {}",
                    client_id
                );
                // Get window list from window manager
                let windows = self.window_manager.get_window_list();

                // Convert to WindowListEntry and use protocol library serialization
                let entries: std::vec::Vec<sws_protocol::WindowListEntry> = windows
                    .into_iter()
                    .map(
                        |(window_id, title, window_type, visible, focused, minimized)| {
                            sws_protocol::WindowListEntry {
                                window_id,
                                title,
                                window_type,
                                visible,
                                focused,
                                minimized,
                            }
                        },
                    )
                    .collect();

                let payload = sws_protocol::payload_window_list(&entries);

                // Send WINDOW_LIST response directly to the client (not via window)
                // This works for clients with or without windows (like stemd)
                send_message_to_client(
                    client_id,
                    sws_protocol::server_msg::WINDOW_LIST,
                    payload,
                );
                println!(
                    "[Compositor] Sent WINDOW_LIST: {} windows to client {}",
                    entries.len(),
                    client_id
                );
            }
        }
        Ok(false)
    }
}
