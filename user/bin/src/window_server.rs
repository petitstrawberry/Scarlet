//! Simple Window Server
//!
//! Phase 1 implementation: VRAM direct drawing and mouse cursor display/movement

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use framebuffer::Framebuffer;
use std::fs::File;
use std::println;

/// Input event structure (16 bytes, matches kernel InputEvent)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct InputEvent {
    time: u64,  // 8 bytes - timestamp in nanoseconds
    type_: u16, // 2 bytes - event type
    code: u16,  // 2 bytes - event code
    value: i32, // 4 bytes - event value
}

impl InputEvent {
    const SIZE: usize = core::mem::size_of::<Self>();
}

/// Event types
mod event_types {
    pub const EV_SYN: u16 = 0x00;
    pub const EV_KEY: u16 = 0x01;
    pub const EV_REL: u16 = 0x02;
    pub const EV_ABS: u16 = 0x03;
}

/// Relative axis codes
mod rel_codes {
    pub const REL_X: u16 = 0x00;
    pub const REL_Y: u16 = 0x01;
    pub const REL_WHEEL: u16 = 0x08;
}

/// Absolute axis codes
mod abs_codes {
    pub const ABS_X: u16 = 0x00;
    pub const ABS_Y: u16 = 0x01;
}

/// Simple cursor state
struct Cursor {
    x: i32,
    y: i32,
    prev_x: i32,
    prev_y: i32,
    width: u32,
    height: u32,
    needs_redraw: bool,
}

impl Cursor {
    fn new() -> Self {
        Self {
            x: 0,
            y: 0,
            prev_x: 0,
            prev_y: 0,
            width: 12,
            height: 18,
            needs_redraw: true,
        }
    }

    /// Update cursor position with bounds checking (relative movement)
    fn update_position(&mut self, dx: i32, dy: i32, screen_width: u32, screen_height: u32) -> bool {
        let old_x = self.x;
        let old_y = self.y;
        self.x = (self.x + dx).max(0).min(screen_width as i32 - 1);
        self.y = (self.y + dy).max(0).min(screen_height as i32 - 1);
        let moved = old_x != self.x || old_y != self.y;
        if moved {
            self.needs_redraw = true;
            println!("[Cursor] Moved to ({}, {})", self.x, self.y);
        }
        moved
    }

    /// Set cursor position directly (absolute positioning for tablet)
    fn set_position(&mut self, x: i32, y: i32, screen_width: u32, screen_height: u32) -> bool {
        let old_x = self.x;
        let old_y = self.y;
        self.x = x.max(0).min(screen_width as i32 - 1);
        self.y = y.max(0).min(screen_height as i32 - 1);
        let moved = old_x != self.x || old_y != self.y;
        if moved {
            self.needs_redraw = true;
            println!("[Cursor] Set to ({}, {})", self.x, self.y);
        }
        moved
    }

    /// Draw an arrow cursor
    fn draw(&self, fb: &mut Framebuffer) {
        let white = [255, 255, 255, 255];
        let black = [0, 0, 0, 255];
        let cx = self.x as u32;
        let cy = self.y as u32;
        
        // Arrow pattern: width for each row
        let pattern = [
            1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12,  // Expanding
            9, 6, 6, 3, 3, 1,  // Contracting tail
        ];
        
        for (y_offset, &width) in pattern.iter().enumerate() {
            for dx in 0..width {
                let px = cx.saturating_add(dx);
                let py = cy.saturating_add(y_offset as u32);
                
                // Black border
                if dx == 0 || dx == width - 1 {
                    let _ = fb.write_pixel(px, py, black);
                } else {
                    let _ = fb.write_pixel(px, py, white);
                }
            }
        }
    }

    /// Clear previous cursor by redrawing background
    fn clear_prev(&self, fb: &mut Framebuffer, bg_color: [u8; 4]) {
        let cx = self.prev_x as u32;
        let cy = self.prev_y as u32;
        
        // Same pattern as draw
        let pattern = [
            1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12,
            9, 6, 6, 3, 3, 1,
        ];
        
        for (y_offset, &width) in pattern.iter().enumerate() {
            for dx in 0..width {
                let px = cx.saturating_add(dx);
                let py = cy.saturating_add(y_offset as u32);
                let _ = fb.write_pixel(px, py, bg_color);
            }
        }
    }
}

/// Window Server main structure
struct WindowServer {
    framebuffer: Framebuffer,
    mouse_file: File,
    cursor: Cursor,
    screen_width: u32,
    screen_height: u32,
    bg_color: [u8; 4],
}

impl WindowServer {
    /// Initialize the window server
    fn new() -> Result<Self, &'static str> {
        println!("[WindowServer] Starting initialization...");

        // Open framebuffer
        let framebuffer =
            Framebuffer::open("/dev/fb0").map_err(|_| "Failed to open framebuffer")?;

        // Get screen dimensions
        let var_info = framebuffer
            .get_var_screen_info()
            .map_err(|_| "Failed to get screen info")?;

        let screen_width = var_info.xres;
        let screen_height = var_info.yres;

        println!("[WindowServer] Screen: {}x{}", screen_width, screen_height);

