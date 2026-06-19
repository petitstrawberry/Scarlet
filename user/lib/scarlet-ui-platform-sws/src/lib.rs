//! SWS (Scarlet Window Server) backend for PlatformWindow
//!
//! This implementation uses the sws-client library to create and manage windows.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;
#[cfg(not(feature = "std"))]
extern crate scarlet_std as std;

use alloc::boxed::Box;
use alloc::vec::Vec;
use scarlet_ui_core::buffer::Buffer;
use scarlet_ui_core::compositor::DamageRect;
use scarlet_ui_core::element::TextInputElementState;
use scarlet_ui_core::error::Result;
use scarlet_ui_core::event::{Event, KeyCode, KeyEvent, MouseButton, MouseEvent};
use scarlet_ui_core::geometry::{Point, Rect, Size};
use scarlet_ui_core::platform::{PlatformBackend, PlatformWindow, WindowCreateRequest};
use sws::event::{Event as SwsEvent, abs_code, event_type, key_code};
use sws_client as sws;

#[cfg(feature = "std")]
macro_rules! logln {
    ($($arg:tt)*) => {
        std::println!($($arg)*)
    };
}

#[cfg(not(feature = "std"))]
macro_rules! logln {
    ($($arg:tt)*) => {
        scarlet_std::println!($($arg)*)
    };
}

const DEFAULT_SCALE_MILLI: u32 = 1000;

const KEY_LEFTCTRL: u16 = 0x1d;
const KEY_LEFTSHIFT: u16 = 0x2a;
const KEY_RIGHTSHIFT: u16 = 0x36;
const KEY_LEFTALT: u16 = 0x38;
const KEY_RIGHTCTRL: u16 = 0x61;
const KEY_RIGHTALT: u16 = 0x64;

#[derive(Clone, Copy, Debug)]
struct TextInputContext {
    context_id: u32,
    serial: u32,
    enabled: bool,
}

/// SWS platform window implementation
pub struct SWSPlatformWindow {
    conn: sws::Connection,
    surface_id: u32,
    scale_milli: u32,
    current_size: Size,
    pending_events: Vec<Event>,
    pending_head: usize,
    pointer_x: i32,
    pointer_y: i32,
    pending_move: bool,
    left_shift_pressed: bool,
    right_shift_pressed: bool,
    left_control_pressed: bool,
    right_control_pressed: bool,
    left_alt_pressed: bool,
    right_alt_pressed: bool,
    text_input: Option<TextInputContext>,
    needs_full_present: bool,
}

/// Scarlet Window Server backend.
#[derive(Default)]
pub struct SwsBackend;

impl SwsBackend {
    /// Create an SWS backend.
    ///
    /// # Returns
    ///
    /// A backend that creates SWS platform windows.
    pub fn new() -> Self {
        Self
    }
}

impl PlatformBackend for SwsBackend {
    fn output_scale_milli(&mut self) -> u32 {
        SWSPlatformWindow::query_output_scale()
    }

    fn create_window(&mut self, request: WindowCreateRequest) -> Result<Box<dyn PlatformWindow>> {
        Ok(Box::new(
            SWSPlatformWindow::create_with_type_and_menu_and_policies(
                &request.app_id,
                &request.title,
                request.size,
                request.window_type,
                &request.menu_titles,
                request.focus_on_create,
                request.active_on_focus,
                request.opaque,
            )?,
        ))
    }
}

impl SWSPlatformWindow {
    fn sanitize_scale(scale_milli: u32) -> u32 {
        scale_milli.max(1)
    }

    pub fn query_output_scale() -> u32 {
        let Ok(mut conn) = sws::Connection::connect("/tmp/sws.sock") else {
            return DEFAULT_SCALE_MILLI;
        };
        conn.get_output_scale()
            .map(Self::sanitize_scale)
            .unwrap_or(DEFAULT_SCALE_MILLI)
    }

    fn logical_to_physical_len_with_scale(value: u32, scale_milli: u32) -> u32 {
        ((value as u64)
            .saturating_mul(scale_milli as u64)
            .saturating_add(999)
            / 1000)
            .max(1) as u32
    }

    fn logical_to_physical_len(&self, value: u32) -> u32 {
        Self::logical_to_physical_len_with_scale(value, self.scale_milli)
    }

    fn physical_to_logical_len(&self, value: u32) -> u32 {
        ((value as u64)
            .saturating_mul(1000)
            .saturating_add(self.scale_milli as u64 - 1)
            / self.scale_milli as u64)
            .max(1) as u32
    }

