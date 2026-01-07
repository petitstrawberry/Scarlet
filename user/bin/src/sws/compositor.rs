//! Compositor module - manages window composition and rendering

use super::cursor::Cursor;
use super::input::{CompositorInputEvent, InputManager, key_codes};
use super::ipc::{IpcEvent, IpcServer};
use super::protocol::ServerMessage;
use super::window::{WindowId, WindowManager};
use framebuffer::Framebuffer;
use std::println;
use std::vec::Vec;

/// Compositor - the main window server with proper layer compositing
pub struct Compositor {
    framebuffer: Framebuffer,
    vram_ptr: Option<usize>,
    vram_size: usize,
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

        let bg_color = [100, 100, 100, 255]; // Gray background

        Ok(Self {
            framebuffer,
            vram_ptr,
            vram_size,
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

        // Create multiple test windows with buffers
        let win1 = self.window_manager.create_window(50, 50, 400, 250);
        if let Some(window) = self.window_manager.get_window_mut(win1) {
            window.set_title("Window 1");
            // Fill with red gradient
            if let Some(ref mut buffer) = window.buffer {
                Self::fill_buffer_gradient(buffer, 400, 250, [200, 50, 50, 255]);
            }
        }

        let win2 = self.window_manager.create_window(150, 120, 350, 200);
        if let Some(window) = self.window_manager.get_window_mut(win2) {
            window.set_title("Window 2");
            // Fill with green gradient
            if let Some(ref mut buffer) = window.buffer {
                Self::fill_buffer_gradient(buffer, 350, 200, [50, 200, 50, 255]);
            }
        }

        let win3 = self.window_manager.create_window(250, 180, 300, 180);
        if let Some(window) = self.window_manager.get_window_mut(win3) {
            window.set_title("Window 3");
            // Fill with blue gradient
            if let Some(ref mut buffer) = window.buffer {
                Self::fill_buffer_gradient(buffer, 300, 180, [50, 50, 200, 255]);
            }
        }

        // Focus the last created window
        self.window_manager.set_focus(win3);

        println!("[Compositor] Created 3 test windows with content");

        // Initial full composite
        self.full_redraw_needed = true;
        self.composite_and_present()?;

        println!("[Compositor] Display initialized");

        Ok(())
    }

    /// Fill buffer with gradient (for testing, static method)
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
        // If window has a buffer, use it; otherwise draw placeholder
        if let Some(ref window_buffer) = window.buffer {
            self.draw_window_from_buffer(window, window_buffer, buffer, stride, clip_rect);
        } else {
            self.draw_window_placeholder(window, buffer, stride, clip_rect);
        }
    }

