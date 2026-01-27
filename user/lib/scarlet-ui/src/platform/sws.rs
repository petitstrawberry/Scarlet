//! SWS (Scarlet Window Server) backend for PlatformWindow
//!
//! This implementation uses the sws-client library to create and manage windows.

use crate::geometry::{Point, Size};
use crate::buffer::Buffer;
use crate::event::{Event, MouseButton, MouseEvent};
use crate::error::Result;
use crate::platform::PlatformWindow;
use sws_client as sws;
use sws::event::{abs_code, event_type, key_code, Event as SwsEvent};
use alloc::vec::Vec;
use alloc::string::String;

/// SWS platform window implementation
pub struct SWSPlatformWindow {
    conn: sws::Connection,
    surface_id: u32,
    current_size: Size,
    pending_events: Vec<Event>,
    pending_head: usize,
    pointer_x: i32,
    pointer_y: i32,
    pending_move: bool,
}

impl SWSPlatformWindow {
    /// Get the connection
    pub fn connection(&self) -> &sws::Connection {
        &self.conn
    }

    /// Get mutable reference to the connection
    pub fn connection_mut(&mut self) -> &mut sws::Connection {
        &mut self.conn
    }

    /// Create a new platform window with a specific window type
    pub fn create_with_type(app_id: &str, title: &str, size: Size, window_type: u32) -> Result<Self> {
        Self::create_with_type_and_menu_and_policies(app_id, title, size, window_type, "", true, window_type == sws_protocol::window_types::NORMAL)
    }

    /// Create a new platform window with a specific window type and initial menu titles
    pub fn create_with_type_and_menu(
        app_id: &str,
        title: &str,
        size: Size,
        window_type: u32,
        menu_titles: &str,
    ) -> Result<Self> {
        Self::create_with_type_and_menu_and_policies(
            app_id,
            title,
            size,
            window_type,
            menu_titles,
            true,
            window_type == sws_protocol::window_types::NORMAL,
        )
    }

    /// Create a new platform window with a specific window type, menu titles, and focus policies
    pub fn create_with_type_and_menu_and_policies(
        app_id: &str,
        title: &str,
        size: Size,
        window_type: u32,
        menu_titles: &str,
        focus_on_create: bool,
        active_on_focus: bool,
    ) -> Result<Self> {
        // Connect to SWS
        let mut conn = sws::Connection::connect("/tmp/sws.sock")
            .map_err(|_| crate::error::Error::ConnectionFailed)?;

        // Create surface with type
        let surface_id = conn.create_surface_with_type_and_policies(
            app_id,
            title,
            menu_titles,
            size.width as u32,
            size.height as u32,
            window_type,
            true,
            focus_on_create,
            active_on_focus,
        ).map_err(|_| crate::error::Error::SurfaceCreationFailed)?;

        Ok(Self {
            conn,
            surface_id,
            current_size: size,
            pending_events: Vec::new(),
            pending_head: 0,
            pointer_x: 0,
            pointer_y: 0,
            pending_move: false,
        })
    }

    pub fn new_with_menu(app_id: &str, title: &str, size: Size, menu_titles: &str) -> Result<Self> {
        Self::create_with_type_and_menu_and_policies(
            app_id,
            title,
            size,
            sws_protocol::window_types::NORMAL,
            menu_titles,
            true,
            true,
        )
    }

    pub fn new_with_menu_and_policies(
        app_id: &str,
        title: &str,
        size: Size,
        menu_titles: &str,
        focus_on_create: bool,
        active_on_focus: bool,
    ) -> Result<Self> {
        Self::create_with_type_and_menu_and_policies(
            app_id,
            title,
            size,
            sws_protocol::window_types::NORMAL,
            menu_titles,
            focus_on_create,
            active_on_focus,
        )
    }

    fn sanitize_menu_titles(menu_titles: &str) -> &str {
        if menu_titles.chars().any(|c| {
            c.is_control() && c != '\n' && c != '\r' && c != '\t'
        }) {
            ""
        } else {
            menu_titles
        }
    }

