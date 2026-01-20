//! Bridge to the Scarlet Window Server (SWS)
//!
//! This module handles all communication with the window server,
//! isolating sws_client types from the rest of scarlet_ui.

use crate::event::{Event, KeyEvent, MouseEvent, MouseEventKind, MouseButtons};
use crate::geometry::Point;
use sws_client::{Connection, Event as SwsEvent, InputEvent};
use std::boxed::Box;

/// Bridge to the window server (SWS)
pub struct SurfaceBridge {
    connection: Connection,
    surface_id: u32,
    pub width: u32,
    pub height: u32,
    mouse_pos: Point,
    mouse_buttons: u8,
    resize_pending: bool,
}

impl SurfaceBridge {
    pub fn new(app_id: &str, app_name: &str, menu_titles: &str, width: u32, height: u32) -> Result<Self, &'static str> {
        let mut connection = Connection::connect_default()
            .map_err(|_| "Failed to connect to window server")?;

        let surface_id = connection
            .create_surface(app_id, app_name, menu_titles, width, height)
            .map_err(|_| "Failed to create surface")?;

        Ok(Self {
            connection,
            surface_id,
            width,
            height,
            mouse_pos: Point::ZERO,
            mouse_buttons: 0,
            resize_pending: false,
        })
    }

    pub fn next_event_timeout(&mut self, _timeout_ms: u64) -> Option<Event> {
        // Dispatch events from server
        let _ = self.connection.dispatch();

        // Convert SWS events to ScarletUI events
        loop {
            match self.connection.poll_event() {
                None => return None,
                Some(sws_event) => {
                    // Convert to ScarletUI event
                    if let Some(event) = self.convert_event(sws_event) {
                        return Some(event);
                    }
                    // Continue polling if conversion returned None
                }
            }
        }
    }

    pub fn present(&mut self, node: &mut dyn crate::traits::RenderNode) -> Result<(), &'static str> {
        use crate::buffer::Buffer;

        // Get root buffer and convert RGBA to BGRA
        if let Some(buffer) = node.get_buffer() {
            let surface = self.connection
                .surface_mut(self.surface_id)
                .ok_or("Surface not found")?;

            let src_data = buffer.data();
            let dst_data = surface.buffer_mut();

            // Copy RGBA to BGRA (swap R and B channels)
            let src_len = src_data.len().min(dst_data.len());
            for i in (0..src_len).step_by(4) {
                if i + 3 < src_len {
                    dst_data[i] = src_data[i + 2];     // B <- R
                    dst_data[i + 1] = src_data[i + 1]; // G <- G
                    dst_data[i + 2] = src_data[i];     // R <- B
                    dst_data[i + 3] = src_data[i + 3]; // A <- A
                }
            }

            // Commit to server
            self.connection
                .commit(self.surface_id)
                .map_err(|_| "Failed to commit")?;
        }

        Ok(())
    }

    pub fn set_root(&mut self, _node: &mut dyn crate::traits::RenderNode) {
        // Nothing to do here, root is managed by Application
    }

    /// Check if a resize event occurred since the last check.
    /// This consumes and resets the flag.
    pub fn check_resize_pending(&mut self) -> bool {
        let result = self.resize_pending;
        self.resize_pending = false;
        result
    }

    fn convert_event(&mut self, sws_event: SwsEvent) -> Option<Event> {
        match sws_event {
            SwsEvent::Input(input) => self.convert_input_event(input),
            SwsEvent::SurfaceConfigure { width, height, .. } => {
                self.width = width;
                self.height = height;
                self.resize_pending = true;  // Mark that resize occurred
                None  // Configure events don't generate ScarletUI events
            }
            SwsEvent::SurfaceDestroyed { .. } => {
                // Window was closed
                None
            }
            SwsEvent::FocusChanged { .. } => {
                // Focus changed - we're only interested if we gained focus
                // For now, assume we always get focus events for our surface
                Some(Event::Focus(true))
            }
            SwsEvent::Error { .. } => {
                None
            }
        }
    }

    fn convert_input_event(&mut self, input: InputEvent) -> Option<Event> {
        use sws_client::event_type::{EV_ABS, EV_KEY, EV_REL};
        use sws_client::abs_code::{ABS_X, ABS_Y};
        use sws_client::key_code::{BTN_LEFT, BTN_MIDDLE, BTN_RIGHT};
        use sws_client::rel_code::{REL_X, REL_Y};

        match input.type_ {
            EV_ABS => {
                // Absolute position (mouse)
                match input.code {
                    ABS_X => {
                        self.mouse_pos.x = input.value as f32;
                    }
                    ABS_Y => {
                        self.mouse_pos.y = input.value as f32;
                    }
                    _ => return None,
                }

                // Generate mouse move event
                Some(Event::Mouse(MouseEvent {
                    position: Point {
                        x: self.mouse_pos.x,
                        y: self.mouse_pos.y,
                    },
                    buttons: MouseButtons(self.mouse_buttons),
                    kind: MouseEventKind::Move,
                }))
            }
            EV_KEY => {
                // Keyboard or button event
                match input.code {
                    BTN_LEFT | BTN_MIDDLE | BTN_RIGHT => {
                        // Mouse button
                        let button = match input.code {
                            BTN_LEFT => MouseButtons::LEFT,
                            BTN_MIDDLE => MouseButtons::MIDDLE,
                            BTN_RIGHT => MouseButtons::RIGHT,
                            _ => return None,
                        };

                        if input.value != 0 {
                            // Button pressed
                            self.mouse_buttons |= button.0;
                            Some(Event::Mouse(MouseEvent {
                                position: Point {
                                    x: self.mouse_pos.x,
                                    y: self.mouse_pos.y,
                                },
                                buttons: MouseButtons(self.mouse_buttons),
                                kind: MouseEventKind::Press,
                            }))
                        } else {
                            // Button released
                            self.mouse_buttons &= !button.0;
                            Some(Event::Mouse(MouseEvent {
                                position: Point {
                                    x: self.mouse_pos.x,
                                    y: self.mouse_pos.y,
                                },
                                buttons: MouseButtons(self.mouse_buttons),
                                kind: MouseEventKind::Release,
                            }))
                        }
                    }
                    _ => {
                        // Keyboard event - map key codes to KeyEvent
                        if input.value != 0 {
                            use sws_client::key_code;
                            let key_event = match input.code {
                                key_code::KEY_ESC => Some(KeyEvent::Escape),
                                key_code::KEY_ENTER => Some(KeyEvent::Enter),
                                key_code::KEY_BACKSPACE => Some(KeyEvent::Backspace),
                                key_code::KEY_TAB => Some(KeyEvent::Tab),
                                key_code::KEY_DELETE => Some(KeyEvent::Delete),
                                key_code::KEY_HOME => Some(KeyEvent::Home),
                                key_code::KEY_END => Some(KeyEvent::End),
                                key_code::KEY_PAGEUP => Some(KeyEvent::PageUp),
                                key_code::KEY_PAGEDOWN => Some(KeyEvent::PageDown),
                                key_code::KEY_UP => Some(KeyEvent::Up),
                                key_code::KEY_DOWN => Some(KeyEvent::Down),
                                key_code::KEY_LEFT => Some(KeyEvent::Left),
                                key_code::KEY_RIGHT => Some(KeyEvent::Right),
                                key_code::KEY_Q => Some(KeyEvent::Char('q')),
                                key_code::KEY_W => Some(KeyEvent::Char('w')),
                                key_code::KEY_E => Some(KeyEvent::Char('e')),
                                key_code::KEY_R => Some(KeyEvent::Char('r')),
                                key_code::KEY_T => Some(KeyEvent::Char('t')),
                                key_code::KEY_Y => Some(KeyEvent::Char('y')),
                                key_code::KEY_U => Some(KeyEvent::Char('u')),
                                key_code::KEY_I => Some(KeyEvent::Char('i')),
                                key_code::KEY_O => Some(KeyEvent::Char('o')),
                                key_code::KEY_P => Some(KeyEvent::Char('p')),
                                key_code::KEY_A => Some(KeyEvent::Char('a')),
                                key_code::KEY_S => Some(KeyEvent::Char('s')),
                                key_code::KEY_D => Some(KeyEvent::Char('d')),
                                key_code::KEY_F => Some(KeyEvent::Char('f')),
                                key_code::KEY_G => Some(KeyEvent::Char('g')),
                                key_code::KEY_H => Some(KeyEvent::Char('h')),
                                key_code::KEY_J => Some(KeyEvent::Char('j')),
                                key_code::KEY_K => Some(KeyEvent::Char('k')),
                                key_code::KEY_L => Some(KeyEvent::Char('l')),
                                key_code::KEY_Z => Some(KeyEvent::Char('z')),
                                key_code::KEY_X => Some(KeyEvent::Char('x')),
                                key_code::KEY_C => Some(KeyEvent::Char('c')),
                                key_code::KEY_V => Some(KeyEvent::Char('v')),
                                key_code::KEY_B => Some(KeyEvent::Char('b')),
                                key_code::KEY_N => Some(KeyEvent::Char('n')),
                                key_code::KEY_M => Some(KeyEvent::Char('m')),
                                key_code::KEY_1 => Some(KeyEvent::Char('1')),
                                key_code::KEY_2 => Some(KeyEvent::Char('2')),
                                key_code::KEY_3 => Some(KeyEvent::Char('3')),
                                key_code::KEY_4 => Some(KeyEvent::Char('4')),
                                key_code::KEY_5 => Some(KeyEvent::Char('5')),
                                key_code::KEY_6 => Some(KeyEvent::Char('6')),
                                key_code::KEY_7 => Some(KeyEvent::Char('7')),
                                key_code::KEY_8 => Some(KeyEvent::Char('8')),
                                key_code::KEY_9 => Some(KeyEvent::Char('9')),
                                key_code::KEY_0 => Some(KeyEvent::Char('0')),
                                key_code::KEY_SPACE => Some(KeyEvent::Char(' ')),
                                key_code::KEY_MINUS => Some(KeyEvent::Char('-')),
                                key_code::KEY_EQUAL => Some(KeyEvent::Char('=')),
                                _ => None,
                            };

                            key_event.map(Event::Key)
                        } else {
                            None
                        }
                    }
                }
            }
            EV_REL => {
                // Relative movement (mouse)
                match input.code {
                    REL_X => {
                        self.mouse_pos.x += input.value as f32;
                    }
                    REL_Y => {
                        self.mouse_pos.y += input.value as f32;
                    }
                    _ => return None,
                }

                // Generate mouse move event
                Some(Event::Mouse(MouseEvent {
                    position: Point {
                        x: self.mouse_pos.x,
                        y: self.mouse_pos.y,
                    },
                    buttons: MouseButtons(self.mouse_buttons),
                    kind: MouseEventKind::Move,
                }))
            }
            _ => None,
        }
    }
}
