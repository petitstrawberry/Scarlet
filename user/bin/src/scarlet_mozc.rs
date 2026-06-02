//! Scarlet-native Mozc IME client for SWS.
//!
//! This client keeps Mozc-specific conversion outside SWS. It talks to a Linux
//! `mozc_server` process through Mozc's Unix-domain IPC protocol.

#![no_std]
#![no_main]

extern crate alloc;
extern crate scarlet_std as std;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::time::Duration;
use scarlet_ui::{Buffer, Canvas, Color};
use std::fs::File;
use std::io::{Read, Write};
use std::println;
use std::socket::{ShutdownHow, Socket};
use std::thread;
use sws_client::{Connection, Error, Event, SurfaceBuilder, event_type, key_code};
use sws_protocol::{ime_capabilities, ime_state, ime_status_flags, ime_trigger, window_types};

const IME_NAME: &str = "scarlet-mozc";
const MOZC_IPC_NAME: &str = "session";
const MOZC_PROFILE_DIR: &str = "/scarlet/system/scarlet/root/.config/mozc";
const MOZC_LEGACY_PROFILE_DIR: &str = "/scarlet/system/scarlet/root/.mozc";
const MOZC_HOME_DIR: &str = "/scarlet/system/scarlet/root";
const MODE_DIRECT_ID: u32 = 0;
const MODE_MOZC_HIRAGANA_ID: u32 = 1;
const MODE_MOZC_KATAKANA_ID: u32 = 2;
const MODE_MOZC_ASCII_ID: u32 = 3;
const MODE_DIRECT_LABEL: &str = "Direct";
const MODE_MOZC_HIRAGANA_LABEL: &str = "Mozc Hiragana";
const MODE_MOZC_KATAKANA_LABEL: &str = "Mozc Katakana";
const MODE_MOZC_ASCII_LABEL: &str = "Mozc ASCII";
const KEY_LEFTCTRL: u16 = 0x1d;
const KEY_RIGHTCTRL: u16 = 0x61;
const KEY_LEFTALT: u16 = 0x38;
const KEY_RIGHTALT: u16 = 0x64;
const CANDIDATE_POPUP_WIDTH: u32 = 360;
const CANDIDATE_POPUP_HEIGHT: u32 = 148;
const CANDIDATE_POPUP_PAGE_SIZE: usize = 5;
const CANDIDATE_POPUP_OFFSET_X: i32 = 0;
const CANDIDATE_POPUP_OFFSET_Y: i32 = 4;
const CANDIDATE_POPUP_PADDING_X: i32 = 4;
const CANDIDATE_POPUP_PADDING_Y: i32 = 4;
const CANDIDATE_ROW_HEIGHT: i32 = 28;

struct ScarletMozc {
    active_context_id: Option<u32>,
    grabbing: bool,
    ipc: MozcIpc,
    session_id: Option<u64>,
    preedit: String,
    active_mode: u32,
    left_shift_down: bool,
    right_shift_down: bool,
    left_ctrl_down: bool,
    right_ctrl_down: bool,
    left_alt_down: bool,
    right_alt_down: bool,
    eaten_keys: Vec<u16>,
    candidate_popup: Option<CandidatePopup>,
    scale_milli: u32,
}

impl ScarletMozc {
    fn new(scale_milli: u32) -> Self {
        Self {
            active_context_id: None,
            grabbing: false,
            ipc: MozcIpc::new(MOZC_IPC_NAME),
            session_id: None,
            preedit: String::new(),
            active_mode: proto::COMPOSITION_HIRAGANA,
            left_shift_down: false,
            right_shift_down: false,
            left_ctrl_down: false,
            right_ctrl_down: false,
            left_alt_down: false,
            right_alt_down: false,
            eaten_keys: Vec::new(),
            candidate_popup: None,
            scale_milli: scale_milli.max(1),
        }
    }

    fn reset_context(&mut self) {
        self.preedit.clear();
        self.eaten_keys.clear();
        self.session_id = None;
    }

