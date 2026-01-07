//! Window management module

use std::vec::Vec;
use std::{print, println};

/// Window ID type
pub type WindowId = u32;

/// Window properties
#[derive(Debug, Clone)]
pub struct Window {
    pub id: WindowId,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub title: Option<Vec<u8>>,
    pub visible: bool,
    pub focused: bool,
    /// Shared memory buffer for window contents (BGRA format, 4 bytes per pixel)
    pub buffer: Option<Vec<u8>>,
}

impl Window {
    /// Create a new window
    pub fn new(id: WindowId, x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            id,
            x,
            y,
            width,
            height,
            title: None,
            visible: true,
            focused: false,
            buffer: None,
        }
    }

    /// Create window with buffer
    pub fn new_with_buffer(id: WindowId, x: i32, y: i32, width: u32, height: u32) -> Self {
        // Allocate buffer (BGRA format, 4 bytes per pixel)
        let buffer_size = (width * height * 4) as usize;
        let mut buffer = Vec::new();
        buffer.resize(buffer_size, 0);

        Self {
            id,
            x,
            y,
            width,
            height,
            title: None,
            visible: true,
            focused: false,
            buffer: Some(buffer),
        }
    }

    /// Get buffer size in bytes
    pub fn buffer_size(&self) -> usize {
        (self.width * self.height * 4) as usize
    }

    /// Set window title
    pub fn set_title(&mut self, title: &str) {
        self.title = Some(title.as_bytes().to_vec());
    }

    /// Check if point is inside window bounds
    pub fn contains_point(&self, x: i32, y: i32) -> bool {
        x >= self.x
            && x < self.x + self.width as i32
            && y >= self.y
            && y < self.y + self.height as i32
    }

    /// Move window to new position
    pub fn move_to(&mut self, x: i32, y: i32) {
        self.x = x;
        self.y = y;
    }

    /// Resize window
    pub fn resize(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
    }
}

/// Window manager - manages multiple windows with Z-order
pub struct WindowManager {
    windows: Vec<Window>,
    next_id: WindowId,
    focused_window: Option<WindowId>,
}

impl WindowManager {
    /// Create a new window manager
    pub fn new() -> Self {
        Self {
            windows: Vec::new(),
            next_id: 1,
            focused_window: None,
        }
    }

    /// Create a new window
    pub fn create_window(&mut self, x: i32, y: i32, width: u32, height: u32) -> WindowId {
        let id = self.next_id;
        self.next_id += 1;

        println!(
            "[WindowManager] Creating window #{} at ({}, {}) with buffer",
            id, x, y
        );
        let window = Window::new_with_buffer(id, x, y, width, height);
        self.windows.push(window);

        // Focus the new window
        self.focus_window(id);

        id
    }

    /// Create window without buffer (for testing)
    pub fn create_window_no_buffer(&mut self, x: i32, y: i32, width: u32, height: u32) -> WindowId {
        let id = self.next_id;
        self.next_id += 1;

        println!(
            "[WindowManager] Creating window #{} at ({}, {}) without buffer",
            id, x, y
        );
        let window = Window::new(id, x, y, width, height);
        self.windows.push(window);

        // Focus the new window
        self.focus_window(id);

        id
    }

    /// Get window by ID
    pub fn get_window(&self, id: WindowId) -> Option<&Window> {
        self.windows.iter().find(|w| w.id == id)
    }

    /// Get mutable window by ID
    pub fn get_window_mut(&mut self, id: WindowId) -> Option<&mut Window> {
        self.windows.iter_mut().find(|w| w.id == id)
    }

    /// Find window at point (top-most)
    pub fn window_at_point(&self, x: i32, y: i32) -> Option<WindowId> {
        // Iterate in reverse order (top to bottom)
        self.windows
            .iter()
            .rev()
            .find(|w| w.visible && w.contains_point(x, y))
            .map(|w| w.id)
    }

    /// Focus a window
    pub fn focus_window(&mut self, id: WindowId) {
        println!("[WindowManager] Focusing window #{}", id);
        // Unfocus all windows
        for window in &mut self.windows {
            window.focused = false;
        }

        // Focus the specified window
        if let Some(window) = self.get_window_mut(id) {
            window.focused = true;
            self.focused_window = Some(id);
        }
    }

    /// Set focus to a window (alias for focus_window)
    pub fn set_focus(&mut self, id: WindowId) {
        self.focus_window(id);
    }

    /// Raise window to top (bring to front in Z-order)
    pub fn raise_to_top(&mut self, id: WindowId) {
        println!("[WindowManager] Raising window #{} to top", id);
        if let Some(index) = self.windows.iter().position(|w| w.id == id) {
            println!(
                "[WindowManager] Window was at index {}, moving to end",
                index
            );
            let window = self.windows.remove(index);
            self.windows.push(window);

            // Print current Z-order
            print!("[WindowManager] Current Z-order (bottom to top): ");
            for w in &self.windows {
                print!("#{} ", w.id);
            }
            println!();
        }
    }

    /// Get focused window ID
    pub fn get_focused_window(&self) -> Option<WindowId> {
        self.focused_window
    }

    /// Get all windows in Z-order (bottom to top)
    pub fn get_windows(&self) -> &[Window] {
        &self.windows
    }

    /// Close window
    pub fn close_window(&mut self, id: WindowId) {
        if let Some(index) = self.windows.iter().position(|w| w.id == id) {
            self.windows.remove(index);

            // Update focus if closed window was focused
            if self.focused_window == Some(id) {
                self.focused_window = self.windows.last().map(|w| w.id);
                if let Some(new_focus) = self.focused_window {
                    if let Some(window) = self.get_window_mut(new_focus) {
                        window.focused = true;
                    }
                }
            }
        }
    }

    /// Move window by delta
    pub fn move_window(&mut self, id: WindowId, dx: i32, dy: i32) {
        if let Some(window) = self.get_window_mut(id) {
            window.x += dx;
            window.y += dy;
        }
    }

    /// Set window position (absolute)
    pub fn set_window_position(&mut self, id: WindowId, x: i32, y: i32) {
        if let Some(window) = self.get_window_mut(id) {
            window.x = x;
            window.y = y;
        }
    }
}