        // Try to open tablet device first (absolute positioning), fallback to mouse (relative)
        let mouse_file = match File::open("/dev/tablet0") {
            Ok(file) => {
                println!("[WindowServer] Opened tablet device (absolute positioning)");
                file
            }
            Err(_) => {
                println!("[WindowServer] Tablet device not found, trying mouse device...");
                File::open("/dev/mouse0").map_err(|_| "Failed to open mouse or tablet device")?
            }
        };

        println!("[WindowServer] Input device ready");

        // Initialize cursor at center
        let mut cursor = Cursor::new();
        cursor.x = (screen_width / 2) as i32;
        cursor.y = (screen_height / 2) as i32;

        let bg_color = [100, 100, 100, 255]; // Gray background

        Ok(Self {
            framebuffer,
            mouse_file,
            cursor,
            screen_width,
            screen_height,
            bg_color,
        })
    }

    /// Initialize display (clear screen and draw cursor)
    fn init_display(&mut self) -> Result<(), &'static str> {
        println!("[WindowServer] Initializing display...");

        // Fill screen with background color
        self.framebuffer
            .fill_screen(self.bg_color)
            .map_err(|_| "Failed to fill screen")?;

        // Draw initial cursor
        self.cursor.draw(&mut self.framebuffer);

        // Flush to display
        self.framebuffer
            .flush()
            .map_err(|_| "Failed to flush framebuffer")?;

        println!("[WindowServer] Display initialized");

        Ok(())
    }

    /// Process mouse input events
    fn process_input(&mut self) -> Result<bool, &'static str> {
        let mut buffer = [0u8; InputEvent::SIZE];

        // Read one event
        let bytes_read = self.mouse_file.read(&mut buffer).map_err(|e| {
            println!("[WindowServer] Read error: {:?}", e);
            "Failed to read mouse event"
        })?;

        println!("[WindowServer] Read {} bytes", bytes_read);

        if bytes_read != InputEvent::SIZE {
            println!(
                "[WindowServer] Incomplete event: expected {}, got {}",
                InputEvent::SIZE,
                bytes_read
            );
            return Ok(false); // No complete event available
        }

        // Parse event
        let event = unsafe { core::ptr::read(buffer.as_ptr() as *const InputEvent) };

        println!(
            "[WindowServer] Event: type={:#x}, code={:#x}, value={}",
            event.type_, event.code, event.value
        );

        // Process event
        match event.type_ {
            event_types::EV_REL => match event.code {
                rel_codes::REL_X => {
                    println!("[WindowServer] REL_X: {}", event.value);
                    self.cursor.update_position(
                        event.value,
                        0,
                        self.screen_width,
                        self.screen_height,
                    );
                }
                rel_codes::REL_Y => {
                    println!("[WindowServer] REL_Y: {}", event.value);
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
                    println!("[WindowServer] ABS_X: {}", event.value);
                    // Tablet X coordinate - set directly
                    self.cursor.set_position(
                        event.value,
                        self.cursor.y,
                        self.screen_width,
                        self.screen_height,
                    );
                }
                abs_codes::ABS_Y => {
                    println!("[WindowServer] ABS_Y: {}", event.value);
                    // Tablet Y coordinate - set directly
                    self.cursor.set_position(
                        self.cursor.x,
                        event.value,
                        self.screen_width,
                        self.screen_height,
                    );
                }
                _ => {}
            },
            event_types::EV_SYN => {
                println!("[WindowServer] SYN event - redrawing if needed");
                if self.cursor.needs_redraw {
                    // Clear previous cursor position
                    self.cursor.clear_prev(&mut self.framebuffer, self.bg_color);
                    // Draw new cursor at current position
                    self.cursor.draw(&mut self.framebuffer);
                    // Update prev position to current (after drawing)
                    self.cursor.prev_x = self.cursor.x;
                    self.cursor.prev_y = self.cursor.y;
                    self.cursor.needs_redraw = false;
                    return Ok(true);
                }
            }
            _ => {}
        }
        Ok(false)
    }

    /// Main event loop
    fn run(&mut self) -> Result<(), &'static str> {
        println!("[WindowServer] Entering main loop...");
        println!("[WindowServer] Move your mouse to see the cursor!");

        loop {
            // Process input events
            match self.process_input() {
                Ok(true) => {
                    // Screen was updated, flush
                    self.framebuffer
                        .flush()
                        .map_err(|_| "Failed to flush framebuffer")?;
                }
                Ok(false) => {
                    // No update needed
                }
                Err(e) => {
                    println!("[WindowServer] Input processing error: {}", e);
                    return Err(e);
                }
            }
        }
    }
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    println!("=== Scarlet Window Server ===");
    println!("Phase 1: Simple cursor display and movement");

    // Initialize window server
    let mut server = match WindowServer::new() {
        Ok(server) => server,
        Err(e) => {
            println!("Failed to initialize window server: {}", e);
            return 1;
        }
    };

    // Initialize display
    if let Err(e) = server.init_display() {
        println!("Failed to initialize display: {}", e);
        return 1;
    }

    // Run main loop
    if let Err(e) = server.run() {
        println!("Window server error: {}", e);
        return 1;
    }

    0
}