    fn handle_event(&mut self, conn: &mut Connection, event: Event) -> Result<(), Error> {
        match event {
            Event::ImeActivate(state) => {
                self.reset_context();
                self.active_context_id = Some(state.context_id);
                self.grabbing = false;
                conn.ime_set_preedit(state.context_id, 0, 0, "", &[])?;
                self.close_candidate_popup(conn)?;
                self.emit_status(conn, state.context_id)?;
                println!(
                    "[scarlet_mozc] activated context={} window={}",
                    state.context_id, state.window_id
                );
            }
            Event::ImeDeactivate { context_id, .. } => {
                if self.active_context_id == Some(context_id) {
                    conn.ime_set_preedit(context_id, 0, 0, "", &[])?;
                    self.close_candidate_popup(conn)?;
                    self.active_context_id = None;
                    self.grabbing = false;
                    self.reset_context();
                }
                println!("[scarlet_mozc] deactivated context={}", context_id);
            }
            Event::ImeReset { context_id, .. } => {
                if self.active_context_id == Some(context_id) {
                    println!("[scarlet_mozc] reset context={}", context_id);
                    self.grabbing = false;
                    self.reset_context();
                    conn.ime_set_preedit(context_id, 0, 0, "", &[])?;
                    self.close_candidate_popup(conn)?;
                    self.emit_status(conn, context_id)?;
                }
            }
            Event::ImeTrigger {
                context_id,
                trigger_id,
                code,
                serial,
                ..
            } => {
                println!(
                    "[scarlet_mozc] trigger context={} serial={} trigger={} code={} grabbing={}",
                    context_id, serial, trigger_id, code, self.grabbing
                );
                if trigger_id == ime_trigger::TOGGLE {
                    self.toggle_keyboard_grab(conn, context_id)?;
                }
            }
            Event::ImeKeyEvent {
                context_id,
                key_serial,
                type_,
                code,
                value,
                time,
                ..
            } => {
                println!(
                    "[scarlet_mozc] key-event context={} serial={} {}({}) type={} value={} grabbing={} preedit='{}'",
                    context_id,
                    key_serial,
                    key_name(code),
                    code,
                    type_,
                    value,
                    self.grabbing,
                    self.preedit
                );
                let handled = self.handle_key(conn, context_id, type_, code, value, time)?;
                conn.ime_key_handled(key_serial, handled)?;
                println!(
                    "[scarlet_mozc] key-handled serial={} handled={} preedit='{}'",
                    key_serial, handled, self.preedit
                );
            }
            Event::SurfaceDestroyed { surface_id } => {
                if self
                    .candidate_popup
                    .is_some_and(|popup| popup.window_id == surface_id)
                {
                    println!(
                        "[scarlet_mozc] candidate popup destroyed window={}",
                        surface_id
                    );
                    self.candidate_popup = None;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn toggle_keyboard_grab(
        &mut self,
        conn: &mut Connection,
        context_id: u32,
    ) -> Result<(), Error> {
        if self.grabbing {
            self.grabbing = false;
            self.reset_context();
            conn.ime_set_preedit(context_id, 0, 0, "", &[])?;
            self.close_candidate_popup(conn)?;
            conn.ime_release_keyboard(context_id)?;
            self.emit_status(conn, context_id)?;
            println!("[scarlet_mozc] released keyboard context={}", context_id);
            return Ok(());
        }

        match self.ensure_session() {
            Ok(()) => {
                self.grabbing = true;
                conn.ime_grab_keyboard(context_id)?;
                self.emit_status(conn, context_id)?;
                println!("[scarlet_mozc] grabbed keyboard context={}", context_id);
            }
            Err(err) => {
                println!("[scarlet_mozc] cannot start Mozc session: {:?}", err);
                self.grabbing = false;
                self.emit_status(conn, context_id)?;
            }
        }
        Ok(())
    }

    fn ensure_session(&mut self) -> core::result::Result<(), MozcError> {
        if self.session_id.is_some() {
            return Ok(());
        }
        let output = self.ipc.call(&proto::encode_create_session())?;
        if let Some(session_id) = output.session_id {
            self.session_id = Some(session_id);
            self.active_mode = output.mode.unwrap_or(proto::COMPOSITION_HIRAGANA);
            println!(
                "[scarlet_mozc] created Mozc session id={} mode={}",
                session_id, self.active_mode
            );
            return Ok(());
        }
        Err(MozcError::InvalidResponse)
    }

    fn handle_key(
        &mut self,
        conn: &mut Connection,
        context_id: u32,
        type_: u16,
        code: u16,
        value: i32,
        time: u64,
    ) -> Result<bool, Error> {
        if !self.grabbing || self.active_context_id != Some(context_id) {
            return Ok(false);
        }
        if type_ != event_type::EV_KEY {
            return Ok(false);
        }

        if self.update_modifier(code, value != 0) {
            return Ok(false);
        }

        if value == 0 {
            return Ok(self.remove_eaten_key(code));
        }

        let Some(session_id) = self.session_id else {
            println!("[scarlet_mozc] no Mozc session");
            return Ok(false);
        };
        let Some(key) = self.translate_key(code, time) else {
            println!("[scarlet_mozc] pass-through unsupported key {}", code);
            return Ok(false);
        };

        let request = proto::encode_send_key(session_id, &key);
        match self.ipc.call(&request) {
            Ok(output) => {
                let handled = output
                    .consumed
                    .unwrap_or_else(|| output.has_visible_update());
                println!(
                    "[scarlet_mozc] output consumed={:?} handled={}",
                    output.consumed, handled
                );
                self.apply_output(conn, context_id, output)?;
                if handled {
                    self.eat_key(code);
                }
                Ok(handled)
            }
            Err(err) => {
                println!("[scarlet_mozc] Mozc SEND_KEY failed: {:?}", err);
                if !self.preedit.is_empty() {
                    self.eat_key(code);
                    return Ok(true);
                }
                Ok(false)
            }
        }
    }

    fn apply_output(
        &mut self,
        conn: &mut Connection,
        context_id: u32,
        output: MozcOutput,
    ) -> Result<(), Error> {
        if let Some(mode) = output.mode {
            self.active_mode = mode;
        }
        if let Some(status) = output.status {
            if let Some(mode) = status.mode {
                self.active_mode = mode;
            }
            if status.activated == Some(false) {
                self.active_mode = proto::COMPOSITION_DIRECT;
            }
        }
        if let Some(commit) = output.result {
            if !commit.is_empty() {
                println!("[scarlet_mozc] commit '{}'", commit);
                conn.ime_commit_text(context_id, &commit)?;
            }
        }
        if let Some(preedit) = output.preedit {
            self.preedit = preedit.text;
            let cursor_byte = char_to_byte_index(&self.preedit, preedit.cursor_chars);
            println!(
                "[scarlet_mozc] preedit '{}' cursor_byte={}",
                self.preedit, cursor_byte
            );
            conn.ime_set_preedit(
                context_id,
                cursor_byte as u32,
                cursor_byte as u32,
                &self.preedit,
                &preedit_spans(&self.preedit),
            )?;
        } else if !self.preedit.is_empty() {
            self.preedit.clear();
            println!("[scarlet_mozc] preedit ''");
            conn.ime_set_preedit(context_id, 0, 0, "", &[])?;
        }
        self.sync_candidate_popup(conn, context_id, output.candidate_window.as_ref())?;
        self.emit_status(conn, context_id)
    }

    fn ensure_candidate_popup(&mut self, conn: &mut Connection) -> Result<u32, Error> {
        if let Some(popup) = self.candidate_popup {
            return Ok(popup.window_id);
        }

        let window_id = SurfaceBuilder::new()
            .app_id("org.scarlet.mozc.candidates")
            .app_name("Mozc Candidates")
            .menu_titles("")
            .size(
                scale_len(CANDIDATE_POPUP_WIDTH, self.scale_milli),
                scale_len(CANDIDATE_POPUP_HEIGHT, self.scale_milli),
            )
            .window_type(window_types::IME_POPUP)
            .resizable(false)
            .focus_on_create(false)
            .active_on_focus(false)
            .position(0, 0)
            .build(conn)?;
        self.candidate_popup = Some(CandidatePopup { window_id });
        println!(
            "[scarlet_mozc] created candidate popup window={}",
            window_id
        );
        Ok(window_id)
    }

    fn close_candidate_popup(&mut self, conn: &mut Connection) -> Result<(), Error> {
        let Some(popup) = self.candidate_popup.take() else {
            return Ok(());
        };

        match conn.destroy_surface(popup.window_id) {
            Ok(()) | Err(Error::SurfaceNotFound) => {
                println!(
                    "[scarlet_mozc] candidate popup closed window={}",
                    popup.window_id
                );
                Ok(())
            }
            Err(err) => Err(err),
        }
    }

    fn sync_candidate_popup(
        &mut self,
        conn: &mut Connection,
        context_id: u32,
        candidate_window: Option<&MozcCandidateWindow>,
    ) -> Result<(), Error> {
        let Some(candidate_window) = candidate_window else {
            return self.close_candidate_popup(conn);
        };
        if candidate_window.focused_index.is_none() || candidate_window.candidates.is_empty() {
            return self.close_candidate_popup(conn);
        }

        let rows = candidate_popup_rows(candidate_window);
        if rows.is_empty() {
            return self.close_candidate_popup(conn);
        }

        let window_id = self.ensure_candidate_popup(conn)?;
        draw_candidate_popup(conn, window_id, &rows, self.scale_milli)?;
        conn.ime_set_popup_window(
            context_id,
            window_id,
            CANDIDATE_POPUP_OFFSET_X,
            CANDIDATE_POPUP_OFFSET_Y,
            true,
        )?;
        self.candidate_popup = Some(CandidatePopup { window_id });
        println!(
            "[scarlet_mozc] candidate popup shown context={} window={} focused={:?} size={}",
            context_id, window_id, candidate_window.focused_index, candidate_window.size
        );
        Ok(())
    }

    fn emit_status(&self, conn: &mut Connection, context_id: u32) -> Result<(), Error> {
        let (state, mode_id, label) = if self.grabbing {
            match self.active_mode {
                proto::COMPOSITION_FULL_KATAKANA => (
                    ime_state::COMPOSING,
                    MODE_MOZC_KATAKANA_ID,
                    MODE_MOZC_KATAKANA_LABEL,
                ),
                proto::COMPOSITION_HALF_ASCII | proto::COMPOSITION_FULL_ASCII => (
                    ime_state::COMPOSING,
                    MODE_MOZC_ASCII_ID,
                    MODE_MOZC_ASCII_LABEL,
                ),
                _ => (
                    ime_state::COMPOSING,
                    MODE_MOZC_HIRAGANA_ID,
                    MODE_MOZC_HIRAGANA_LABEL,
                ),
            }
        } else {
            (ime_state::DIRECT, MODE_DIRECT_ID, MODE_DIRECT_LABEL)
        };
        let flags = if self.grabbing {
            ime_status_flags::MODE_ACTIVE
        } else {
            0
        };
        conn.ime_set_status(context_id, state, mode_id, flags, label)
    }

    fn translate_key(&self, code: u16, time: u64) -> Option<proto::KeyEvent> {
        let key_code = printable_key_code(code, self.shift_down());
        let special_key = self.special_key(code);
        let key_is_printable = key_code.is_some();
        let mut key = proto::KeyEvent {
            key_code,
            special_key,
            modifier_keys: self.modifier_keys(!key_is_printable),
            activated: Some(true),
            mode: Some(proto::COMPOSITION_HIRAGANA),
            timestamp_msec: Some((time / 1_000_000) as i64),
        };
        if key.key_code.is_none() && key.special_key.is_none() {
            return None;
        }
        if key.key_code.is_some() && key.special_key.is_some() {
            key.special_key = None;
        }
        Some(key)
    }

    fn special_key(&self, code: u16) -> Option<u32> {
        if code == key_code::KEY_SPACE && !self.preedit.is_empty() {
            return Some(proto::SPECIAL_HENKAN);
        }
        special_key(code)
    }

    fn shift_down(&self) -> bool {
        self.left_shift_down || self.right_shift_down
    }

    fn modifier_keys(&self, include_shift: bool) -> Vec<u32> {
        let mut modifiers = Vec::new();
        if self.left_ctrl_down || self.right_ctrl_down {
            modifiers.push(proto::MODIFIER_CTRL);
        }
        if self.left_alt_down || self.right_alt_down {
            modifiers.push(proto::MODIFIER_ALT);
        }
        if include_shift && self.shift_down() {
            modifiers.push(proto::MODIFIER_SHIFT);
        }
        if self.left_ctrl_down {
            modifiers.push(proto::MODIFIER_LEFT_CTRL);
        }
        if self.right_ctrl_down {
            modifiers.push(proto::MODIFIER_RIGHT_CTRL);
        }
        if self.left_alt_down {
            modifiers.push(proto::MODIFIER_LEFT_ALT);
        }
        if self.right_alt_down {
            modifiers.push(proto::MODIFIER_RIGHT_ALT);
        }
        if include_shift && self.left_shift_down {
            modifiers.push(proto::MODIFIER_LEFT_SHIFT);
        }
        if include_shift && self.right_shift_down {
            modifiers.push(proto::MODIFIER_RIGHT_SHIFT);
        }
        modifiers
    }

    fn update_modifier(&mut self, code: u16, pressed: bool) -> bool {
        match code {
            key_code::KEY_LEFTSHIFT => self.left_shift_down = pressed,
            key_code::KEY_RIGHTSHIFT => self.right_shift_down = pressed,
            KEY_LEFTCTRL => self.left_ctrl_down = pressed,
            KEY_RIGHTCTRL => self.right_ctrl_down = pressed,
            KEY_LEFTALT => self.left_alt_down = pressed,
            KEY_RIGHTALT => self.right_alt_down = pressed,
            _ => return false,
        }
        true
    }

    fn eat_key(&mut self, code: u16) {
        if !self.eaten_keys.contains(&code) {
            self.eaten_keys.push(code);
        }
    }

    fn remove_eaten_key(&mut self, code: u16) -> bool {
        if let Some(index) = self.eaten_keys.iter().position(|eaten| *eaten == code) {
            self.eaten_keys.remove(index);
            true
        } else {
            false
        }
    }
}

#[derive(Clone, Copy)]
struct CandidatePopup {
    window_id: u32,
}

struct CandidatePopupRow {
    index: usize,
    text: String,
    selected: bool,
}

#[derive(Default)]
struct MozcCandidateWindow {
    focused_index: Option<usize>,
    size: u32,
    candidates: Vec<MozcCandidate>,
}

struct MozcCandidate {
    index: usize,
    value: String,
}

fn candidate_popup_rows(candidate_window: &MozcCandidateWindow) -> Vec<CandidatePopupRow> {
    let Some(focused_index) = candidate_window.focused_index else {
        return Vec::new();
    };
    let page_start =
        (focused_index / CANDIDATE_POPUP_PAGE_SIZE).saturating_mul(CANDIDATE_POPUP_PAGE_SIZE);
    let mut rows = Vec::new();
    for candidate in candidate_window
        .candidates
        .iter()
        .filter(|candidate| candidate.index >= page_start)
        .take(CANDIDATE_POPUP_PAGE_SIZE)
    {
        rows.push(CandidatePopupRow {
            index: candidate.index,
            text: candidate.value.clone(),
            selected: candidate.index == focused_index,
        });
    }
    rows
}

fn draw_candidate_popup(
    conn: &mut Connection,
    window_id: u32,
    rows: &[CandidatePopupRow],
    scale_milli: u32,
) -> Result<(), Error> {
    let mut ui_buffer = Buffer::from_logical_dimensions_with_scale(
        CANDIDATE_POPUP_WIDTH,
        CANDIDATE_POPUP_HEIGHT,
        scale_milli,
    );
    {
        let mut canvas = Canvas::for_buffer(&mut ui_buffer);
        draw_candidate_popup_ui(&mut canvas, rows);
    }

    let Some(surface) = conn.surface_mut(window_id) else {
        return Err(Error::SurfaceNotFound);
    };

    surface.with_buffer(|buffer, width, height| {
        for byte in buffer.iter_mut() {
            *byte = 0;
        }

        let src = ui_buffer.data();
        let src_stride = ui_buffer.width() as usize * 4;
        let dst_stride = width as usize * 4;
        let copy_stride = src_stride.min(dst_stride);
        let copy_rows = ui_buffer.height().min(height) as usize;
        for row in 0..copy_rows {
            let src_start = row * src_stride;
            let dst_start = row * dst_stride;
            buffer[dst_start..dst_start + copy_stride]
                .copy_from_slice(&src[src_start..src_start + copy_stride]);
        }
    });
    conn.commit(window_id)
}

fn draw_candidate_popup_ui(canvas: &mut Canvas<'_>, rows: &[CandidatePopupRow]) {
    let bg = Color::rgb(250u8, 250u8, 252u8);
    let border = Color::rgb(106u8, 112u8, 124u8);
    let text = Color::rgb(24u8, 26u8, 32u8);
    let selected_bg = Color::rgb(40u8, 96u8, 180u8);
    let selected_text = Color::WHITE;

    canvas.fill_rect(0, 0, CANDIDATE_POPUP_WIDTH, CANDIDATE_POPUP_HEIGHT, bg);
    canvas.draw_rect(0, 0, CANDIDATE_POPUP_WIDTH, CANDIDATE_POPUP_HEIGHT, border);

    for (row_pos, row) in rows.iter().enumerate() {
        let y = CANDIDATE_POPUP_PADDING_Y + (row_pos as i32 * CANDIDATE_ROW_HEIGHT);
        let row_h = CANDIDATE_ROW_HEIGHT as u32;
        if row.selected {
            canvas.fill_rect(
                CANDIDATE_POPUP_PADDING_X,
                y,
                CANDIDATE_POPUP_WIDTH.saturating_sub((CANDIDATE_POPUP_PADDING_X * 2) as u32),
                row_h,
                selected_bg,
            );
        }

        let row_text = format!("{}  {}", row.index.saturating_add(1), row.text);
        let color = if row.selected { selected_text } else { text };
        canvas.draw_text_sized(
            CANDIDATE_POPUP_PADDING_X + 10,
            y + 6,
            &row_text,
            color,
            17.0,
        );
    }
}

fn scale_len(value: u32, scale_milli: u32) -> u32 {
    ((value as u64)
        .saturating_mul(scale_milli.max(1) as u64)
        .saturating_add(999)
        / 1000)
        .max(1) as u32
}

struct MozcIpc {
    server_name: &'static str,
    cached_abstract_name: Option<String>,
}

impl MozcIpc {
    fn new(server_name: &'static str) -> Self {
        Self {
            server_name,
            cached_abstract_name: None,
        }
    }

    fn call(&mut self, request: &[u8]) -> core::result::Result<MozcOutput, MozcError> {
        let abstract_name = self.abstract_socket_name()?;
        let mut socket = Socket::new().map_err(|_| MozcError::Socket)?;
        socket
            .connect_abstract(&abstract_name)
            .map_err(|_| MozcError::Connect)?;
        socket_write_all(&mut socket, request).map_err(|_| MozcError::Write)?;
        socket
            .shutdown(ShutdownHow::Write)
            .map_err(|_| MozcError::Shutdown)?;

        let mut response = Vec::new();
        let mut buffer = [0u8; 4096];
        loop {
            match socket.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    response.extend_from_slice(&buffer[..n]);
                    if response.len() > 1024 * 1024 {
                        return Err(MozcError::ResponseTooLarge);
                    }
                }
                Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(_) => return Err(MozcError::Read),
            }
        }
        println!("[scarlet_mozc] Mozc IPC response bytes={}", response.len());
        proto::decode_command_output(&response).ok_or(MozcError::InvalidResponse)
    }

    fn abstract_socket_name(&mut self) -> core::result::Result<String, MozcError> {
        if let Some(name) = &self.cached_abstract_name {
            return Ok(name.clone());
        }
        let key = load_mozc_ipc_key(self.server_name)?;
        let name = format!("tmp/.mozc.{}.{}", key, self.server_name);
        println!("[scarlet_mozc] Mozc abstract socket '{}'", name);
        self.cached_abstract_name = Some(name.clone());
        Ok(name)
    }
}

#[derive(Debug)]
enum MozcError {
    KeyFile,
    InvalidKeyFile,
    Socket,
    Connect,
    Write,
    Shutdown,
    Read,
    ResponseTooLarge,
    InvalidResponse,
}

#[derive(Default)]
struct MozcOutput {
    session_id: Option<u64>,
    consumed: Option<bool>,
    result: Option<String>,
    preedit: Option<MozcPreedit>,
    candidate_window: Option<MozcCandidateWindow>,
    status: Option<MozcStatus>,
    mode: Option<u32>,
}

impl MozcOutput {
    fn has_visible_update(&self) -> bool {
        self.result.as_ref().is_some_and(|text| !text.is_empty()) || self.preedit.is_some()
    }
}

struct MozcPreedit {
    text: String,
    cursor_chars: usize,
}

#[derive(Default)]
struct MozcStatus {
    activated: Option<bool>,
    mode: Option<u32>,
}

fn load_mozc_ipc_key(server_name: &str) -> core::result::Result<String, MozcError> {
    let paths = [
        format!("{}/.{}.ipc", MOZC_PROFILE_DIR, server_name),
        format!("{}/.{}.ipc", MOZC_LEGACY_PROFILE_DIR, server_name),
        format!("{}/.{}.ipc", MOZC_HOME_DIR, server_name),
    ];

    let mut file = None;
    let mut loaded_path = "";
    for path in &paths {
        match File::open(path) {
            Ok(opened) => {
                loaded_path = path;
                file = Some(opened);
                break;
            }
            Err(err) => {
                println!(
                    "[scarlet_mozc] IPC key file not available '{}': {:?}",
                    path, err
                );
            }
        }
    }
    let mut file = file.ok_or(MozcError::KeyFile)?;

    let mut data = Vec::new();
    let mut buffer = [0u8; 512];
    loop {
        let n = file.read(&mut buffer).map_err(|_| MozcError::KeyFile)?;
        if n == 0 {
            break;
        }
        data.extend_from_slice(&buffer[..n]);
        if data.len() > 4096 {
            return Err(MozcError::InvalidKeyFile);
        }
    }
    println!(
        "[scarlet_mozc] IPC key file '{}' bytes={}",
        loaded_path,
        data.len()
    );
    let decoded_key = proto::decode_ipc_key(&data);
    if let Some(key) = &decoded_key {
        println!("[scarlet_mozc] decoded IPC key '{}'", key);
    }
    let key = match decoded_key {
        Some(key) if is_mozc_ipc_key(&key) => key,
        _ => {
            let key = proto::find_ascii_hex_key(&data).ok_or(MozcError::InvalidKeyFile)?;
            println!("[scarlet_mozc] recovered IPC key '{}'", key);
            key
        }
    };
    println!("[scarlet_mozc] loaded IPC key file '{}'", loaded_path);
    Ok(key)
}

fn is_mozc_ipc_key(key: &str) -> bool {
    key.len() == 32
        && key
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn socket_write_all(socket: &mut Socket, mut data: &[u8]) -> std::io::Result<()> {
    while !data.is_empty() {
        let written = socket.write(data)?;
        if written == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WriteZero,
                "socket write returned zero",
            ));
        }
        data = &data[written..];
    }
    Ok(())
}