    /// Draw window from its shared memory buffer
    fn draw_window_from_buffer(
        &self,
        window: &super::window::Window,
        window_buffer: &[u8],
        screen_buffer: &mut [u8],
        stride: u32,
        clip_rect: Option<(i32, i32, u32, u32)>,
    ) {
        let border_color = if window.focused {
            [50, 50, 150, 255] // Blue border for focused
        } else {
            [100, 100, 100, 255] // Gray border for unfocused
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

                let screen_offset = ((screen_y as u32 * stride)
                    + (screen_x as u32 * self.bytes_per_pixel))
                    as usize;

                // Draw border or content
                let is_border = x == 0 || y == 0 || x == window.width - 1 || y == window.height - 1;

                if is_border {
                    // Draw border
                    if screen_offset + 4 <= screen_buffer.len() {
                        screen_buffer[screen_offset] = border_color[0];
                        screen_buffer[screen_offset + 1] = border_color[1];
                        screen_buffer[screen_offset + 2] = border_color[2];
                        screen_buffer[screen_offset + 3] = border_color[3];
                    }
                } else {
                    // Draw from window buffer (BGRA format)
                    let window_offset = ((y * window.width + x) * 4) as usize;
                    if window_offset + 4 <= window_buffer.len()
                        && screen_offset + 4 <= screen_buffer.len()
                    {
                        screen_buffer[screen_offset] = window_buffer[window_offset];
                        screen_buffer[screen_offset + 1] = window_buffer[window_offset + 1];
                        screen_buffer[screen_offset + 2] = window_buffer[window_offset + 2];
                        screen_buffer[screen_offset + 3] = window_buffer[window_offset + 3];
                    }
                }
            }
        }
    }

    /// Draw placeholder window (for windows without buffers yet)
    fn draw_window_placeholder(
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
    /// Handle mouse click (for window focus)
    fn handle_click(&mut self) -> Result<(), &'static str> {
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
        println!("[Compositor] Starting main loop (multithreaded)");

        loop {
            let mut needs_redraw = false;

            // Process IPC events from global queue (non-blocking)
            let ipc_events = self.ipc_server.process_messages()?;
            if !ipc_events.is_empty() {
                println!("[Compositor] Processing {} IPC events", ipc_events.len());
                needs_redraw = true;
            }
            for event in ipc_events {
                self.handle_ipc_event(event)?;
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
            if needs_redraw || self.full_redraw_needed {
                if self.full_redraw_needed {
                    println!("[Compositor] Full redraw triggered");
                }
                self.composite_and_present()?;
                self.event_counter += 1;
            }

            // Sleep briefly to limit frame rate and reduce CPU usage
            // 16ms = ~60fps, adjust as needed
            std::thread::sleep(core::time::Duration::from_millis(16));

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
                Ok(true)
            }
            CompositorInputEvent::MouseAbsolute { x, y } => {
                self.cursor
                    .set_position(x, y, self.screen_width, self.screen_height);
                Ok(true)
            }
            CompositorInputEvent::MouseButton { button, pressed } => {
                if button == key_codes::BTN_LEFT && pressed {
                    self.handle_click()?;
                }
                Ok(true)
            }
        }
    }

    /// Handle IPC events from clients
    fn handle_ipc_event(&mut self, event: IpcEvent) -> Result<(), &'static str> {
        match event {
            IpcEvent::CreateWindow {
                client_id,
                width,
                height,
            } => {
                println!(
                    "[Compositor] Client {} creating window {}x{}",
                    client_id, width, height
                );
                let window_id = self.window_manager.create_window(0, 0, width, height);

                // Get buffer size from window
                let buffer_size = if let Some(window) = self.window_manager.get_window(window_id) {
                    window.buffer_size()
                } else {
                    0
                };

                // Send WindowCreated message to client
                let message = ServerMessage::WindowCreated {
                    window_id,
                    shm_size: buffer_size,
                };
                if let Err(e) = self.ipc_server.send_to_client(client_id, message) {
                    println!("[Compositor] Failed to send WindowCreated: {}", e);
                }

                self.full_redraw_needed = true;
            }
            IpcEvent::WindowCreated { window_id, width, height } => {
                println!(
                    "[Compositor] Window #{} created via IPC ({}x{}), triggering redraw",
                    window_id, width, height
                );
                self.full_redraw_needed = true;
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
            }
            IpcEvent::WindowDestroyed { window_id } => {
                println!(
                    "[Compositor] Window #{} destroyed via IPC, triggering redraw",
                    window_id
                );
                self.window_manager.close_window(window_id);
                self.full_redraw_needed = true;
            }
            IpcEvent::BufferUpdated {
                window_id,
                damage_x,
                damage_y,
                damage_width,
                damage_height,
            } => {
                println!(
                    "[Compositor] Window #{} buffer updated: ({},{}) {}x{}",
                    window_id, damage_x, damage_y, damage_width, damage_height
                );
                // TODO: Optimize by only compositing the damaged region
                self.full_redraw_needed = true;
            }
            IpcEvent::ClientBufferUpdate { window_id, buffer } => {
                println!(
                    "[Compositor] Window #{} received new buffer ({} bytes)",
                    window_id,
                    buffer.len()
                );
                // Update window buffer
                if let Some(window) = self.window_manager.get_window_mut(window_id) {
                    window.buffer = Some(buffer);
                    println!("[Compositor] Buffer updated for window #{}", window_id);
                }
                self.full_redraw_needed = true;
            }
            IpcEvent::RequestMove { window_id } => {
                println!("[Compositor] Window #{} requested move", window_id);
                // TODO: Enter move mode for this window
            }
            IpcEvent::MoveWindow { window_id, x, y } => {
                println!(
                    "[Compositor] Moving window #{} to ({}, {})",
                    window_id, x, y
                );
                self.window_manager.set_window_position(window_id, x, y);
                self.full_redraw_needed = true;
            }
        }
        Ok(())
    }
}