    fn push_event(&mut self, event: Event) {
        // Coalesce consecutive mouse-move events to reduce work.
        if let Event::Mouse(MouseEvent::Moved { .. }) = event {
            if let Some(last) = self.pending_events.last_mut() {
                if let Event::Mouse(MouseEvent::Moved { x, y }) = last {
                    if let Event::Mouse(MouseEvent::Moved { x: new_x, y: new_y }) = event {
                        *x = new_x;
                        *y = new_y;
                        return;
                    }
                }
            }
        }
        self.pending_events.push(event);
    }
}

impl PlatformWindow for SWSPlatformWindow {
    fn new(app_id: &str, title: &str, size: Size) -> Result<Self> {
        // Connect to SWS
        let mut conn = sws::Connection::connect("/tmp/sws.sock")
            .map_err(|_| crate::error::Error::ConnectionFailed)?;

        // Create surface
        let surface_id = conn.create_surface(
            app_id,
            title,
            "",
            size.width as u32,
            size.height as u32,
        ).map_err(|_| crate::error::Error::SurfaceCreationFailed)?;

        Ok(Self {
            conn,
            surface_id,
            current_size: size,
            pending_events: Vec::new(),
            pending_head: 0,
            pointer_x: 0,
            pointer_y: 0,
            pending_move: false,
        })
    }

    fn poll_event(&mut self) -> Option<Event> {
        let debug = crate::debug::is_enabled();
        if self.pending_head >= self.pending_events.len() {
            self.pending_events.clear();
            self.pending_head = 0;
        }

        let _ = self.conn.dispatch().ok();

        while let Some(ev) = self.conn.poll_event() {
            self.handle_sws_event(ev);
        }

        if self.pending_head < self.pending_events.len() {
            let ev = self.pending_events[self.pending_head].clone();
            self.pending_head += 1;
            if self.pending_head >= self.pending_events.len() {
                self.pending_events.clear();
                self.pending_head = 0;
            }
            if debug {
                scarlet_std::println!("[SWSPlatformWindow] poll_event: {:?}", ev);
            }
            Some(ev)
        } else {
            None
        }
    }

    fn present(&mut self, buffer: &Buffer) {
        // Get the surface and copy pixels
        if let Some(surface) = self.conn.surface_mut(self.surface_id) {
            // Get the shared memory buffer
            surface.with_buffer(|shm_buf, width, height| {
                // SWS shared memory is width * height * 4 bytes (BGRA u8 array)
                let src_data = buffer.data(); // &[u8]
                let shm_len = (width * height * 4) as usize;
                let dst_data = unsafe {
                    core::slice::from_raw_parts_mut(shm_buf.as_mut_ptr(), shm_len)
                };

                // Copy u8 bytes directly
                let copy_len = src_data.len().min(shm_len);
                dst_data[..copy_len].copy_from_slice(&src_data[..copy_len]);
            });
        }

        // Commit the surface
        let _ = self.conn.commit(self.surface_id);
    }

    fn set_title(&mut self, title: &str) {
        // Note: sws-client doesn't have a set_surface_title method
        // The title is set during surface creation
        let _ = title;
    }

    fn size(&self) -> Size {
        self.current_size
    }

    fn resize(&mut self, width: u32, height: u32) -> Result<()> {
        if width == 0 || height == 0 {
            return Err(crate::error::Error::InvalidSize { width, height });
        }

        let new_size = Size {
            width: width as f32,
            height: height as f32,
        };

        if self.current_size == new_size {
            return Ok(());
        }

        self.conn
            .resize_window(self.surface_id, width, height)
            .map_err(|_| crate::error::Error::IoError)?;

        self.current_size = new_size;
        Ok(())
    }

    fn close(&mut self) -> Result<()> {
        // Destroy the surface
        self.conn.destroy_surface(self.surface_id)
            .map_err(|_| crate::error::Error::IoError)?;

        Ok(())
    }

    fn minimize(&mut self) -> Result<()> {
        self.conn.minimize_window(self.surface_id)
            .map_err(|_| crate::error::Error::IoError)
    }