    fn logical_to_physical_pos(&self, value: i32) -> i32 {
        ((value as i64).saturating_mul(self.scale_milli as i64) / 1000) as i32
    }

    fn physical_to_logical_pos(&self, value: i32) -> i32 {
        ((value as i64).saturating_mul(1000) / self.scale_milli as i64) as i32
    }

    /// Get the connection
    pub fn connection(&self) -> &sws::Connection {
        &self.conn
    }

    /// Get mutable reference to the connection
    pub fn connection_mut(&mut self) -> &mut sws::Connection {
        &mut self.conn
    }

    /// Create a new platform window with a specific window type
    pub fn create_with_type(
        app_id: &str,
        title: &str,
        size: Size,
        window_type: u32,
    ) -> Result<Self> {
        Self::create_with_type_and_menu_and_policies(
            app_id,
            title,
            size,
            window_type,
            "",
            true,
            window_type == sws_protocol::window_types::NORMAL,
            true,
        )
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
            true,
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
        opaque: bool,
    ) -> Result<Self> {
        // Connect to SWS
        let mut conn = sws::Connection::connect("/tmp/sws.sock")
            .map_err(|_| scarlet_ui_core::error::Error::ConnectionFailed)?;
        let scale_milli = conn
            .get_output_scale()
            .map(Self::sanitize_scale)
            .unwrap_or(DEFAULT_SCALE_MILLI);
        let physical_width =
            Self::logical_to_physical_len_with_scale(size.width.max(1.0) as u32, scale_milli);
        let physical_height =
            Self::logical_to_physical_len_with_scale(size.height.max(1.0) as u32, scale_milli);

        // Create surface with type
        let surface_id = conn
            .create_surface_with_type_and_policies(
                app_id,
                title,
                menu_titles,
                physical_width,
                physical_height,
                window_type,
                true,
                focus_on_create,
                active_on_focus,
            )
            .map_err(|_| scarlet_ui_core::error::Error::SurfaceCreationFailed)?;

        if !opaque {
            conn.set_window_has_alpha_content(surface_id, true)
                .map_err(|_| scarlet_ui_core::error::Error::IoError)?;
        }

        Ok(Self {
            conn,
            surface_id,
            scale_milli,
            current_size: size,
            pending_events: Vec::new(),
            pending_head: 0,
            pointer_x: 0,
            pointer_y: 0,
            pending_move: false,
            left_shift_pressed: false,
            right_shift_pressed: false,
            left_control_pressed: false,
            right_control_pressed: false,
            left_alt_pressed: false,
            right_alt_pressed: false,
            text_input: None,
            needs_full_present: false,
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
        opaque: bool,
    ) -> Result<Self> {
        Self::create_with_type_and_menu_and_policies(
            app_id,
            title,
            size,
            sws_protocol::window_types::NORMAL,
            menu_titles,
            focus_on_create,
            active_on_focus,
            opaque,
        )
    }

    fn sanitize_menu_titles(menu_titles: &str) -> &str {
        if menu_titles
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\r' && c != '\t')
        {
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

    pub fn sync_text_input(&mut self, state: Option<&TextInputElementState>) {
        let Some(state) = state else {
            if let Some(context) = self.text_input.as_mut()
                && context.enabled
            {
                if self.conn.disable_text_input(context.context_id).is_ok() {
                    context.enabled = false;
                }
            }
            return;
        };

        if self.text_input.is_none() {
            match self.conn.create_text_input_context(self.surface_id, 0) {
                Ok((context_id, serial)) => {
                    self.text_input = Some(TextInputContext {
                        context_id,
                        serial,
                        enabled: false,
                    });
                }
                Err(_) => return,
            }
        }

        let Some(context) = self.text_input else {
            return;
        };
        let context_id = context.context_id;
        let cursor_rect = self.logical_rect_to_physical(state.cursor_rect);
        let _ = self.conn.set_text_input_cursor_rect(
            context_id,
            cursor_rect.origin.x as i32,
            cursor_rect.origin.y as i32,
            cursor_rect.size.width.max(1.0) as u32,
            cursor_rect.size.height.max(1.0) as u32,
        );
        let cursor_byte = state.surrounding_text.len() as u32;
        let _ = self.conn.set_text_input_surrounding_text(
            context_id,
            cursor_byte,
            cursor_byte,
            &state.surrounding_text,
        );
        let _ = self.conn.set_text_input_content_type(
            context_id,
            sws_protocol::text_input_content_hints::NONE,
            sws_protocol::text_input_content_purpose::NORMAL,
        );
        let _ = self
            .conn
            .commit_text_input_state(context_id, context.serial);

        if !context.enabled && self.conn.enable_text_input(context_id).is_ok() {
            if let Some(context) = self.text_input.as_mut() {
                context.enabled = true;
            }
        }
    }

    /// Set a text-input cursor rectangle using ScarletUI logical coordinates.
    pub fn set_text_input_cursor_rect(
        &mut self,
        context_id: u32,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    ) -> core::result::Result<(), sws::Error> {
        let cursor_rect = self.logical_rect_to_physical(Rect::from_xywh(
            x as f32,
            y as f32,
            width.max(1) as f32,
            height.max(1) as f32,
        ));
        self.conn.set_text_input_cursor_rect(
            context_id,
            cursor_rect.origin.x as i32,
            cursor_rect.origin.y as i32,
            cursor_rect.size.width.max(1.0) as u32,
            cursor_rect.size.height.max(1.0) as u32,
        )
    }

    fn logical_rect_to_physical(&self, rect: Rect) -> Rect {
        Rect::from_xywh(
            self.logical_to_physical_pos(rect.origin.x as i32) as f32,
            self.logical_to_physical_pos(rect.origin.y as i32) as f32,
            self.logical_to_physical_len(rect.size.width.max(1.0) as u32) as f32,
            self.logical_to_physical_len(rect.size.height.max(1.0) as u32) as f32,
        )
    }

    fn copy_buffer_region(
        buffer: &Buffer,
        dst_data: &mut [u8],
        dst_width: u32,
        dst_height: u32,
        damage: DamageRect,
    ) -> Option<DamageRect> {
        let (x, y, width, height) = damage;
        let x = x.min(dst_width);
        let y = y.min(dst_height);
        let width = width.min(dst_width.saturating_sub(x));
        let height = height.min(dst_height.saturating_sub(y));
        if width == 0 || height == 0 {
            return None;
        }

        let src_data = buffer.data();
        let src_width = buffer.width() as usize;
        let src_height = buffer.height() as usize;
        let dst_width = dst_width as usize;
        let x_start = x as usize;
        let x_end = x.saturating_add(width) as usize;
        let clear_len = width as usize * 4;

        for row in y as usize..y.saturating_add(height) as usize {
            let dst_offset = row
                .saturating_mul(dst_width)
                .saturating_add(x_start)
                .saturating_mul(4);
            let dst_row_end = dst_offset.saturating_add(clear_len).min(dst_data.len());
            if dst_offset >= dst_row_end {
                continue;
            }

            if row >= src_height || x_start >= src_width {
                dst_data[dst_offset..dst_row_end].fill(0);
                continue;
            }

            let copy_x_end = x_end.min(src_width);
            let copy_len = copy_x_end.saturating_sub(x_start).saturating_mul(4);
            if copy_len > 0 {
                let src_offset = row
                    .saturating_mul(src_width)
                    .saturating_add(x_start)
                    .saturating_mul(4);
                let src_end = src_offset.saturating_add(copy_len).min(src_data.len());
                let dst_copy_end = dst_offset.saturating_add(src_end.saturating_sub(src_offset));
                if src_offset < src_end && dst_copy_end <= dst_data.len() {
                    dst_data[dst_offset..dst_copy_end]
                        .copy_from_slice(&src_data[src_offset..src_end]);
                }
            }

            if copy_x_end < x_end {
                let clear_start = dst_offset.saturating_add(copy_len);
                if clear_start < dst_row_end {
                    dst_data[clear_start..dst_row_end].fill(0);
                }
            }
        }

        Some((x, y, width, height))
    }

    fn map_key_code(code: u16) -> KeyCode {
        match code {
            key_code::KEY_ESC => KeyCode::Escape,
            key_code::KEY_ENTER => KeyCode::Enter,
            key_code::KEY_TAB => KeyCode::Tab,
            key_code::KEY_BACKSPACE => KeyCode::Backspace,
            key_code::KEY_SPACE => KeyCode::Space,
            key_code::KEY_LEFT => KeyCode::Left,
            key_code::KEY_RIGHT => KeyCode::Right,
            key_code::KEY_UP => KeyCode::Up,
            key_code::KEY_DOWN => KeyCode::Down,
            key_code::KEY_HOME => KeyCode::Home,
            key_code::KEY_END => KeyCode::End,
            key_code::KEY_PAGEUP => KeyCode::PageUp,
            key_code::KEY_PAGEDOWN => KeyCode::PageDown,
            key_code::KEY_INSERT => KeyCode::Insert,
            key_code::KEY_DELETE => KeyCode::Delete,
            key_code::KEY_F1 => KeyCode::F(1),
            key_code::KEY_F2 => KeyCode::F(2),
            key_code::KEY_F3 => KeyCode::F(3),
            key_code::KEY_F4 => KeyCode::F(4),
            key_code::KEY_F5 => KeyCode::F(5),
            key_code::KEY_F6 => KeyCode::F(6),
            key_code::KEY_F7 => KeyCode::F(7),
            key_code::KEY_F8 => KeyCode::F(8),
            key_code::KEY_F9 => KeyCode::F(9),
            key_code::KEY_F10 => KeyCode::F(10),
            key_code::KEY_F11 => KeyCode::F(11),
            key_code::KEY_F12 => KeyCode::F(12),
            _ => Self::map_key_char(code).map_or(KeyCode::Unknown, KeyCode::Char),
        }
    }

    fn map_key_char(code: u16) -> Option<char> {
        match code {
            key_code::KEY_1 => Some('1'),
            key_code::KEY_2 => Some('2'),
            key_code::KEY_3 => Some('3'),
            key_code::KEY_4 => Some('4'),
            key_code::KEY_5 => Some('5'),
            key_code::KEY_6 => Some('6'),
            key_code::KEY_7 => Some('7'),
            key_code::KEY_8 => Some('8'),
            key_code::KEY_9 => Some('9'),
            key_code::KEY_0 => Some('0'),
            key_code::KEY_Q => Some('q'),
            key_code::KEY_W => Some('w'),
            key_code::KEY_E => Some('e'),
            key_code::KEY_R => Some('r'),
            key_code::KEY_T => Some('t'),
            key_code::KEY_Y => Some('y'),
            key_code::KEY_U => Some('u'),
            key_code::KEY_I => Some('i'),
            key_code::KEY_O => Some('o'),
            key_code::KEY_P => Some('p'),
            key_code::KEY_A => Some('a'),
            key_code::KEY_S => Some('s'),
            key_code::KEY_D => Some('d'),
            key_code::KEY_F => Some('f'),
            key_code::KEY_G => Some('g'),
            key_code::KEY_H => Some('h'),
            key_code::KEY_J => Some('j'),
            key_code::KEY_K => Some('k'),
            key_code::KEY_L => Some('l'),
            key_code::KEY_Z => Some('z'),
            key_code::KEY_X => Some('x'),
            key_code::KEY_C => Some('c'),
            key_code::KEY_V => Some('v'),
            key_code::KEY_B => Some('b'),
            key_code::KEY_N => Some('n'),
            key_code::KEY_M => Some('m'),
            key_code::KEY_COMMA => Some(','),
            key_code::KEY_DOT => Some('.'),
            key_code::KEY_SLASH => Some('/'),
            key_code::KEY_SEMICOLON => Some(';'),
            key_code::KEY_APOSTROPHE => Some('\''),
            key_code::KEY_LEFTBRACE => Some('['),
            key_code::KEY_RIGHTBRACE => Some(']'),
            key_code::KEY_BACKSLASH => Some('\\'),
            key_code::KEY_MINUS => Some('-'),
            key_code::KEY_EQUAL => Some('='),
            key_code::KEY_SPACE => Some(' '),
            _ => None,
        }
    }

    fn is_modifier_key(code: u16) -> bool {
        matches!(
            code,
            KEY_LEFTSHIFT
                | KEY_RIGHTSHIFT
                | KEY_LEFTCTRL
                | KEY_RIGHTCTRL
                | KEY_LEFTALT
                | KEY_RIGHTALT
        )
    }

    fn update_modifier_state(&mut self, code: u16, pressed: bool) {
        match code {
            KEY_LEFTSHIFT => self.left_shift_pressed = pressed,
            KEY_RIGHTSHIFT => self.right_shift_pressed = pressed,
            KEY_LEFTCTRL => self.left_control_pressed = pressed,
            KEY_RIGHTCTRL => self.right_control_pressed = pressed,
            KEY_LEFTALT => self.left_alt_pressed = pressed,
            KEY_RIGHTALT => self.right_alt_pressed = pressed,
            _ => {}
        }
    }

    fn reset_transient_modifiers(&mut self) {
        self.left_shift_pressed = false;
        self.right_shift_pressed = false;
        self.left_control_pressed = false;
        self.right_control_pressed = false;
        self.left_alt_pressed = false;
        self.right_alt_pressed = false;
    }

    fn shift_pressed(&self) -> bool {
        self.left_shift_pressed || self.right_shift_pressed
    }

    fn control_pressed(&self) -> bool {
        self.left_control_pressed || self.right_control_pressed
    }

    fn map_key_char_with_modifiers(&self, code: u16) -> Option<char> {
        let base = Self::map_key_char(code)?;
        if self.control_pressed() {
            return match base {
                'a'..='z' => Some((base as u8 - b'a' + 1) as char),
                '[' => Some(0x1b as char),
                '\\' => Some(0x1c as char),
                ']' => Some(0x1d as char),
                '-' | '_' => Some(0x1f as char),
                _ => None,
            };
        }

        if base.is_ascii_lowercase() {
            if self.shift_pressed() {
                return Some(base.to_ascii_uppercase());
            }
            return Some(base);
        }

        if self.shift_pressed() {
            return match base {
                '1' => Some('!'),
                '2' => Some('@'),
                '3' => Some('#'),
                '4' => Some('$'),
                '5' => Some('%'),
                '6' => Some('^'),
                '7' => Some('&'),
                '8' => Some('*'),
                '9' => Some('('),
                '0' => Some(')'),
                '-' => Some('_'),
                '=' => Some('+'),
                '[' => Some('{'),
                ']' => Some('}'),
                '\\' => Some('|'),
                ';' => Some(':'),
                '\'' => Some('"'),
                ',' => Some('<'),
                '.' => Some('>'),
                '/' => Some('?'),
                _ => Some(base),
            };
        }

        Some(base)
    }
}

impl PlatformWindow for SWSPlatformWindow {
    fn new(app_id: &str, title: &str, size: Size) -> Result<Self> {
        // Connect to SWS
        let mut conn = sws::Connection::connect("/tmp/sws.sock")
            .map_err(|_| scarlet_ui_core::error::Error::ConnectionFailed)?;
        let scale_milli = conn
            .get_output_scale()
            .map(Self::sanitize_scale)
            .unwrap_or(DEFAULT_SCALE_MILLI);
        let physical_width =
            Self::logical_to_physical_len_with_scale(size.width.max(1.0) as u32, scale_milli);
        let physical_height =
            Self::logical_to_physical_len_with_scale(size.height.max(1.0) as u32, scale_milli);

        // Create surface
        let surface_id = conn
            .create_surface(app_id, title, "", physical_width, physical_height)
            .map_err(|_| scarlet_ui_core::error::Error::SurfaceCreationFailed)?;

        Ok(Self {
            conn,
            surface_id,
            scale_milli,
            current_size: size,
            pending_events: Vec::new(),
            pending_head: 0,
            pointer_x: 0,
            pointer_y: 0,
            pending_move: false,
            left_shift_pressed: false,
            right_shift_pressed: false,
            left_control_pressed: false,
            right_control_pressed: false,
            left_alt_pressed: false,
            right_alt_pressed: false,
            text_input: None,
            needs_full_present: false,
        })
    }

    fn poll_event(&mut self) -> Option<Event> {
        let debug = scarlet_ui_core::debug::is_enabled();
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
                logln!("[SWSPlatformWindow] poll_event: {:?}", ev);
            }
            Some(ev)
        } else {
            None
        }
    }

    fn output_scale_milli(&self) -> u32 {
        self.scale_milli
    }

    fn present(&mut self, buffer: &Buffer) {
        self.present_with_damage(buffer, None);
    }

    fn present_with_damage(&mut self, buffer: &Buffer, damage: Option<&[DamageRect]>) {
        let damage = if self.needs_full_present {
            None
        } else {
            damage
        };

        if damage.is_some_and(|rects| rects.is_empty()) {
            return;
        }

        // Get the surface and copy pixels
        if let Some(surface) = self.conn.surface_mut(self.surface_id) {
            // Get the shared memory buffer
            surface.with_buffer(|shm_buf, width, height| {
                let full_damage = [(0, 0, width, height)];
                let regions = damage.unwrap_or(&full_damage);
                for region in regions {
                    let _ = Self::copy_buffer_region(buffer, shm_buf, width, height, *region);
                }
            });
        }

        match damage {
            Some(rects) => {
                for rect in rects {
                    let (x, y, width, height) = *rect;
                    if width > 0 && height > 0 {
                        let _ = self
                            .conn
                            .commit_region(self.surface_id, x, y, width, height);
                    }
                }
            }
            None => {
                let _ = self.conn.commit(self.surface_id);
            }
        };

        self.needs_full_present = false;
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
            return Err(scarlet_ui_core::error::Error::InvalidSize { width, height });
        }

        let new_size = Size {
            width: width as f32,
            height: height as f32,
        };

        let physical_width = self.logical_to_physical_len(width);
        let physical_height = self.logical_to_physical_len(height);
        if self.current_size == new_size
            && let Some(surface) = self.conn.surface(self.surface_id)
            && surface.width() == physical_width
            && surface.height() == physical_height
        {
            return Ok(());
        }

        self.conn
            .resize_window(self.surface_id, physical_width, physical_height)
            .map_err(|_| scarlet_ui_core::error::Error::IoError)?;

        self.current_size = new_size;
        self.needs_full_present = true;
        Ok(())
    }

    fn close(&mut self) -> Result<()> {
        // Destroy the surface
        self.conn
            .destroy_surface(self.surface_id)
            .map_err(|_| scarlet_ui_core::error::Error::IoError)?;

        Ok(())
    }

    fn minimize(&mut self) -> Result<()> {
        self.conn
            .minimize_window(self.surface_id)
            .map_err(|_| scarlet_ui_core::error::Error::IoError)
    }

    fn maximize(&mut self) -> Result<()> {
        self.conn
            .maximize_window(self.surface_id)
            .map_err(|_| scarlet_ui_core::error::Error::IoError)
    }

    fn restore(&mut self) -> Result<()> {
        self.conn
            .restore_window(self.surface_id)
            .map_err(|_| scarlet_ui_core::error::Error::IoError)
    }

    fn request_move(&mut self) -> Result<()> {
        self.conn
            .request_move_window(self.surface_id)
            .map_err(|_| scarlet_ui_core::error::Error::IoError)
    }

    fn create_popup(&mut self, position: Point, size: Size) -> Result<u32> {
        // Create a popup window with ALWAYS_ON_TOP type
        let popup_surface_id = self
            .conn
            .create_surface_with_type_and_policies(
                "org.scarlet-os.popup",
                "Popup",
                "",
                self.logical_to_physical_len(size.width.max(1.0) as u32),
                self.logical_to_physical_len(size.height.max(1.0) as u32),
                sws_protocol::window_types::ALWAYS_ON_TOP,
                true,
                true,
                false,
            )
            .map_err(|_| scarlet_ui_core::error::Error::SurfaceCreationFailed)?;

        // Position the popup
        self.conn
            .move_window(
                popup_surface_id,
                self.logical_to_physical_pos(position.x as i32),
                self.logical_to_physical_pos(position.y as i32),
            )
            .map_err(|_| scarlet_ui_core::error::Error::IoError)?;

        Ok(popup_surface_id)
    }

    fn destroy_popup(&mut self, surface_id: u32) -> Result<()> {
        self.conn
            .destroy_surface(surface_id)
            .map_err(|_| scarlet_ui_core::error::Error::IoError)
    }

    fn set_workarea(&mut self, x: i32, y: i32, width: u32, height: u32) -> Result<()> {
        self.conn
            .set_workarea(
                self.logical_to_physical_pos(x),
                self.logical_to_physical_pos(y),
                self.logical_to_physical_len(width),
                self.logical_to_physical_len(height),
            )
            .map_err(|_| scarlet_ui_core::error::Error::IoError)
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
            .map_err(|_| scarlet_ui_core::error::Error::ConnectionFailed)?;
        let scale_milli = conn
            .get_output_scale()
            .map(Self::sanitize_scale)
            .unwrap_or(DEFAULT_SCALE_MILLI);
        let physical_width =
            Self::logical_to_physical_len_with_scale(size.width.max(1.0) as u32, scale_milli);
        let physical_height =
            Self::logical_to_physical_len_with_scale(size.height.max(1.0) as u32, scale_milli);

        let surface_id = conn
            .create_surface_with_type(
                app_id,
                title,
                "",
                physical_width,
                physical_height,
                window_type,
            )
            .map_err(|_| scarlet_ui_core::error::Error::SurfaceCreationFailed)?;

        Ok(Self {
            conn,
            surface_id,
            scale_milli,
            current_size: size,
            pending_events: Vec::new(),
            pending_head: 0,
            pointer_x: 0,
            pointer_y: 0,
            pending_move: false,
            left_shift_pressed: false,
            right_shift_pressed: false,
            left_control_pressed: false,
            right_control_pressed: false,
            left_alt_pressed: false,
            right_alt_pressed: false,
            text_input: None,
            needs_full_present: false,
        })
    }

    fn move_window(&mut self, x: i32, y: i32) -> Result<()> {
        self.conn
            .move_window(
                self.surface_id,
                self.logical_to_physical_pos(x),
                self.logical_to_physical_pos(y),
            )
            .map_err(|_| scarlet_ui_core::error::Error::IoError)
    }

    fn set_window_type(&mut self, surface_id: u32, window_type: u32) -> Result<()> {
        self.conn
            .set_window_type(surface_id, window_type)
            .map_err(|_| scarlet_ui_core::error::Error::IoError)
    }

    fn get_screen_size(&mut self) -> Result<(u32, u32)> {
        let (width, height) = self
            .conn
            .get_screen_size()
            .map_err(|_| scarlet_ui_core::error::Error::IoError)?;
        Ok((
            self.physical_to_logical_len(width),
            self.physical_to_logical_len(height),
        ))
    }

    fn surface_id(&self) -> u32 {
        self.surface_id
    }

    fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
        self
    }