fn preedit_spans(text: &str) -> Vec<u8> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut spans = Vec::new();
    spans.extend_from_slice(&0u32.to_le_bytes());
    spans.extend_from_slice(&(text.len() as u32).to_le_bytes());
    spans.extend_from_slice(&sws_protocol::preedit_style::UNDERLINE.to_le_bytes());
    spans
}

fn char_to_byte_index(text: &str, char_index: usize) -> usize {
    text.char_indices()
        .map(|(byte, _)| byte)
        .nth(char_index)
        .unwrap_or(text.len())
}

fn printable_key_code(code: u16, shifted: bool) -> Option<u32> {
    let c = match code {
        key_code::KEY_A => letter('a', shifted),
        key_code::KEY_B => letter('b', shifted),
        key_code::KEY_C => letter('c', shifted),
        key_code::KEY_D => letter('d', shifted),
        key_code::KEY_E => letter('e', shifted),
        key_code::KEY_F => letter('f', shifted),
        key_code::KEY_G => letter('g', shifted),
        key_code::KEY_H => letter('h', shifted),
        key_code::KEY_I => letter('i', shifted),
        key_code::KEY_J => letter('j', shifted),
        key_code::KEY_K => letter('k', shifted),
        key_code::KEY_L => letter('l', shifted),
        key_code::KEY_M => letter('m', shifted),
        key_code::KEY_N => letter('n', shifted),
        key_code::KEY_O => letter('o', shifted),
        key_code::KEY_P => letter('p', shifted),
        key_code::KEY_Q => letter('q', shifted),
        key_code::KEY_R => letter('r', shifted),
        key_code::KEY_S => letter('s', shifted),
        key_code::KEY_T => letter('t', shifted),
        key_code::KEY_U => letter('u', shifted),
        key_code::KEY_V => letter('v', shifted),
        key_code::KEY_W => letter('w', shifted),
        key_code::KEY_X => letter('x', shifted),
        key_code::KEY_Y => letter('y', shifted),
        key_code::KEY_Z => letter('z', shifted),
        key_code::KEY_1 => {
            if shifted {
                '!'
            } else {
                '1'
            }
        }
        key_code::KEY_2 => {
            if shifted {
                '@'
            } else {
                '2'
            }
        }
        key_code::KEY_3 => {
            if shifted {
                '#'
            } else {
                '3'
            }
        }
        key_code::KEY_4 => {
            if shifted {
                '$'
            } else {
                '4'
            }
        }
        key_code::KEY_5 => {
            if shifted {
                '%'
            } else {
                '5'
            }
        }
        key_code::KEY_6 => {
            if shifted {
                '^'
            } else {
                '6'
            }
        }
        key_code::KEY_7 => {
            if shifted {
                '&'
            } else {
                '7'
            }
        }
        key_code::KEY_8 => {
            if shifted {
                '*'
            } else {
                '8'
            }
        }
        key_code::KEY_9 => {
            if shifted {
                '('
            } else {
                '9'
            }
        }
        key_code::KEY_0 => {
            if shifted {
                ')'
            } else {
                '0'
            }
        }
        key_code::KEY_MINUS => {
            if shifted {
                '_'
            } else {
                '-'
            }
        }
        key_code::KEY_EQUAL => {
            if shifted {
                '+'
            } else {
                '='
            }
        }
        key_code::KEY_LEFTBRACE => {
            if shifted {
                '{'
            } else {
                '['
            }
        }
        key_code::KEY_RIGHTBRACE => {
            if shifted {
                '}'
            } else {
                ']'
            }
        }
        key_code::KEY_SEMICOLON => {
            if shifted {
                ':'
            } else {
                ';'
            }
        }
        key_code::KEY_APOSTROPHE => {
            if shifted {
                '"'
            } else {
                '\''
            }
        }
        key_code::KEY_COMMA => {
            if shifted {
                '<'
            } else {
                ','
            }
        }
        key_code::KEY_DOT => {
            if shifted {
                '>'
            } else {
                '.'
            }
        }
        key_code::KEY_SLASH => {
            if shifted {
                '?'
            } else {
                '/'
            }
        }
        key_code::KEY_BACKSLASH => {
            if shifted {
                '|'
            } else {
                '\\'
            }
        }
        _ => return None,
    };
    Some(c as u32)
}