    fn maximize(&mut self) -> Result<()> {
        self.conn.maximize_window(self.surface_id)
            .map_err(|_| crate::error::Error::IoError)
    }

    fn restore(&mut self) -> Result<()> {
        self.conn.restore_window(self.surface_id)
            .map_err(|_| crate::error::Error::IoError)
    }

    fn request_move(&mut self) -> Result<()> {
        self.conn.request_move_window(self.surface_id)
            .map_err(|_| crate::error::Error::IoError)
    }

    fn create_popup(&mut self, position: Point, size: Size) -> Result<u32> {
        // Create a popup window with ALWAYS_ON_TOP type
        let popup_surface_id = self
            .conn
            .create_surface_with_type_and_policies(
                "org.scarlet-os.popup",
                "Popup",
                "",
                size.width as u32,
                size.height as u32,
                sws_protocol::window_types::ALWAYS_ON_TOP,
                true,
                true,
                false,
            )
            .map_err(|_| crate::error::Error::SurfaceCreationFailed)?;

        // Position the popup
        self.conn.move_window(popup_surface_id, position.x as i32, position.y as i32)
            .map_err(|_| crate::error::Error::IoError)?;

        Ok(popup_surface_id)
    }

    fn destroy_popup(&mut self, surface_id: u32) -> Result<()> {
        self.conn.destroy_surface(surface_id)
            .map_err(|_| crate::error::Error::IoError)
    }

    fn set_workarea(&mut self, x: i32, y: i32, width: u32, height: u32) -> Result<()> {
        self.conn.set_workarea(x, y, width, height)
            .map_err(|_| crate::error::Error::IoError)
    }

    fn create_window_with_type(
        &mut self,
        app_id: &str,
        title: &str,
        size: Size,
        window_type: u32,
    ) -> Result<Self>
    where
        Self: Sized,
    {
        let mut conn = sws::Connection::connect("/tmp/sws.sock")
            .map_err(|_| crate::error::Error::ConnectionFailed)?;

        let surface_id = conn.create_surface_with_type(
            app_id,
            title,
            "",
            size.width as u32,
            size.height as u32,
            window_type,
        ).map_err(|_| crate::error::Error::SurfaceCreationFailed)?;

        Ok(Self {
            conn,
            surface_id,
            current_size: size,
            pending_events: Vec::new(),
            pending_head: 0,
            pointer_x: 0,
            pointer_y: 0,
            pending_move: false,
        })
    }

    fn move_window(&mut self, x: i32, y: i32) -> Result<()> {
        self.conn.move_window(self.surface_id, x, y)
            .map_err(|_| crate::error::Error::IoError)
    }

    fn set_window_type(&mut self, surface_id: u32, window_type: u32) -> Result<()> {
        self.conn.set_window_type(surface_id, window_type)
            .map_err(|_| crate::error::Error::IoError)
    }

    fn get_screen_size(&mut self) -> Result<(u32, u32)> {
        self.conn.get_screen_size()
            .map_err(|_| crate::error::Error::IoError)
    }

    fn surface_id(&self) -> u32 {
        self.surface_id
    }

    fn set_resizable(&mut self, resizable: bool) -> Result<()> {
        self.conn.set_window_resizable(self.surface_id, resizable)
            .map_err(|_| crate::error::Error::IoError)?;

        if resizable {
            let _ = self.conn.set_window_size_limits(self.surface_id, sws::WindowSizeLimits::NONE);
        } else {
            let limits = sws::WindowSizeLimits {
                min_width: self.current_size.width.max(0.0) as u32,
                min_height: self.current_size.height.max(0.0) as u32,
                max_width: self.current_size.width.max(0.0) as u32,
                max_height: self.current_size.height.max(0.0) as u32,
            };
            let _ = self.conn.set_window_size_limits(self.surface_id, limits);
        }

        Ok(())
    }

    fn set_menu_titles(&mut self, menu_titles: &str) -> Result<()> {
        self.conn
            .set_window_menu_titles(self.surface_id, menu_titles)
            .map_err(|_| crate::error::Error::IoError)
    }
}

