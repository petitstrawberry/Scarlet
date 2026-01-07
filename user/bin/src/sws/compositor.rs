//! Compositor module - manages window composition and rendering

use super::cursor::Cursor;
use super::input::{InputEvent, InputManager, abs_codes, event_types, rel_codes};
use super::ipc::{IpcEvent, IpcServer};
use super::window::{WindowId, WindowManager};
use framebuffer::Framebuffer;
use std::println;
use std::vec::Vec;

/// Compositor - the main window server with proper layer compositing
pub struct Compositor {
    framebuffer: Framebuffer,
    vram_ptr: Option<usize>,
    vram_size: usize,
    input_manager: InputManager,
    window_manager: WindowManager,
    ipc_server: IpcServer,
    cursor: Cursor,
    screen_width: u32,
    screen_height: u32,
    bg_color: [u8; 4],
    bytes_per_pixel: u32,
    full_redraw_needed: bool,
    event_counter: u64,
}

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

        let screen_width = var_info.xres;
        let screen_height = var_info.yres;
        let bytes_per_pixel = 4; // BGRA

        println!("[Compositor] Screen: {}x{}", screen_width, screen_height);

        // Try to get mmap info for direct VRAM access
        let (vram_ptr, vram_size) = if let Some((addr, size)) = framebuffer.get_mapping_info() {
            println!("[Compositor] Using mmap direct VRAM access at 0x{:x}", addr);
            (Some(addr), size)
        } else {
            println!("[Compositor] Warning: mmap not available, using fallback I/O");
            (None, 0)
        };

        // Initialize input manager
        let input_manager = InputManager::new()?;

        // Initialize IPC server
        let mut ipc_server = IpcServer::new("/tmp/sws.sock")?;
        ipc_server.listen()?;

        // Initialize window manager
        let window_manager = WindowManager::new();

        // Initialize cursor at center
        let mut cursor = Cursor::new();
        cursor.x = (screen_width / 2) as i32;
        cursor.y = (screen_height / 2) as i32;

        let bg_color = [100, 100, 100, 255]; // Gray background

        Ok(Self {
            framebuffer,
            vram_ptr,
            vram_size,
            input_manager,
            window_manager,
            ipc_server,
            cursor,
            screen_width,
            screen_height,
            bg_color,
            bytes_per_pixel,
            full_redraw_needed: true,
            event_counter: 0,
        })
    }

    /// Initialize display (clear screen and draw cursor)
    pub fn init_display(&mut self) -> Result<(), &'static str> {
        println!("[Compositor] Initializing display...");

        // Create multiple test windows for demonstration
        let win1 = self.window_manager.create_window(50, 50, 400, 250);
        if let Some(window) = self.window_manager.get_window_mut(win1) {
            window.set_title("Window 1");
        }

        let win2 = self.window_manager.create_window(150, 120, 350, 200);
        if let Some(window) = self.window_manager.get_window_mut(win2) {
            window.set_title("Window 2");
        }

        let win3 = self.window_manager.create_window(250, 180, 300, 180);
        if let Some(window) = self.window_manager.get_window_mut(win3) {
            window.set_title("Window 3");
        }

        // Focus the last created window
        self.window_manager.set_focus(win3);

        println!("[Compositor] Created 3 test windows");

        // Initial full composite
        self.full_redraw_needed = true;
        self.composite_and_present()?;

        println!("[Compositor] Display initialized");

        Ok(())
    }

    /// Composite all layers directly to VRAM (or framebuffer as fallback)
    fn composite_and_present(&mut self) -> Result<(), &'static str> {
        if let Some(vram_addr) = self.vram_ptr {
            // Fast path: Direct VRAM access
            self.composite_to_vram(vram_addr)?;
        } else {
            // Fallback: Use framebuffer API
            self.composite_via_framebuffer()?;
        }

        // Flush to display
        self.framebuffer
            .flush()
            .map_err(|_| "Failed to flush framebuffer")?;

        self.full_redraw_needed = false;
        Ok(())
    }

    /// Composite directly to VRAM (fastest method)
    fn composite_to_vram(&mut self, vram_addr: usize) -> Result<(), &'static str> {
        let stride = self.screen_width * self.bytes_per_pixel;

        if self.full_redraw_needed {
            // Full screen redraw
            unsafe {
                let vram = core::slice::from_raw_parts_mut(vram_addr as *mut u8, self.vram_size);

                // Layer 1: Fill with background color
                for y in 0..self.screen_height {
                    for x in 0..self.screen_width {
                        let offset = ((y * stride) + (x * self.bytes_per_pixel)) as usize;
                        vram[offset] = self.bg_color[0]; // B
                        vram[offset + 1] = self.bg_color[1]; // G
                        vram[offset + 2] = self.bg_color[2]; // R
                        vram[offset + 3] = self.bg_color[3]; // A
                    }
                }

                // Layer 2: Draw all windows (no clipping for full redraw)
                for window in self.window_manager.get_windows() {
                    if !window.visible {
                        continue;
                    }
                    self.draw_window_to_buffer_clipped(window, vram, stride, None);
                }

                // Layer 3: Draw cursor
                self.draw_cursor_to_buffer(vram, stride);
            }
        } else {
            // Incremental update: only cursor dirty region
            let (dx, dy, dw, dh) = self.cursor.get_dirty_region();
            let clip_rect = Some((dx, dy, dw, dh));

            unsafe {
                let vram = core::slice::from_raw_parts_mut(vram_addr as *mut u8, self.vram_size);

                // Redraw dirty region background
                for y in dy.max(0)..(dy + dh as i32).min(self.screen_height as i32) {
                    for x in dx.max(0)..(dx + dw as i32).min(self.screen_width as i32) {
                        let offset =
                            ((y as u32 * stride) + (x as u32 * self.bytes_per_pixel)) as usize;
                        vram[offset] = self.bg_color[0];
                        vram[offset + 1] = self.bg_color[1];
                        vram[offset + 2] = self.bg_color[2];
                        vram[offset + 3] = self.bg_color[3];
                    }
                }

                // Redraw ALL windows, but clipped to dirty region
                for window in self.window_manager.get_windows() {
                    if !window.visible {
                        continue;
                    }
                    // Check if window intersects dirty region
                    if window.x + window.width as i32 >= dx
                        && window.x < dx + dw as i32
                        && window.y + window.height as i32 >= dy
                        && window.y < dy + dh as i32
                    {
                        self.draw_window_to_buffer_clipped(window, vram, stride, clip_rect);
                    }
                }

                // Draw cursor
                self.draw_cursor_to_buffer(vram, stride);
            }
        }

        self.cursor.mark_drawn();
        Ok(())
    }

    /// Draw a window to buffer with optional clipping
    /// clip_rect: (x, y, width, height) in screen coordinates
    fn draw_window_to_buffer_clipped(
        &self,
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
        let border_color = if window.focused {
            [50, 50, 150, 255]
        } else {
            [100, 100, 100, 255]
        };

        for y in 0..window.height {
            for x in 0..window.width {
                let screen_x = window.x + x as i32;
                let screen_y = window.y + y as i32;

                // Screen bounds check
                if screen_x < 0
                    || screen_x >= self.screen_width as i32
                    || screen_y < 0
                    || screen_y >= self.screen_height as i32
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

                let offset = ((screen_y as u32 * stride) + (screen_x as u32 * self.bytes_per_pixel))
                    as usize;
                let is_border = x == 0 || y == 0 || x == window.width - 1 || y == window.height - 1;
                let color = if is_border {
                    border_color
                } else {
                    window_color
                };

                if offset + 4 <= buffer.len() {
                    buffer[offset] = color[0];
                    buffer[offset + 1] = color[1];
                    buffer[offset + 2] = color[2];
                    buffer[offset + 3] = color[3];
                }
            }
        }
    }

    /// Draw cursor to buffer
    fn draw_cursor_to_buffer(&self, buffer: &mut [u8], stride: u32) {
        self.cursor.draw_to_buffer_direct(
            buffer,
            self.screen_width,
            self.screen_height,
            self.bytes_per_pixel,
            stride,
        );
    }

    /// Composite via framebuffer API (fallback when mmap unavailable)
    fn composite_via_framebuffer(&mut self) -> Result<(), &'static str> {
        // Allocate temporary backbuffer
        let buffer_size = (self.screen_width * self.screen_height * self.bytes_per_pixel) as usize;
        let mut backbuffer = Vec::with_capacity(buffer_size);
        backbuffer.resize(buffer_size, 0);

        let stride = self.screen_width * self.bytes_per_pixel;

        // Layer 1: Fill with background
        for y in 0..self.screen_height {
            for x in 0..self.screen_width {
                let offset = ((y * stride) + (x * self.bytes_per_pixel)) as usize;
                backbuffer[offset] = self.bg_color[0];
                backbuffer[offset + 1] = self.bg_color[1];
                backbuffer[offset + 2] = self.bg_color[2];
                backbuffer[offset + 3] = self.bg_color[3];
            }
        }

        // Layer 2: Draw windows
        for window in self.window_manager.get_windows() {
            if !window.visible {
                continue;
            }
            self.draw_window_to_buffer_clipped(window, &mut backbuffer, stride, None);
        }

        // Layer 3: Draw cursor
        self.draw_cursor_to_buffer(&mut backbuffer, stride);

        // Present
        self.framebuffer
            .write_block(0, 0, self.screen_width, self.screen_height, &backbuffer)
            .map_err(|_| "Failed to write backbuffer")?;

        self.cursor.mark_drawn();
        Ok(())
    }

    /// Process input events
    fn process_input(&mut self) -> Result<bool, &'static str> {
        let event = match self.input_manager.read_event()? {
            Some(event) => event,
            None => return Ok(false), // No event available
        };

        // Process event
        match event.type_ {
            event_types::EV_REL => match event.code {
                rel_codes::REL_X => {
                    self.cursor.update_position(
                        event.value,
                        0,
                        self.screen_width,
                        self.screen_height,
                    );
                }
                rel_codes::REL_Y => {
                    self.cursor.update_position(
                        0,
                        event.value,
                        self.screen_width,
                        self.screen_height,
                    );
                }
                _ => {}
            },
            event_types::EV_ABS => match event.code {
                abs_codes::ABS_X => {
                    let screen_x = self
                        .input_manager
                        .scale_tablet_coord(event.value, self.screen_width);
                    self.cursor.set_position(
                        screen_x,
                        self.cursor.y,
                        self.screen_width,
                        self.screen_height,
                    );
                }
                abs_codes::ABS_Y => {
                    let screen_y = self
                        .input_manager
                        .scale_tablet_coord(event.value, self.screen_height);
                    self.cursor.set_position(
                        self.cursor.x,
                        screen_y,
                        self.screen_width,
                        self.screen_height,
                    );
                }
                _ => {
                    println!(
                        "[Compositor] Unknown EV_ABS code: 0x{:x}, value: {}",
                        event.code, event.value
                    );
                }
            },
            event_types::EV_KEY => {
                use super::input::key_codes;
                println!(
                    "[Compositor] EV_KEY: code=0x{:x}, value={}",
                    event.code, event.value
                );
                match event.code {
                    key_codes::BTN_LEFT => {
                        if event.value == 1 {
                            // Button pressed
                            println!("[Compositor] BTN_LEFT pressed!");
                            self.handle_mouse_click()?;
                            return Ok(true); // Need full redraw for focus change
                        }
                    }
                    _ => {}
                }
            }
            event_types::EV_SYN => {
                if self.cursor.needs_redraw() {
                    return Ok(true); // Need to redraw
                }
            }
            _ => {}
        }

        Ok(false)
    }

    /// Handle mouse click (for window focus)
    fn handle_mouse_click(&mut self) -> Result<(), &'static str> {
        let click_x = self.cursor.x;
        let click_y = self.cursor.y;

        // Find topmost window at click position
        if let Some(win_id) = self.window_manager.window_at_point(click_x, click_y) {
            println!("[Compositor] Clicked on window #{}", win_id);

            // Change focus and bring to front
            self.window_manager.set_focus(win_id);
            self.window_manager.raise_to_top(win_id);

            // Need full redraw when Z-order changes
            self.full_redraw_needed = true;
        }

        Ok(())
    }

    /// Main event loop
    pub fn run(&mut self) -> Result<(), &'static str> {
        println!("[Compositor] Starting main loop");

        loop {
            // Process IPC messages from clients
            let ipc_events = self.ipc_server.process_messages()?;
            for event in ipc_events {
                self.handle_ipc_event(event)?;
            }

            // Process one input event
            let needs_redraw = self.process_input()?;

            // Re-composite and present if needed
            if needs_redraw {
                self.composite_and_present()?;
            }

            // Periodically print Z-order (every 100 events)
            self.event_counter += 1;
            if self.event_counter % 100 == 0 {
                use std::print;
                print!("[Compositor] Z-order check #{}: ", self.event_counter);
                for window in self.window_manager.get_windows() {
                    print!("#{}{} ", window.id, if window.focused { "(F)" } else { "" });
                }
                println!();
            }
        }
    }

    /// Handle IPC events from clients
    fn handle_ipc_event(&mut self, event: IpcEvent) -> Result<(), &'static str> {
        match event {
            IpcEvent::CreateWindow { client_id, width, height } => {
                println!("[Compositor] Client {} creating window {}x{}", client_id, width, height);
                let window_id = self.window_manager.create_window(0, 0, width, height);
                // TODO: Send WindowCreated message to client with shared memory info
                self.full_redraw_needed = true;
            }
            IpcEvent::DestroyWindow { client_id, window_id } => {
                println!("[Compositor] Client {} destroying window #{}", client_id, window_id);
                self.window_manager.close_window(window_id);
                self.full_redraw_needed = true;
            }
            IpcEvent::BufferUpdated { window_id, damage_x, damage_y, damage_width, damage_height } => {
                println!("[Compositor] Window #{} buffer updated: ({},{}) {}x{}",
                    window_id, damage_x, damage_y, damage_width, damage_height);
                // TODO: Composite only the damaged region
                self.full_redraw_needed = true;
            }
            IpcEvent::RequestMove { window_id } => {
                println!("[Compositor] Window #{} requested move", window_id);
                // TODO: Enter move mode for this window
            }
            IpcEvent::MoveWindow { window_id, x, y } => {
                println!("[Compositor] Moving window #{} to ({}, {})", window_id, x, y);
                self.window_manager.set_window_position(window_id, x, y);
                self.full_redraw_needed = true;
            }
        }
        Ok(())
    }
}