fn letter(c: char, shifted: bool) -> char {
    if shifted {
        ((c as u8) - b'a' + b'A') as char
    } else {
        c
    }
}

fn special_key(code: u16) -> Option<u32> {
    match code {
        key_code::KEY_SPACE => Some(proto::SPECIAL_SPACE),
        key_code::KEY_ENTER => Some(proto::SPECIAL_ENTER),
        key_code::KEY_LEFT => Some(proto::SPECIAL_LEFT),
        key_code::KEY_RIGHT => Some(proto::SPECIAL_RIGHT),
        key_code::KEY_UP => Some(proto::SPECIAL_UP),
        key_code::KEY_DOWN => Some(proto::SPECIAL_DOWN),
        key_code::KEY_ESC => Some(proto::SPECIAL_ESCAPE),
        key_code::KEY_DELETE => Some(proto::SPECIAL_DELETE),
        key_code::KEY_BACKSPACE => Some(proto::SPECIAL_BACKSPACE),
        _ => None,
    }
}

fn key_name(code: u16) -> &'static str {
    match code {
        key_code::KEY_ESC => "KEY_ESC",
        key_code::KEY_ENTER => "KEY_ENTER",
        key_code::KEY_BACKSPACE => "KEY_BACKSPACE",
        key_code::KEY_SPACE => "KEY_SPACE",
        key_code::KEY_LEFTSHIFT => "KEY_LEFTSHIFT",
        key_code::KEY_RIGHTSHIFT => "KEY_RIGHTSHIFT",
        KEY_LEFTCTRL => "KEY_LEFTCTRL",
        KEY_RIGHTCTRL => "KEY_RIGHTCTRL",
        KEY_LEFTALT => "KEY_LEFTALT",
        KEY_RIGHTALT => "KEY_RIGHTALT",
        key_code::KEY_DELETE => "KEY_DELETE",
        key_code::KEY_LEFT => "KEY_LEFT",
        key_code::KEY_RIGHT => "KEY_RIGHT",
        key_code::KEY_UP => "KEY_UP",
        key_code::KEY_DOWN => "KEY_DOWN",
        key_code::KEY_A => "KEY_A",
        key_code::KEY_B => "KEY_B",
        key_code::KEY_C => "KEY_C",
        key_code::KEY_D => "KEY_D",
        key_code::KEY_E => "KEY_E",
        key_code::KEY_F => "KEY_F",
        key_code::KEY_G => "KEY_G",
        key_code::KEY_H => "KEY_H",
        key_code::KEY_I => "KEY_I",
        key_code::KEY_J => "KEY_J",
        key_code::KEY_K => "KEY_K",
        key_code::KEY_L => "KEY_L",
        key_code::KEY_M => "KEY_M",
        key_code::KEY_N => "KEY_N",
        key_code::KEY_O => "KEY_O",
        key_code::KEY_P => "KEY_P",
        key_code::KEY_Q => "KEY_Q",
        key_code::KEY_R => "KEY_R",
        key_code::KEY_S => "KEY_S",
        key_code::KEY_T => "KEY_T",
        key_code::KEY_U => "KEY_U",
        key_code::KEY_V => "KEY_V",
        key_code::KEY_W => "KEY_W",
        key_code::KEY_X => "KEY_X",
        key_code::KEY_Y => "KEY_Y",
        key_code::KEY_Z => "KEY_Z",
        key_code::KEY_1 => "KEY_1",
        key_code::KEY_2 => "KEY_2",
        key_code::KEY_3 => "KEY_3",
        key_code::KEY_4 => "KEY_4",
        key_code::KEY_5 => "KEY_5",
        key_code::KEY_6 => "KEY_6",
        key_code::KEY_7 => "KEY_7",
        key_code::KEY_8 => "KEY_8",
        key_code::KEY_9 => "KEY_9",
        key_code::KEY_0 => "KEY_0",
        key_code::KEY_MINUS => "KEY_MINUS",
        key_code::KEY_EQUAL => "KEY_EQUAL",
        key_code::KEY_LEFTBRACE => "KEY_LEFTBRACE",
        key_code::KEY_RIGHTBRACE => "KEY_RIGHTBRACE",
        key_code::KEY_SEMICOLON => "KEY_SEMICOLON",
        key_code::KEY_APOSTROPHE => "KEY_APOSTROPHE",
        key_code::KEY_COMMA => "KEY_COMMA",
        key_code::KEY_DOT => "KEY_DOT",
        key_code::KEY_SLASH => "KEY_SLASH",
        key_code::KEY_BACKSLASH => "KEY_BACKSLASH",
        _ => "KEY_UNKNOWN",
    }
}