impl SWSPlatformWindow {
    fn handle_sws_event(&mut self, ev: SwsEvent) {
        let debug = crate::debug::is_enabled();
        if debug {
            scarlet_std::println!("[SWSPlatformWindow] sws_event: {:?}", ev);
        }
        match ev {
            SwsEvent::Input(input) => {
                if input.surface_id != self.surface_id {
                    return;
                }

                match (input.type_, input.code) {
                    (event_type::EV_ABS, abs_code::ABS_X) => {
                        self.pointer_x = input.value;
                        self.pending_move = true;
                        if debug {
                            scarlet_std::println!("[SWSPlatformWindow] ABS_X: {}", input.value);
                        }
                    }
                    (event_type::EV_ABS, abs_code::ABS_Y) => {
                        self.pointer_y = input.value;
                        self.pending_move = true;
                        if debug {
                            scarlet_std::println!("[SWSPlatformWindow] ABS_Y: {}", input.value);
                        }
                    }
                    (event_type::EV_SYN, _) => {
                        if self.pending_move {
                            self.push_event(Event::Mouse(MouseEvent::Moved {
                                x: self.pointer_x,
                                y: self.pointer_y,
                            }));
                            if debug {
                                scarlet_std::println!(
                                    "[SWSPlatformWindow] MouseMoved: x={}, y={}",
                                    self.pointer_x,
                                    self.pointer_y
                                );
                            }
                            self.pending_move = false;
                        }
                    }
                    (event_type::EV_KEY, key_code::BTN_LEFT) => {
                        let button = MouseButton::Left;
                        if input.value != 0 {
                            self.push_event(Event::Mouse(MouseEvent::ButtonPressed {
                                button,
                                x: self.pointer_x,
                                y: self.pointer_y,
                            }));
                            if debug {
                                scarlet_std::println!(
                                    "[SWSPlatformWindow] MouseDown: left x={}, y={}",
                                    self.pointer_x,
                                    self.pointer_y
                                );
                            }
                        } else {
                            self.push_event(Event::Mouse(MouseEvent::ButtonReleased {
                                button,
                                x: self.pointer_x,
                                y: self.pointer_y,
                            }));
                            if debug {
                                scarlet_std::println!(
                                    "[SWSPlatformWindow] MouseUp: left x={}, y={}",
                                    self.pointer_x,
                                    self.pointer_y
                                );
                            }
                        }
                    }
                    (event_type::EV_KEY, key_code::BTN_RIGHT) => {
                        let button = MouseButton::Right;
                        if input.value != 0 {
                            self.push_event(Event::Mouse(MouseEvent::ButtonPressed {
                                button,
                                x: self.pointer_x,
                                y: self.pointer_y,
                            }));
                            if debug {
                                scarlet_std::println!(
                                    "[SWSPlatformWindow] MouseDown: right x={}, y={}",
                                    self.pointer_x,
                                    self.pointer_y
                                );
                            }
                        } else {
                            self.push_event(Event::Mouse(MouseEvent::ButtonReleased {
                                button,
                                x: self.pointer_x,
                                y: self.pointer_y,
                            }));
                            if debug {
                                scarlet_std::println!(
                                    "[SWSPlatformWindow] MouseUp: right x={}, y={}",
                                    self.pointer_x,
                                    self.pointer_y
                                );
                            }
                        }
                    }
                    (event_type::EV_KEY, key_code::BTN_MIDDLE) => {
                        let button = MouseButton::Middle;
                        if input.value != 0 {
                            self.push_event(Event::Mouse(MouseEvent::ButtonPressed {
                                button,
                                x: self.pointer_x,
                                y: self.pointer_y,
                            }));
                            if debug {
                                scarlet_std::println!(
                                    "[SWSPlatformWindow] MouseDown: middle x={}, y={}",
                                    self.pointer_x,
                                    self.pointer_y
                                );
                            }
                        } else {
                            self.push_event(Event::Mouse(MouseEvent::ButtonReleased {
                                button,
                                x: self.pointer_x,
                                y: self.pointer_y,
                            }));
                            if debug {
                                scarlet_std::println!(
                                    "[SWSPlatformWindow] MouseUp: middle x={}, y={}",
                                    self.pointer_x,
                                    self.pointer_y
                                );
                            }
                        }
                    }
                    _ => {}
                }
            }
            SwsEvent::SurfaceConfigure {
                surface_id,
                width,
                height,
            } => {
                if surface_id == self.surface_id {
                    self.push_event(Event::Resize { width, height });
                    if debug {
                        scarlet_std::println!(
                            "[SWSPlatformWindow] SurfaceConfigure: {}x{}",
                            width,
                            height
                        );
                    }
                }
            }
            SwsEvent::SurfaceDestroyed { surface_id } => {
                if surface_id == self.surface_id {
                    self.push_event(Event::Quit);
                    if debug {
                        scarlet_std::println!("[SWSPlatformWindow] SurfaceDestroyed");
                    }
                }
            }
            SwsEvent::MenuItemActivated {
                window_id,
                menu_item_id,
            } => {
                if window_id == self.surface_id {
                    self.push_event(Event::MenuItemActivated {
                        window_id,
                        menu_item_id,
                    });
                }
            }
            SwsEvent::FocusChanged {
                window_id,
                app_id,
                app_name,
                title,
                menu_titles,
            } => {
                let menu_titles = Self::sanitize_menu_titles(&menu_titles);
                // Push FocusChanged event for all windows to receive
                // This allows TaskBar to update its menu based on focus changes
                scarlet_std::println!("[SWSPlatformWindow] FocusChanged: window_id={}, app_name={}, menu_titles={}", window_id, app_name, menu_titles);
                self.push_event(Event::Custom {
                    event_type: 0xF0C0F, // FocusChanged event type
                    data: {
                        // Encode the focus change data
                        let mut data = Vec::new();
                        data.extend_from_slice(&window_id.to_le_bytes());
                        data.extend_from_slice(&(app_id.len() as u32).to_le_bytes());
                        data.extend_from_slice(app_id.as_bytes());
                        data.extend_from_slice(&(app_name.len() as u32).to_le_bytes());
                        data.extend_from_slice(app_name.as_bytes());
                        data.extend_from_slice(&(title.len() as u32).to_le_bytes());
                        data.extend_from_slice(title.as_bytes());
                        data.extend_from_slice(&(menu_titles.len() as u32).to_le_bytes());
                        data.extend_from_slice(menu_titles.as_bytes());
                        data
                    },
                });
            }
            SwsEvent::ActiveAppChanged {
                window_id,
                app_id,
                app_name,
                title,
                menu_titles,
            } => {
                let menu_titles = Self::sanitize_menu_titles(&menu_titles);
                // Push ActiveAppChanged event for TaskBar to update menu bar
                // This is ONLY sent for normal windows (not TaskBar/Desktop/etc)
                // and only when the active APPLICATION changes (same app, different window = no broadcast)
                scarlet_std::println!("[SWSPlatformWindow] ActiveAppChanged: window_id={}, app_name={}, menu_titles={}", window_id, app_name, menu_titles);
                self.push_event(Event::Custom {
                    event_type: 0xF0C0A, // ActiveAppChanged event type
                    data: {
                        // Encode the active app change data (same format as FocusChanged)
                        let mut data = Vec::new();
                        data.extend_from_slice(&window_id.to_le_bytes());
                        data.extend_from_slice(&(app_id.len() as u32).to_le_bytes());
                        data.extend_from_slice(app_id.as_bytes());
                        data.extend_from_slice(&(app_name.len() as u32).to_le_bytes());
                        data.extend_from_slice(app_name.as_bytes());
                        data.extend_from_slice(&(title.len() as u32).to_le_bytes());
                        data.extend_from_slice(title.as_bytes());
                        data.extend_from_slice(&(menu_titles.len() as u32).to_le_bytes());
                        data.extend_from_slice(menu_titles.as_bytes());
                        data
                    },
                });
            }
            _ => {}
        }
    }
}
