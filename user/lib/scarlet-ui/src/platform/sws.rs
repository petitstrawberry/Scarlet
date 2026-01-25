//! SWS (Scarlet Window Server) backend for PlatformWindow
//!
//! This implementation uses the sws-client library to create and manage windows.

use crate::geometry::Size;
use crate::buffer::Buffer;
use crate::event::{Event, MouseButton, MouseEvent};
use crate::error::Result;
use crate::platform::PlatformWindow;
use sws_client as sws;
use sws::event::{abs_code, event_type, key_code, Event as SwsEvent};
use alloc::vec::Vec;

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
    /// Get the surface ID
    pub fn surface_id(&self) -> u32 {
        self.surface_id
    }

    /// Get the connection
    pub fn connection(&self) -> &sws::Connection {
        &self.conn
    }

    /// Get mutable reference to the connection
    pub fn connection_mut(&mut self) -> &mut sws::Connection {
        &mut self.conn
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
        // Note: sws-client doesn't have a resize_surface method
        // Resize would need to be implemented in the protocol
        // For now, just update our tracked size
        self.current_size = Size {
            width: width as f32,
            height: height as f32,
        };

        Ok(())
    }

    fn close(&mut self) -> Result<()> {
        // Destroy the surface
        self.conn.destroy_surface(self.surface_id)
            .map_err(|_| crate::error::Error::IoError)?;

        Ok(())
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
                            self.pending_events.push(Event::Mouse(MouseEvent::Moved {
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
                            self.pending_events.push(Event::Mouse(MouseEvent::ButtonPressed {
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
                            self.pending_events.push(Event::Mouse(MouseEvent::ButtonReleased {
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
                            self.pending_events.push(Event::Mouse(MouseEvent::ButtonPressed {
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
                            self.pending_events.push(Event::Mouse(MouseEvent::ButtonReleased {
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
                            self.pending_events.push(Event::Mouse(MouseEvent::ButtonPressed {
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
                            self.pending_events.push(Event::Mouse(MouseEvent::ButtonReleased {
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
                    self.current_size = Size::new(width as f32, height as f32);
                    self.pending_events.push(Event::Resize { width, height });
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
                    self.pending_events.push(Event::Quit);
                    if debug {
                        scarlet_std::println!("[SWSPlatformWindow] SurfaceDestroyed");
                    }
                }
            }
            _ => {}
        }
    }
}