mod proto {
    use super::{MozcCandidate, MozcCandidateWindow, MozcOutput, MozcPreedit, MozcStatus};
    use alloc::string::{String, ToString};
    use alloc::vec::Vec;

    pub const INPUT_CREATE_SESSION: u64 = 1;
    pub const INPUT_SEND_KEY: u64 = 3;

    pub const COMPOSITION_DIRECT: u32 = 0;
    pub const COMPOSITION_HIRAGANA: u32 = 1;
    pub const COMPOSITION_FULL_KATAKANA: u32 = 2;
    pub const COMPOSITION_HALF_ASCII: u32 = 3;
    pub const COMPOSITION_FULL_ASCII: u32 = 4;

    pub const SPECIAL_SPACE: u32 = 4;
    pub const SPECIAL_ENTER: u32 = 5;
    pub const SPECIAL_LEFT: u32 = 6;
    pub const SPECIAL_RIGHT: u32 = 7;
    pub const SPECIAL_UP: u32 = 8;
    pub const SPECIAL_DOWN: u32 = 9;
    pub const SPECIAL_ESCAPE: u32 = 10;
    pub const SPECIAL_DELETE: u32 = 11;
    pub const SPECIAL_BACKSPACE: u32 = 12;
    pub const SPECIAL_HENKAN: u32 = 13;

