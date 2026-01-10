//! Scarlet UI - Native UI toolkit for Scarlet OS
//!
//! This library provides widgets and utilities for building graphical applications
//! on Scarlet OS using the Scarlet Window Server (SWS).
//!
//! # Architecture
//!
//! - `sws-client`: Low-level connection and protocol handling
//! - `scarlet-ui` (this crate): High-level UI toolkit
//!
//! # Example
//!
//! ```no_run
//! use scarlet_ui::{Application, Window, Color};
//!
//! fn main() {
//!     let mut app = Application::new().expect("Failed to connect");
//!     let mut window = app.create_window("Hello", 400, 300).unwrap();
//!     
//!     window.canvas().fill(Color::WHITE);
//!     window.commit();
//!     
//!     app.run(|event| {
//!         // Handle events
//!     });
//! }
//! ```

#![no_std]

extern crate scarlet_std as std;

pub mod color;
pub mod event;
pub mod graphics;
pub mod widgets;
pub mod window;

pub use color::Color;
pub use event::{Event, EventType, MouseButton};
pub use graphics::{Canvas, Point, Rect};
pub use widgets::{Button, Label, Widget};
pub use window::Window;

use std::vec::Vec;
use sws_client::{Connection, Event as SwsEvent, InputEvent};

/// Application context that manages the SWS connection
pub struct Application {
    connection: Connection,
    windows: Vec<u32>,
}

impl Application {
    /// Create a new application and connect to SWS
    pub fn new() -> Result<Self, &'static str> {
        Self::with_socket_path("/tmp/sws.sock")
    }

    /// Create a new application with a custom socket path
    pub fn with_socket_path(path: &str) -> Result<Self, &'static str> {
        let connection = Connection::connect(path).map_err(|_| "Failed to connect to SWS")?;
        Ok(Self {
            connection,
            windows: Vec::new(),
        })
    }

    /// Create a new window
    pub fn create_window(
        &mut self,
        title: &str,
        width: u32,
        height: u32,
    ) -> Result<Window, &'static str> {
        let surface_id = self
            .connection
            .create_surface(width, height)
            .map_err(|_| "Failed to create surface")?;

        self.windows.push(surface_id);

        Window::new(&mut self.connection, surface_id, title, width, height)
    }

    /// Get mutable access to the connection (for advanced use)
    pub fn connection(&mut self) -> &mut Connection {
        &mut self.connection
    }

    /// Dispatch and process events
    ///
    /// Returns the number of events processed
    pub fn dispatch(&mut self) -> Result<usize, &'static str> {
        self.connection
            .dispatch()
            .map_err(|_| "Failed to dispatch events")
    }

    /// Poll for next event
    pub fn poll_event(&mut self) -> Option<(u32, Event)> {
        // First dispatch any pending socket data
        let _ = self.connection.dispatch();

        // Convert sws-client events to scarlet-ui events
        if let Some(sws_event) = self.connection.poll_event() {
            match sws_event {
                SwsEvent::Input(input) => {
                    // Determine which window this event is for
                    // For now, send to all windows (TODO: proper targeting)
                    if let Some(&window_id) = self.windows.first() {
                        if let Some(event) = convert_input_event(&input) {
                            return Some((window_id, event));
                        }
                    }
                }
                SwsEvent::SurfaceDestroyed { surface_id } => {
                    self.windows.retain(|&id| id != surface_id);
                    return Some((surface_id, Event::WindowClose));
                }
                SwsEvent::Error { code: _ } => {
                    // Handle error
                }
            }
        }

        None
    }

    /// Commit changes for a window
    pub fn commit(&mut self, surface_id: u32) -> Result<(), &'static str> {
        self.connection
            .commit(surface_id)
            .map_err(|_| "Failed to commit")
    }

    /// Run the event loop with a callback
    pub fn run<F>(&mut self, mut handler: F) -> !
    where
        F: FnMut(u32, Event) -> bool,
    {
        loop {
            let _ = self.dispatch();

            while let Some((window_id, event)) = self.poll_event() {
                if !handler(window_id, event) {
                    // Handler returned false, exit
                    loop {
                        // Spin (no exit syscall yet)
                    }
                }
            }
        }
    }
}

/// Convert sws-client InputEvent to scarlet-ui Event
fn convert_input_event(input: &InputEvent) -> Option<Event> {
    use sws_client::event::{abs_code, event_type, key_code};

    match input.type_ {
        event_type::EV_KEY => {
            let button = match input.code {
                key_code::BTN_LEFT => Some(MouseButton::Left),
                key_code::BTN_RIGHT => Some(MouseButton::Right),
                key_code::BTN_MIDDLE => Some(MouseButton::Middle),
                _ => None,
            };

            if let Some(btn) = button {
                if input.value != 0 {
                    Some(Event::MouseDown(btn))
                } else {
                    Some(Event::MouseUp(btn))
                }
            } else {
                // Keyboard event
                Some(Event::Key {
                    code: input.code,
                    pressed: input.value != 0,
                })
            }
        }
        event_type::EV_ABS => match input.code {
            abs_code::ABS_X => Some(Event::MouseMove {
                x: input.value,
                y: -1, // Y will come in next event
            }),
            abs_code::ABS_Y => Some(Event::MouseMove {
                x: -1, // X came in previous event
                y: input.value,
            }),
            _ => None,
        },
        _ => None,
    }
}