    fn set_resizable(&mut self, resizable: bool) -> Result<()> {
        self.conn
            .set_window_resizable(self.surface_id, resizable)
            .map_err(|_| scarlet_ui_core::error::Error::IoError)?;

        if resizable {
            let _ = self
                .conn
                .set_window_size_limits(self.surface_id, sws::WindowSizeLimits::NONE);
        } else {
            let limits = sws::WindowSizeLimits {
                min_width: self.logical_to_physical_len(self.current_size.width.max(1.0) as u32),
                min_height: self.logical_to_physical_len(self.current_size.height.max(1.0) as u32),
                max_width: self.logical_to_physical_len(self.current_size.width.max(1.0) as u32),
                max_height: self.logical_to_physical_len(self.current_size.height.max(1.0) as u32),
            };
            let _ = self.conn.set_window_size_limits(self.surface_id, limits);
        }

        Ok(())
    }

    fn set_opaque(&mut self, opaque: bool) -> Result<()> {
        self.conn
            .set_window_has_alpha_content(self.surface_id, !opaque)
            .map_err(|_| scarlet_ui_core::error::Error::IoError)
    }

    fn set_menu_titles(&mut self, menu_titles: &str) -> Result<()> {
        self.conn
            .set_window_menu_titles(self.surface_id, menu_titles)
            .map_err(|_| scarlet_ui_core::error::Error::IoError)
    }