    pub const MODIFIER_CTRL: u32 = 1;
    pub const MODIFIER_ALT: u32 = 2;
    pub const MODIFIER_SHIFT: u32 = 4;
    pub const MODIFIER_LEFT_CTRL: u32 = 32;
    pub const MODIFIER_LEFT_ALT: u32 = 64;
    pub const MODIFIER_LEFT_SHIFT: u32 = 128;
    pub const MODIFIER_RIGHT_CTRL: u32 = 256;
    pub const MODIFIER_RIGHT_ALT: u32 = 512;
    pub const MODIFIER_RIGHT_SHIFT: u32 = 1024;

    pub struct KeyEvent {
        pub key_code: Option<u32>,
        pub special_key: Option<u32>,
        pub modifier_keys: Vec<u32>,
        pub activated: Option<bool>,
        pub mode: Option<u32>,
        pub timestamp_msec: Option<i64>,
    }

    pub fn encode_create_session() -> Vec<u8> {
        let mut input = Vec::new();
        push_varint_field(&mut input, 1, INPUT_CREATE_SESSION);
        input
    }

    pub fn encode_send_key(session_id: u64, key: &KeyEvent) -> Vec<u8> {
        let mut key_message = Vec::new();
        if let Some(key_code) = key.key_code {
            push_varint_field(&mut key_message, 1, key_code as u64);
        }
        if let Some(special_key) = key.special_key {
            push_varint_field(&mut key_message, 3, special_key as u64);
        }
        for modifier in &key.modifier_keys {
            push_varint_field(&mut key_message, 4, *modifier as u64);
        }
        if let Some(mode) = key.mode {
            push_varint_field(&mut key_message, 7, mode as u64);
        }
        if let Some(activated) = key.activated {
            push_varint_field(&mut key_message, 9, activated as u64);
        }
        if let Some(timestamp_msec) = key.timestamp_msec {
            push_varint_field(&mut key_message, 10, timestamp_msec as u64);
        }

        let mut input = Vec::new();
        push_varint_field(&mut input, 1, INPUT_SEND_KEY);
        push_varint_field(&mut input, 2, session_id);
        push_bytes_field(&mut input, 3, &key_message);
        input
    }