    fn sync_text_input(&mut self, state: Option<&TextInputElementState>) {
        SWSPlatformWindow::sync_text_input(self, state);
    }
}

impl SWSPlatformWindow {
    fn handle_sws_event(&mut self, ev: SwsEvent) {
        let debug = scarlet_ui_core::debug::is_enabled();
        if debug {
            logln!("[SWSPlatformWindow] sws_event: {:?}", ev);
        }
        match ev {
            SwsEvent::Input(input) => {
                if debug && input.type_ == event_type::EV_KEY {
                    logln!(
                        "[SWSPlatformWindow] raw key: input_surface={} window_surface={} code={} value={}",
                        input.surface_id,
                        self.surface_id,
                        input.code,
                        input.value
                    );
                }
                if input.surface_id != self.surface_id {
                    if debug && input.type_ == event_type::EV_KEY {
                        logln!(
                            "[SWSPlatformWindow] ignored key for another surface: input_surface={} window_surface={}",
                            input.surface_id,
                            self.surface_id
                        );
                    }
                    return;
                }

                match (input.type_, input.code) {
                    (event_type::EV_ABS, abs_code::ABS_X) => {
                        self.pointer_x = self.physical_to_logical_pos(input.value);
                        self.pending_move = true;
                        if debug {
                            logln!("[SWSPlatformWindow] ABS_X: {}", input.value);
                        }
                    }
                    (event_type::EV_ABS, abs_code::ABS_Y) => {
                        self.pointer_y = self.physical_to_logical_pos(input.value);
                        self.pending_move = true;
                        if debug {
                            logln!("[SWSPlatformWindow] ABS_Y: {}", input.value);
                        }
                    }
                    (event_type::EV_SYN, _) => {
                        if self.pending_move {
                            self.push_event(Event::Mouse(MouseEvent::Moved {
                                x: self.pointer_x,
                                y: self.pointer_y,
                            }));
                            if debug {
                                logln!(
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
                                logln!(
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
                                logln!(
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
                                logln!(
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
                                logln!(
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
                                logln!(
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
                                logln!(
                                    "[SWSPlatformWindow] MouseUp: middle x={}, y={}",
                                    self.pointer_x,
                                    self.pointer_y
                                );
                            }
                        }
                    }
                    (event_type::EV_KEY, code) => {
                        let pressed = input.value != 0;
                        if Self::is_modifier_key(code) {
                            self.update_modifier_state(code, pressed);
                            return;
                        }
                        let mapped_char = self.map_key_char_with_modifiers(code);
                        let mapped = if mapped_char.is_some_and(|c| c.is_control()) {
                            mapped_char.map_or(KeyCode::Unknown, KeyCode::Char)
                        } else {
                            Self::map_key_code(code)
                        };
                        if debug {
                            logln!(
                                "[SWSPlatformWindow] key dispatch: code={} value={} mapped={:?} char={:?}",
                                code,
                                input.value,
                                mapped,
                                mapped_char
                            );
                        }
                        if pressed {
                            self.push_event(Event::Keyboard(KeyEvent::Pressed { keycode: mapped }));
                            if let Some(c) = mapped_char
                                && !c.is_control()
                            {
                                self.push_event(Event::Keyboard(KeyEvent::Char { c }));
                            }
                        } else {
                            self.push_event(Event::Keyboard(KeyEvent::Released {
                                keycode: mapped,
                            }));
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
                    let logical_width = self.physical_to_logical_len(width);
                    let logical_height = self.physical_to_logical_len(height);
                    self.push_event(Event::Resize {
                        width: logical_width,
                        height: logical_height,
                    });
                    if debug {
                        logln!(
                            "[SWSPlatformWindow] SurfaceConfigure: physical={}x{} logical={}x{}",
                            width,
                            height,
                            logical_width,
                            logical_height
                        );
                    }
                }
            }
            SwsEvent::ScreenSizeChanged { width, height } => {
                self.push_event(Event::ScreenSizeChanged {
                    width: self.physical_to_logical_len(width),
                    height: self.physical_to_logical_len(height),
                });
            }
            SwsEvent::OutputScaleChanged { scale_milli } => {
                self.scale_milli = Self::sanitize_scale(scale_milli);
                self.push_event(Event::Resize {
                    width: self.current_size.width.max(1.0) as u32,
                    height: self.current_size.height.max(1.0) as u32,
                });
            }
            SwsEvent::SurfaceDestroyed { surface_id } => {
                if surface_id == self.surface_id {
                    self.push_event(Event::Quit);
                    if debug {
                        logln!("[SWSPlatformWindow] SurfaceDestroyed");
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
            SwsEvent::TextInputPreedit {
                context_id,
                serial,
                cursor_byte,
                anchor_byte,
                text,
                spans,
            } => {
                self.push_event(Event::TextInputPreedit {
                    context_id,
                    serial,
                    cursor_byte,
                    anchor_byte,
                    text,
                    spans,
                });
            }
            SwsEvent::TextInputCommit {
                context_id,
                serial,
                text,
            } => {
                self.push_event(Event::TextInputCommit {
                    context_id,
                    serial,
                    text,
                });
            }
            SwsEvent::TextInputDeleteSurroundingText {
                context_id,
                serial,
                before_bytes,
                after_bytes,
            } => {
                self.push_event(Event::TextInputDeleteSurroundingText {
                    context_id,
                    serial,
                    before_bytes,
                    after_bytes,
                });
            }
            SwsEvent::TextInputDone { context_id, serial } => {
                self.push_event(Event::TextInputDone { context_id, serial });
            }
            SwsEvent::FocusChanged {
                window_id,
                app_id,
                app_name,
                title,
                menu_titles,
            } => {
                let menu_titles = Self::sanitize_menu_titles(&menu_titles);
                if window_id != self.surface_id {
                    self.reset_transient_modifiers();
                }
                // Push FocusChanged event for all windows to receive
                // This allows TaskBar to update its menu based on focus changes
                if debug {
                    logln!(
                        "[SWSPlatformWindow] FocusChanged: window_id={}, app_name={}, menu_titles={}",
                        window_id,
                        app_name,
                        menu_titles
                    );
                }
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
                if debug {
                    logln!(
                        "[SWSPlatformWindow] ActiveAppChanged: window_id={}, app_name={}, menu_titles={}",
                        window_id,
                        app_name,
                        menu_titles
                    );
                }
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