    pub fn decode_ipc_key(data: &[u8]) -> Option<String> {
        let mut cursor = Cursor::new(data);
        while let Some((field, wire)) = cursor.read_key() {
            if field == 1 && wire == WIRE_BYTES {
                return cursor.read_bytes().and_then(bytes_to_string);
            }
            cursor.skip(wire)?;
        }
        None
    }

    pub fn find_ascii_hex_key(data: &[u8]) -> Option<String> {
        let mut start = 0usize;
        while start + 32 <= data.len() {
            let candidate = &data[start..start + 32];
            if candidate
                .iter()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b))
            {
                return bytes_to_string(candidate);
            }
            start += 1;
        }
        None
    }

    pub fn decode_command_output(data: &[u8]) -> Option<MozcOutput> {
        decode_output(data)
    }

    fn decode_output(data: &[u8]) -> Option<MozcOutput> {
        let mut output = MozcOutput::default();
        let mut cursor = Cursor::new(data);
        while let Some((field, wire)) = cursor.read_key() {
            match (field, wire) {
                (1, WIRE_VARINT) => output.session_id = cursor.read_varint(),
                (2, WIRE_VARINT) => output.mode = cursor.read_varint().map(|v| v as u32),
                (3, WIRE_VARINT) => output.consumed = cursor.read_varint().map(|v| v != 0),
                (4, WIRE_BYTES) => {
                    let result = cursor.read_bytes()?;
                    output.result = decode_result(result)?;
                }
                (5, WIRE_BYTES) => {
                    let preedit = cursor.read_bytes()?;
                    output.preedit = decode_preedit(preedit);
                }
                (6, WIRE_BYTES) => {
                    let candidate_window = cursor.read_bytes()?;
                    output.candidate_window = decode_candidate_window(candidate_window);
                }
                (13, WIRE_BYTES) => {
                    let status = cursor.read_bytes()?;
                    output.status = Some(decode_status(status)?);
                }
                _ => {
                    if cursor.skip(wire).is_none() {
                        break;
                    }
                }
            }
        }
        Some(output)
    }

    fn decode_candidate_window(data: &[u8]) -> Option<MozcCandidateWindow> {
        let mut window = MozcCandidateWindow::default();
        let mut cursor = Cursor::new(data);
        while let Some((field, wire)) = cursor.read_key() {
            match (field, wire) {
                (1, WIRE_VARINT) => window.focused_index = cursor.read_varint().map(|v| v as usize),
                (2, WIRE_VARINT) => window.size = cursor.read_varint()? as u32,
                (3, WIRE_START_GROUP) => {
                    if let Some(candidate) = decode_candidate(&mut cursor) {
                        window.candidates.push(candidate);
                    }
                }
                _ => {
                    if cursor.skip(wire).is_none() {
                        break;
                    }
                }
            }
        }
        Some(window)
    }

    fn decode_candidate(cursor: &mut Cursor<'_>) -> Option<MozcCandidate> {
        let mut index = None;
        let mut value = None;
        while let Some((field, wire)) = cursor.read_key() {
            match (field, wire) {
                (3, WIRE_END_GROUP) => break,
                (4, WIRE_VARINT) => index = cursor.read_varint().map(|v| v as usize),
                (5, WIRE_BYTES) => value = cursor.read_bytes().and_then(bytes_to_string),
                _ => {
                    if cursor.skip(wire).is_none() {
                        break;
                    }
                }
            }
        }
        Some(MozcCandidate {
            index: index?,
            value: value?,
        })
    }

    fn decode_result(data: &[u8]) -> Option<Option<String>> {
        let mut value = None;
        let mut cursor = Cursor::new(data);
        while let Some((field, wire)) = cursor.read_key() {
            match (field, wire) {
                (2, WIRE_BYTES) => value = cursor.read_bytes().and_then(bytes_to_string),
                _ => {
                    if cursor.skip(wire).is_none() {
                        break;
                    }
                }
            }
        }
        Some(value)
    }

    fn decode_preedit(data: &[u8]) -> Option<MozcPreedit> {
        let mut cursor_chars = 0usize;
        let mut text = String::new();
        let mut cursor = Cursor::new(data);
        while let Some((field, wire)) = cursor.read_key() {
            match (field, wire) {
                (1, WIRE_VARINT) => cursor_chars = cursor.read_varint()? as usize,
                (2, WIRE_START_GROUP) => {
                    text.push_str(&decode_preedit_segment(&mut cursor)?);
                }
                _ => {
                    if cursor.skip(wire).is_none() {
                        break;
                    }
                }
            }
        }
        Some(MozcPreedit { text, cursor_chars })
    }

    fn decode_preedit_segment(cursor: &mut Cursor<'_>) -> Option<String> {
        let mut value = String::new();
        while let Some((field, wire)) = cursor.read_key() {
            match (field, wire) {
                (2, WIRE_END_GROUP) => return Some(value),
                (4, WIRE_BYTES) => value = cursor.read_bytes().and_then(bytes_to_string)?,
                _ => {
                    if cursor.skip(wire).is_none() {
                        return Some(value);
                    }
                }
            }
        }
        Some(value)
    }

    fn decode_status(data: &[u8]) -> Option<MozcStatus> {
        let mut status = MozcStatus::default();
        let mut cursor = Cursor::new(data);
        while let Some((field, wire)) = cursor.read_key() {
            match (field, wire) {
                (1, WIRE_VARINT) => status.activated = cursor.read_varint().map(|v| v != 0),
                (2, WIRE_VARINT) => status.mode = cursor.read_varint().map(|v| v as u32),
                _ => {
                    if cursor.skip(wire).is_none() {
                        break;
                    }
                }
            }
        }
        Some(status)
    }

    fn push_varint_field(out: &mut Vec<u8>, field: u64, value: u64) {
        push_varint(out, (field << 3) | WIRE_VARINT);
        push_varint(out, value);
    }

    fn push_bytes_field(out: &mut Vec<u8>, field: u64, bytes: &[u8]) {
        push_varint(out, (field << 3) | WIRE_BYTES);
        push_varint(out, bytes.len() as u64);
        out.extend_from_slice(bytes);
    }

    fn push_varint(out: &mut Vec<u8>, mut value: u64) {
        while value >= 0x80 {
            out.push((value as u8) | 0x80);
            value >>= 7;
        }
        out.push(value as u8);
    }

    const WIRE_VARINT: u64 = 0;
    const WIRE_64BIT: u64 = 1;
    const WIRE_BYTES: u64 = 2;
    const WIRE_START_GROUP: u64 = 3;
    const WIRE_END_GROUP: u64 = 4;
    const WIRE_32BIT: u64 = 5;

    struct Cursor<'a> {
        data: &'a [u8],
        pos: usize,
    }

    impl<'a> Cursor<'a> {
        fn new(data: &'a [u8]) -> Self {
            Self { data, pos: 0 }
        }

        fn read_key(&mut self) -> Option<(u64, u64)> {
            if self.pos >= self.data.len() {
                return None;
            }
            let key = self.read_varint()?;
            Some((key >> 3, key & 0x7))
        }

        fn read_varint(&mut self) -> Option<u64> {
            let mut shift = 0;
            let mut value = 0u64;
            while self.pos < self.data.len() && shift < 64 {
                let byte = self.data[self.pos];
                self.pos += 1;
                value |= ((byte & 0x7f) as u64) << shift;
                if byte & 0x80 == 0 {
                    return Some(value);
                }
                shift += 7;
            }
            None
        }

        fn read_bytes(&mut self) -> Option<&'a [u8]> {
            let len = self.read_varint()? as usize;
            let end = self.pos.checked_add(len)?;
            if end > self.data.len() {
                return None;
            }
            let bytes = &self.data[self.pos..end];
            self.pos = end;
            Some(bytes)
        }

        fn skip(&mut self, wire: u64) -> Option<()> {
            match wire {
                WIRE_VARINT => {
                    self.read_varint()?;
                }
                WIRE_64BIT => {
                    self.pos = self.pos.checked_add(8)?;
                    if self.pos > self.data.len() {
                        return None;
                    }
                }
                WIRE_BYTES => {
                    self.read_bytes()?;
                }
                WIRE_START_GROUP => {
                    while let Some((_, nested_wire)) = self.read_key() {
                        if nested_wire == WIRE_END_GROUP {
                            break;
                        }
                        self.skip(nested_wire)?;
                    }
                }
                WIRE_END_GROUP => {}
                WIRE_32BIT => {
                    self.pos = self.pos.checked_add(4)?;
                    if self.pos > self.data.len() {
                        return None;
                    }
                }
                _ => return None,
            }
            Some(())
        }
    }

    fn bytes_to_string(bytes: &[u8]) -> Option<String> {
        core::str::from_utf8(bytes).ok().map(ToString::to_string)
    }
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    println!("[scarlet_mozc] connecting to SWS...");

    let mut conn = match Connection::connect_default() {
        Ok(conn) => conn,
        Err(err) => {
            println!("[scarlet_mozc] failed to connect to SWS: {:?}", err);
            return 1;
        }
    };
    let scale_milli = conn.get_output_scale().unwrap_or(1000).max(1);

    let capabilities = ime_capabilities::KEYBOARD_GRAB
        | ime_capabilities::STYLED_PREEDIT
        | ime_capabilities::STATUS
        | ime_capabilities::OWN_CANDIDATE_UI;
    let ime_id = match conn.register_input_method(IME_NAME, capabilities) {
        Ok(ime_id) => ime_id,
        Err(err) => {
            println!("[scarlet_mozc] failed to register IME: {:?}", err);
            return 1;
        }
    };

    if let Err(err) = conn.set_active_input_method(ime_id) {
        println!(
            "[scarlet_mozc] failed to activate IME {}: {:?}",
            ime_id, err
        );
        return 1;
    }

    println!("[scarlet_mozc] registered {} as id={}", IME_NAME, ime_id);

    let mut ime = ScarletMozc::new(scale_milli);
    loop {
        match conn.dispatch() {
            Ok(_) => {
                while let Some(event) = conn.poll_event() {
                    if let Err(err) = ime.handle_event(&mut conn, event) {
                        println!("[scarlet_mozc] event handling failed: {:?}", err);
                        return 1;
                    }
                }
            }
            Err(Error::WouldBlock) => {}
            Err(Error::Disconnected) => {
                println!("[scarlet_mozc] disconnected from SWS");
                return 1;
            }
            Err(err) => {
                println!("[scarlet_mozc] dispatch failed: {:?}", err);
                return 1;
            }
        }

        let _ = thread::sleep(Duration::from_millis(10));
    }
}
