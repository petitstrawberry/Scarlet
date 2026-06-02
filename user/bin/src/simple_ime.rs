//! Simple external IME service for SWS text-input protocol.
//!
//! This binary intentionally keeps conversion outside SWS. SWS only brokers
//! text-input state and key events between applications and this process.

#![no_std]
#![no_main]

extern crate alloc;
extern crate scarlet_std as std;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::time::Duration;
use scarlet_ui::{Buffer, Canvas, Color};
use std::fs::File;
use std::println;
use std::thread;
use sws_client::{Connection, Error, Event, SurfaceBuilder, event_type, key_code};
use sws_protocol::{ime_capabilities, ime_state, ime_status_flags, ime_trigger, window_types};

const IME_NAME: &str = "simple-skk";
const MODE_DIRECT_ID: u32 = 0;
const MODE_SIMPLE_SKK_ID: u32 = 1;
const MODE_DIRECT_LABEL: &str = "Direct";
const MODE_SIMPLE_SKK_LABEL: &str = "Simple SKK";
const SKK_DICTIONARY_PATHS: &[&str] = &[
    "/share/skk/SKK-JISYO.L",
    "/usr/share/skk/SKK-JISYO.L",
    "/usr/local/share/skk/SKK-JISYO.L",
    "/etc/skk/SKK-JISYO.L",
];
const MAX_SKK_DICTIONARY_BYTES: usize = 16 * 1024 * 1024;
const CANDIDATE_POPUP_WIDTH: u32 = 360;
const CANDIDATE_POPUP_HEIGHT: u32 = 148;
const CANDIDATE_POPUP_PAGE_SIZE: usize = 5;
const CANDIDATE_POPUP_OFFSET_X: i32 = 0;
const CANDIDATE_POPUP_OFFSET_Y: i32 = 4;
const CANDIDATE_POPUP_PADDING_X: i32 = 4;
const CANDIDATE_ROW_HEIGHT: i32 = 28;
const CANDIDATE_POPUP_PADDING_Y: i32 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SkkPhase {
    Direct,
    Preedit,
    Candidate,
}

struct SimpleIme {
    active_context_id: Option<u32>,
    grabbing: bool,
    dictionary: Vec<SkkDictionaryEntry>,
    pending: Vec<u8>,
    phase: SkkPhase,
    reading: String,
    okuri_marker: Option<char>,
    okuri: String,
    selected_index: usize,
    left_shift_down: bool,
    right_shift_down: bool,
    eaten_keys: Vec<u16>,
    candidate_popup: Option<CandidatePopup>,
    candidate_popup_requested: bool,
    scale_milli: u32,
}

impl SimpleIme {
    fn new(scale_milli: u32) -> Self {
        let dictionary = load_skk_dictionary();
        Self {
            active_context_id: None,
            grabbing: false,
            dictionary,
            pending: Vec::new(),
            phase: SkkPhase::Direct,
            reading: String::new(),
            okuri_marker: None,
            okuri: String::new(),
            selected_index: 0,
            left_shift_down: false,
            right_shift_down: false,
            eaten_keys: Vec::new(),
            candidate_popup: None,
            candidate_popup_requested: false,
            scale_milli: scale_milli.max(1),
        }
    }

    fn reset_context(&mut self) {
        self.pending.clear();
        self.phase = SkkPhase::Direct;
        self.reading.clear();
        self.okuri_marker = None;
        self.okuri.clear();
        self.selected_index = 0;
        self.eaten_keys.clear();
        self.candidate_popup_requested = false;
    }

    fn handle_event(&mut self, conn: &mut Connection, event: Event) -> Result<(), Error> {
        match event {
            Event::ImeActivate(state) => {
                self.reset_context();
                self.active_context_id = Some(state.context_id);
                self.grabbing = false;
                conn.ime_set_preedit(state.context_id, 0, 0, "", &[])?;
                self.sync_candidate_popup(conn, state.context_id)?;
                self.emit_status(conn, state.context_id)?;
                println!(
                    "[simple_ime] activated context={} window={}",
                    state.context_id, state.window_id
                );
            }
            Event::ImeDeactivate { context_id, .. } => {
                if self.active_context_id == Some(context_id) {
                    self.hide_candidate_popup(conn, context_id)?;
                    self.active_context_id = None;
                    self.grabbing = false;
                    self.reset_context();
                }
                println!("[simple_ime] deactivated context={}", context_id);
            }
            Event::ImeContextState(state) => {
                self.active_context_id = Some(state.context_id);
                println!(
                    "[simple_ime] context-state context={} serial={} cursor=({}, {}) purpose={} text='{}' grabbing={}",
                    state.context_id,
                    state.serial,
                    state.cursor_x,
                    state.cursor_y,
                    state.content_purpose,
                    self.debug_text(),
                    self.grabbing
                );
            }
            Event::ImeReset { context_id, .. } => {
                if self.active_context_id == Some(context_id) {
                    println!("[simple_ime] reset context={}", context_id);
                    self.grabbing = false;
                    self.reset_context();
                    conn.ime_set_preedit(context_id, 0, 0, "", &[])?;
                    self.sync_candidate_popup(conn, context_id)?;
                    self.emit_status(conn, context_id)?;
                }
            }
            Event::ImeTrigger {
                context_id,
                trigger_id,
                code,
                time,
                serial,
            } => {
                println!(
                    "[simple_ime] trigger context={} serial={} trigger={} code={} time={} grabbing={}",
                    context_id, serial, trigger_id, code, time, self.grabbing
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
                ..
            } => {
                println!(
                    "[simple_ime] key-event context={} serial={} {}({}) type={} value={} grabbing={} text='{}'",
                    context_id,
                    key_serial,
                    key_name(code),
                    code,
                    type_,
                    value,
                    self.grabbing,
                    self.debug_text()
                );
                let handled = self.handle_key(conn, context_id, type_, code, value)?;
                conn.ime_key_handled(key_serial, handled)?;
                println!(
                    "[simple_ime] key-handled serial={} handled={} text='{}'",
                    key_serial,
                    handled,
                    self.debug_text()
                );
            }
            Event::SurfaceDestroyed { surface_id } => {
                if self
                    .candidate_popup
                    .is_some_and(|popup| popup.window_id == surface_id)
                {
                    println!(
                        "[simple_ime] candidate popup destroyed window={}",
                        surface_id
                    );
                    self.candidate_popup = None;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_key(
        &mut self,
        conn: &mut Connection,
        context_id: u32,
        type_: u16,
        code: u16,
        value: i32,
    ) -> Result<bool, Error> {
        if !self.grabbing || self.active_context_id != Some(context_id) {
            println!(
                "[simple_ime] pass-through: not grabbing or inactive context={} active={:?}",
                context_id, self.active_context_id
            );
            return Ok(false);
        }
        if type_ != event_type::EV_KEY {
            println!("[simple_ime] pass-through: non-key type={}", type_);
            return Ok(false);
        }

        if code == key_code::KEY_LEFTSHIFT || code == key_code::KEY_RIGHTSHIFT {
            self.set_shift_state(code, value != 0);
            return Ok(true);
        }

        if value == 0 {
            let handled = self.remove_eaten_key(code);
            println!(
                "[simple_ime] key-release {}({}) handled={}",
                key_name(code),
                code,
                handled
            );
            return Ok(handled);
        }

        let handled = match letter_from_key(code) {
            Some(ch) => {
                self.handle_letter(conn, context_id, ch)?;
                true
            }
            None => {
                if let Some(ch) = printable_symbol_from_key(code, self.shift_down()) {
                    self.handle_printable_symbol(conn, context_id, ch)?;
                    true
                } else {
                    self.handle_control_key(conn, context_id, code)?
                }
            }
        };

        if handled && !self.eaten_keys.contains(&code) {
            self.eaten_keys.push(code);
        }

        Ok(handled)
    }

    fn toggle_keyboard_grab(
        &mut self,
        conn: &mut Connection,
        context_id: u32,
    ) -> Result<(), Error> {
        self.active_context_id = Some(context_id);

        if self.grabbing {
            println!(
                "[simple_ime] toggle: release requested context={} pending='{}'",
                context_id,
                self.pending_text()
            );
            self.commit_current_text(conn, context_id)?;
            conn.ime_release_keyboard(context_id)?;
            self.grabbing = false;
            self.reset_context();
            conn.ime_set_preedit(context_id, 0, 0, "", &[])?;
            self.sync_candidate_popup(conn, context_id)?;
            self.emit_status(conn, context_id)?;
            println!("[simple_ime] released keyboard context={}", context_id);
            return Ok(());
        }

        println!("[simple_ime] toggle: grab requested context={}", context_id);
        self.reset_context();
        self.grabbing = true;
        conn.ime_grab_keyboard(context_id)?;
        conn.ime_set_preedit(context_id, 0, 0, "", &[])?;
        self.emit_status(conn, context_id)?;
        println!("[simple_ime] grabbed keyboard context={}", context_id);
        Ok(())
    }

    fn handle_letter(
        &mut self,
        conn: &mut Connection,
        context_id: u32,
        ch: char,
    ) -> Result<(), Error> {
        let shifted = self.shift_down();
        if self.phase == SkkPhase::Candidate {
            self.commit_current_text(conn, context_id)?;
        }

        match self.phase {
            SkkPhase::Direct => {
                if shifted {
                    self.phase = SkkPhase::Preedit;
                    self.pending.push(ch as u8);
                    self.absorb_ready_kana();
                    self.update_preedit(conn, context_id)
                } else {
                    self.pending.push(ch as u8);
                    let text = drain_committable(&mut self.pending);
                    if !text.is_empty() {
                        println!("[simple_ime] direct commit '{}'", text);
                        conn.ime_commit_text(context_id, &text)?;
                    }
                    self.update_preedit(conn, context_id)
                }
            }
            SkkPhase::Preedit => {
                if shifted && !self.reading.is_empty() && self.okuri_marker.is_none() {
                    self.flush_pending_to_reading();
                    self.okuri_marker = Some(ch);
                    self.pending.push(ch as u8);
                } else {
                    self.pending.push(ch as u8);
                }
                self.absorb_ready_kana();
                self.update_preedit(conn, context_id)
            }
            SkkPhase::Candidate => Ok(()),
        }
    }

    fn handle_printable_symbol(
        &mut self,
        conn: &mut Connection,
        context_id: u32,
        ch: char,
    ) -> Result<(), Error> {
        if self.phase == SkkPhase::Candidate {
            self.commit_current_text(conn, context_id)?;
        }

        if self.phase == SkkPhase::Direct && self.pending.is_empty() {
            let mut text = String::new();
            text.push(ch);
            println!("[simple_ime] direct symbol commit '{}'", text);
            conn.ime_commit_text(context_id, &text)?;
            return self.update_preedit(conn, context_id);
        }

        if self.phase == SkkPhase::Direct {
            self.phase = SkkPhase::Preedit;
        }
        self.flush_pending_to_reading();
        if self.okuri_marker.is_some() {
            self.okuri.push(ch);
        } else {
            self.reading.push(ch);
        }
        self.update_preedit(conn, context_id)
    }

    fn handle_control_key(
        &mut self,
        conn: &mut Connection,
        context_id: u32,
        code: u16,
    ) -> Result<bool, Error> {
        match code {
            key_code::KEY_BACKSPACE => self.handle_backspace(conn, context_id),
            key_code::KEY_DELETE => self.handle_delete(conn, context_id),
            key_code::KEY_ENTER => {
                if self.is_empty() {
                    Ok(false)
                } else {
                    self.commit_current_text(conn, context_id)?;
                    Ok(true)
                }
            }
            key_code::KEY_SPACE => {
                if self.is_empty() {
                    Ok(false)
                } else {
                    self.convert_or_select_next(conn, context_id)?;
                    Ok(true)
                }
            }
            key_code::KEY_ESC => self.handle_escape(conn, context_id),
            key_code::KEY_RIGHT | key_code::KEY_DOWN => {
                if self.phase == SkkPhase::Candidate {
                    self.select_next_candidate();
                    self.update_preedit(conn, context_id)?;
                    Ok(true)
                } else {
                    Ok(false)
                }
            }
            key_code::KEY_LEFT | key_code::KEY_UP => {
                if self.phase == SkkPhase::Candidate {
                    self.select_previous_candidate();
                    self.update_preedit(conn, context_id)?;
                    Ok(true)
                } else {
                    Ok(false)
                }
            }
            key_code::KEY_COMMA => {
                self.commit_current_text(conn, context_id)?;
                conn.ime_commit_text(context_id, "、")?;
                Ok(true)
            }
            key_code::KEY_DOT => {
                self.commit_current_text(conn, context_id)?;
                conn.ime_commit_text(context_id, "。")?;
                Ok(true)
            }
            key_code::KEY_SLASH => {
                self.commit_current_text(conn, context_id)?;
                conn.ime_commit_text(context_id, "・")?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    fn handle_backspace(&mut self, conn: &mut Connection, context_id: u32) -> Result<bool, Error> {
        if self.is_empty() && self.eaten_keys.contains(&key_code::KEY_BACKSPACE) {
            return Ok(true);
        }
        if self.phase == SkkPhase::Candidate {
            self.select_previous_candidate();
            self.update_preedit(conn, context_id)?;
            return Ok(true);
        }
        if self.pending.pop().is_some() {
            self.update_preedit(conn, context_id)?;
            return Ok(true);
        }
        if self.okuri.pop().is_some() {
            self.update_preedit(conn, context_id)?;
            return Ok(true);
        }
        if self.okuri_marker.take().is_some() {
            self.update_preedit(conn, context_id)?;
            return Ok(true);
        }
        if self.reading.pop().is_some() {
            if self.reading.is_empty() {
                self.phase = SkkPhase::Direct;
            }
            self.update_preedit(conn, context_id)?;
            return Ok(true);
        }
        Ok(false)
    }

    fn handle_delete(&mut self, conn: &mut Connection, context_id: u32) -> Result<bool, Error> {
        if self.is_empty() {
            if self.eaten_keys.contains(&key_code::KEY_DELETE) {
                return Ok(true);
            }
            return Ok(false);
        }

        // The current sample IME keeps the preedit cursor at the end. Delete is
        // therefore distinct from Backspace: it does not remove the previous
        // preedit unit, but it must not leak to the application while composing.
        self.update_preedit(conn, context_id)?;
        Ok(true)
    }

    fn handle_escape(&mut self, conn: &mut Connection, context_id: u32) -> Result<bool, Error> {
        match self.phase {
            SkkPhase::Candidate => {
                self.phase = SkkPhase::Preedit;
                self.selected_index = 0;
                self.candidate_popup_requested = false;
                self.update_preedit(conn, context_id)?;
                Ok(true)
            }
            SkkPhase::Preedit => {
                self.clear_composition();
                self.update_preedit(conn, context_id)?;
                Ok(true)
            }
            SkkPhase::Direct if !self.pending.is_empty() => {
                self.pending.clear();
                self.update_preedit(conn, context_id)?;
                Ok(true)
            }
            SkkPhase::Direct => Ok(false),
        }
    }

    fn convert_or_select_next(
        &mut self,
        conn: &mut Connection,
        context_id: u32,
    ) -> Result<(), Error> {
        if self.phase == SkkPhase::Candidate {
            self.select_next_candidate();
        } else {
            self.flush_pending_to_reading();
            if self.reading.is_empty() {
                return self.update_preedit(conn, context_id);
            }
            self.phase = SkkPhase::Candidate;
            self.selected_index = 0;
            self.candidate_popup_requested = false;
        }
        self.update_preedit(conn, context_id)
    }

    fn commit_current_text(&mut self, conn: &mut Connection, context_id: u32) -> Result<(), Error> {
        self.flush_pending_to_reading();
        let text = self.current_commit_text();
        self.clear_composition();
        if !text.is_empty() {
            println!("[simple_ime] commit '{}'", text);
            conn.ime_commit_text(context_id, &text)?;
        }
        self.update_preedit(conn, context_id)
    }

    fn update_preedit(&mut self, conn: &mut Connection, context_id: u32) -> Result<(), Error> {
        let preedit = self.preedit_text();
        println!("[simple_ime] preedit '{}'", preedit);
        let spans = preedit_spans(&preedit, self.phase);
        conn.ime_set_preedit(context_id, preedit.len() as u32, 0, &preedit, &spans)?;
        self.sync_candidate_popup(conn, context_id)?;
        self.emit_status(conn, context_id)?;
        Ok(())
    }

    fn emit_status(&self, conn: &mut Connection, context_id: u32) -> Result<(), Error> {
        let state = match self.phase {
            SkkPhase::Candidate => ime_state::CANDIDATES,
            SkkPhase::Preedit => ime_state::COMPOSING,
            SkkPhase::Direct if !self.pending.is_empty() => ime_state::COMPOSING,
            SkkPhase::Direct => ime_state::DIRECT,
        };
        let (mode_id, mode_label) = if self.grabbing {
            (MODE_SIMPLE_SKK_ID, MODE_SIMPLE_SKK_LABEL)
        } else {
            (MODE_DIRECT_ID, MODE_DIRECT_LABEL)
        };
        let mut flags = 0;
        if self.grabbing {
            flags |= ime_status_flags::MODE_ACTIVE;
        }
        if self.phase == SkkPhase::Candidate {
            flags |= ime_status_flags::CANDIDATES_VISIBLE;
        }
        println!(
            "[simple_ime] status context={} state={} mode_id={} label='{}' flags={}",
            context_id, state, mode_id, mode_label, flags
        );
        conn.ime_set_status(context_id, state, mode_id, flags, mode_label)
    }

    fn pending_text(&self) -> &str {
        core::str::from_utf8(&self.pending).unwrap_or("<invalid>")
    }

    fn debug_text(&self) -> String {
        let mut text = self.preedit_text();
        if text.is_empty() {
            text.push_str(self.pending_text());
        }
        text
    }

    fn set_shift_state(&mut self, code: u16, pressed: bool) {
        match code {
            key_code::KEY_LEFTSHIFT => self.left_shift_down = pressed,
            key_code::KEY_RIGHTSHIFT => self.right_shift_down = pressed,
            _ => {}
        }
    }

    fn shift_down(&self) -> bool {
        self.left_shift_down || self.right_shift_down
    }

    fn is_empty(&self) -> bool {
        self.pending.is_empty()
            && self.reading.is_empty()
            && self.okuri_marker.is_none()
            && self.okuri.is_empty()
    }

    fn absorb_ready_kana(&mut self) {
        let kana = drain_committable(&mut self.pending);
        if kana.is_empty() {
            return;
        }
        if self.okuri_marker.is_some() {
            self.okuri.push_str(&kana);
        } else {
            self.reading.push_str(&kana);
        }
    }

    fn flush_pending_to_reading(&mut self) {
        let mut text = drain_committable(&mut self.pending);
        if self.pending == b"n" {
            text.push_str("ん");
        } else if !self.pending.is_empty()
            && let Ok(raw) = core::str::from_utf8(&self.pending)
        {
            text.push_str(raw);
        }
        self.pending.clear();

        if self.okuri_marker.is_some() {
            self.okuri.push_str(&text);
        } else {
            self.reading.push_str(&text);
        }
    }

    fn clear_composition(&mut self) {
        self.pending.clear();
        self.phase = SkkPhase::Direct;
        self.reading.clear();
        self.okuri_marker = None;
        self.okuri.clear();
        self.selected_index = 0;
        self.candidate_popup_requested = false;
    }

    fn candidates(&self) -> Vec<&str> {
        let key = self.dictionary_key();
        if let Some(candidates) = self.lookup_loaded_dictionary(&key) {
            return candidates;
        }
        if let Some(candidates) = lookup_builtin_dictionary(&key) {
            return candidates.to_vec();
        }
        if self.reading.is_empty() {
            Vec::new()
        } else {
            let mut candidates = Vec::new();
            candidates.push(self.reading.as_str());
            candidates
        }
    }

    fn lookup_loaded_dictionary(&self, key: &str) -> Option<Vec<&str>> {
        self.dictionary
            .binary_search_by(|entry| entry.key.as_str().cmp(key))
            .ok()
            .map(|index| {
                self.dictionary[index]
                    .candidates
                    .iter()
                    .map(String::as_str)
                    .collect()
            })
    }

    fn dictionary_key(&self) -> String {
        let mut key = self.reading.clone();
        if let Some(marker) = self.okuri_marker {
            key.push(marker);
        }
        key
    }

    fn current_commit_text(&self) -> String {
        let mut text = String::new();
        if self.phase == SkkPhase::Candidate {
            let candidates = self.candidates();
            if let Some(candidate) = candidates.get(self.selected_index) {
                text.push_str(candidate);
                text.push_str(&self.okuri);
                return text;
            }
        }
        text.push_str(&self.reading);
        text.push_str(&self.okuri);
        if let Ok(raw) = core::str::from_utf8(&self.pending) {
            text.push_str(raw);
        }
        text
    }

    fn preedit_text(&self) -> String {
        let mut text = String::new();
        match self.phase {
            SkkPhase::Direct => {
                if let Ok(raw) = core::str::from_utf8(&self.pending) {
                    text.push_str(raw);
                }
            }
            SkkPhase::Preedit => {
                text.push_str("▽");
                text.push_str(&self.reading);
                if self.okuri_marker.is_some() {
                    text.push('*');
                }
                text.push_str(&self.okuri);
                if let Ok(raw) = core::str::from_utf8(&self.pending) {
                    text.push_str(raw);
                }
            }
            SkkPhase::Candidate => {
                text.push_str("▼");
                let candidates = self.candidates();
                if let Some(candidate) = candidates.get(self.selected_index) {
                    text.push_str(candidate);
                } else {
                    text.push_str(&self.reading);
                }
                text.push_str(&self.okuri);
            }
        }
        text
    }

    fn select_next_candidate(&mut self) {
        let len = self.candidates().len();
        if len > 0 {
            self.selected_index = (self.selected_index + 1) % len;
            self.candidate_popup_requested = true;
        }
    }

    fn select_previous_candidate(&mut self) {
        let len = self.candidates().len();
        if len > 0 {
            self.selected_index = if self.selected_index == 0 {
                len - 1
            } else {
                self.selected_index - 1
            };
            self.candidate_popup_requested = true;
        }
    }

    fn remove_eaten_key(&mut self, code: u16) -> bool {
        let Some(index) = self.eaten_keys.iter().position(|key| *key == code) else {
            return false;
        };
        self.eaten_keys.remove(index);
        true
    }

    fn ensure_candidate_popup(&mut self, conn: &mut Connection) -> Result<u32, Error> {
        if let Some(popup) = self.candidate_popup {
            return Ok(popup.window_id);
        }

        let window_id = SurfaceBuilder::new()
            .app_id("org.scarlet.simple-ime.candidates")
            .app_name("Simple IME Candidates")
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
        self.candidate_popup = Some(CandidatePopup {
            window_id,
            visible: false,
        });
        println!("[simple_ime] created candidate popup window={}", window_id);
        Ok(window_id)
    }

    fn hide_candidate_popup(
        &mut self,
        conn: &mut Connection,
        context_id: u32,
    ) -> Result<(), Error> {
        let Some(mut popup) = self.candidate_popup else {
            return Ok(());
        };
        if popup.visible {
            conn.ime_set_popup_window(context_id, popup.window_id, 0, 0, false)?;
            popup.visible = false;
            self.candidate_popup = Some(popup);
            println!("[simple_ime] candidate popup hidden context={}", context_id);
        }
        Ok(())
    }

    fn sync_candidate_popup(
        &mut self,
        conn: &mut Connection,
        context_id: u32,
    ) -> Result<(), Error> {
        if self.phase != SkkPhase::Candidate {
            return self.hide_candidate_popup(conn, context_id);
        }
        if !self.candidate_popup_requested {
            return self.hide_candidate_popup(conn, context_id);
        }

        let rows = self.candidate_popup_rows();
        if rows.is_empty() {
            return self.hide_candidate_popup(conn, context_id);
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
        self.candidate_popup = Some(CandidatePopup {
            window_id,
            visible: true,
        });
        println!(
            "[simple_ime] candidate popup shown context={} window={} selected={}",
            context_id, window_id, self.selected_index
        );
        Ok(())
    }

    fn candidate_popup_rows(&self) -> Vec<CandidatePopupRow> {
        let candidates = self.candidates();
        if candidates.is_empty() {
            return Vec::new();
        }

        let page_start = (self.selected_index / CANDIDATE_POPUP_PAGE_SIZE)
            .saturating_mul(CANDIDATE_POPUP_PAGE_SIZE);
        let mut rows = Vec::new();
        for (index, candidate) in candidates
            .iter()
            .enumerate()
            .skip(page_start)
            .take(CANDIDATE_POPUP_PAGE_SIZE)
        {
            rows.push(CandidatePopupRow {
                index,
                text: candidate.to_string(),
                selected: index == self.selected_index,
            });
        }
        rows
    }
}

#[derive(Clone, Copy)]
struct CandidatePopup {
    window_id: u32,
    visible: bool,
}

struct CandidatePopupRow {
    index: usize,
    text: String,
    selected: bool,
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

fn drain_committable(pending: &mut Vec<u8>) -> String {
    let mut out = String::new();

    loop {
        let text = core::str::from_utf8(pending).unwrap_or("");
        if text.is_empty() {
            break;
        }

        if let Some((prefix_len, kana)) = special_prefix(text) {
            out.push_str(kana);
            pending.drain(..prefix_len);
            continue;
        }

        if let Some((prefix_len, kana)) = ready_mapping(text) {
            out.push_str(kana);
            pending.drain(..prefix_len);
            continue;
        }

        if is_known_prefix(text) {
            break;
        }

        let first = pending[0] as char;
        out.push(first);
        pending.drain(..1);
    }

    out
}

fn special_prefix(text: &str) -> Option<(usize, &'static str)> {
    let bytes = text.as_bytes();
    if bytes.len() >= 2 && bytes[0] == bytes[1] && is_consonant(bytes[0]) && bytes[0] != b'n' {
        return Some((1, "っ"));
    }

    if text.starts_with("nn") {
        return Some((2, "ん"));
    }

    if bytes.len() >= 2 && bytes[0] == b'n' && !is_vowel(bytes[1]) && bytes[1] != b'y' {
        return Some((1, "ん"));
    }

    None
}

fn ready_mapping(text: &str) -> Option<(usize, &'static str)> {
    let mut best = None;
    for (roma, kana) in ROMAJI_KANA {
        if text.starts_with(roma) && best.is_none_or(|(len, _)| roma.len() > len) {
            best = Some((roma.len(), *kana));
        }
    }
    best
}

fn is_known_prefix(text: &str) -> bool {
    ROMAJI_KANA
        .iter()
        .any(|(roma, _)| roma.starts_with(text) && roma.len() > text.len())
        || text == "n"
}

fn is_vowel(byte: u8) -> bool {
    matches!(byte, b'a' | b'i' | b'u' | b'e' | b'o')
}

fn is_consonant(byte: u8) -> bool {
    byte.is_ascii_lowercase() && !is_vowel(byte)
}

fn letter_from_key(code: u16) -> Option<char> {
    match code {
        key_code::KEY_A => Some('a'),
        key_code::KEY_B => Some('b'),
        key_code::KEY_C => Some('c'),
        key_code::KEY_D => Some('d'),
        key_code::KEY_E => Some('e'),
        key_code::KEY_F => Some('f'),
        key_code::KEY_G => Some('g'),
        key_code::KEY_H => Some('h'),
        key_code::KEY_I => Some('i'),
        key_code::KEY_J => Some('j'),
        key_code::KEY_K => Some('k'),
        key_code::KEY_L => Some('l'),
        key_code::KEY_M => Some('m'),
        key_code::KEY_N => Some('n'),
        key_code::KEY_O => Some('o'),
        key_code::KEY_P => Some('p'),
        key_code::KEY_Q => Some('q'),
        key_code::KEY_R => Some('r'),
        key_code::KEY_S => Some('s'),
        key_code::KEY_T => Some('t'),
        key_code::KEY_U => Some('u'),
        key_code::KEY_V => Some('v'),
        key_code::KEY_W => Some('w'),
        key_code::KEY_X => Some('x'),
        key_code::KEY_Y => Some('y'),
        key_code::KEY_Z => Some('z'),
        _ => None,
    }
}

fn printable_symbol_from_key(code: u16, shifted: bool) -> Option<char> {
    match code {
        key_code::KEY_1 => Some(if shifted { '！' } else { '１' }),
        key_code::KEY_2 => Some(if shifted { '＠' } else { '２' }),
        key_code::KEY_3 => Some(if shifted { '＃' } else { '３' }),
        key_code::KEY_4 => Some(if shifted { '＄' } else { '４' }),
        key_code::KEY_5 => Some(if shifted { '％' } else { '５' }),
        key_code::KEY_6 => Some(if shifted { '＾' } else { '６' }),
        key_code::KEY_7 => Some(if shifted { '＆' } else { '７' }),
        key_code::KEY_8 => Some(if shifted { '＊' } else { '８' }),
        key_code::KEY_9 => Some(if shifted { '（' } else { '９' }),
        key_code::KEY_0 => Some(if shifted { '）' } else { '０' }),
        key_code::KEY_MINUS => Some(if shifted { '＿' } else { '－' }),
        key_code::KEY_EQUAL => Some(if shifted { '＋' } else { '＝' }),
        key_code::KEY_LEFTBRACE => Some(if shifted { '｛' } else { '［' }),
        key_code::KEY_RIGHTBRACE => Some(if shifted { '｝' } else { '］' }),
        key_code::KEY_SEMICOLON => Some(if shifted { '：' } else { '；' }),
        key_code::KEY_APOSTROPHE => Some(if shifted { '＂' } else { '＇' }),
        key_code::KEY_COMMA if shifted => Some('＜'),
        key_code::KEY_DOT if shifted => Some('＞'),
        key_code::KEY_SLASH if shifted => Some('？'),
        key_code::KEY_BACKSLASH => Some(if shifted { '｜' } else { '＼' }),
        _ => None,
    }
}

fn key_name(code: u16) -> &'static str {
    match code {
        key_code::KEY_ESC => "KEY_ESC",
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
        key_code::KEY_ENTER => "KEY_ENTER",
        key_code::KEY_SPACE => "KEY_SPACE",
        key_code::KEY_BACKSPACE => "KEY_BACKSPACE",
        key_code::KEY_DELETE => "KEY_DELETE",
        key_code::KEY_LEFTSHIFT => "KEY_LEFTSHIFT",
        key_code::KEY_RIGHTSHIFT => "KEY_RIGHTSHIFT",
        key_code::KEY_UP => "KEY_UP",
        key_code::KEY_DOWN => "KEY_DOWN",
        key_code::KEY_LEFT => "KEY_LEFT",
        key_code::KEY_RIGHT => "KEY_RIGHT",
        key_code::KEY_COMMA => "KEY_COMMA",
        key_code::KEY_DOT => "KEY_DOT",
        key_code::KEY_SLASH => "KEY_SLASH",
        key_code::KEY_MINUS => "KEY_MINUS",
        key_code::KEY_EQUAL => "KEY_EQUAL",
        key_code::KEY_SEMICOLON => "KEY_SEMICOLON",
        key_code::KEY_APOSTROPHE => "KEY_APOSTROPHE",
        key_code::KEY_LEFTBRACE => "KEY_LEFTBRACE",
        key_code::KEY_RIGHTBRACE => "KEY_RIGHTBRACE",
        key_code::KEY_BACKSLASH => "KEY_BACKSLASH",
        _ => "KEY_UNKNOWN",
    }
}

struct SkkDictEntry {
    key: &'static str,
    candidates: &'static [&'static str],
}

#[derive(Clone, Debug)]
struct SkkDictionaryEntry {
    key: String,
    candidates: Vec<String>,
}

const CAND_KANJI: &[&str] = &["漢字", "感じ", "幹事"];
const CAND_NIHON: &[&str] = &["日本"];
const CAND_NIHONGO: &[&str] = &["日本語"];
const CAND_KYOU: &[&str] = &["今日", "京"];
const CAND_ASHITA: &[&str] = &["明日"];
const CAND_WATASHI: &[&str] = &["私"];
const CAND_NAMAE: &[&str] = &["名前"];
const CAND_KOTOBA: &[&str] = &["言葉"];
const CAND_HENKAN: &[&str] = &["変換"];
const CAND_JISHO: &[&str] = &["辞書"];
const CAND_TOUKYOU: &[&str] = &["東京"];
const CAND_OOSAKA: &[&str] = &["大阪"];
const CAND_SEKAI: &[&str] = &["世界"];
const CAND_SAKURA: &[&str] = &["桜"];
const CAND_HITO: &[&str] = &["人"];
const CAND_YAMA: &[&str] = &["山"];
const CAND_KAWA: &[&str] = &["川"];
const CAND_MIZU: &[&str] = &["水"];
const CAND_HI: &[&str] = &["日", "火"];
const CAND_TSUKI: &[&str] = &["月"];
const CAND_IK: &[&str] = &["行"];
const CAND_KAK: &[&str] = &["書"];
const CAND_YOM: &[&str] = &["読"];
const CAND_UGOK: &[&str] = &["動"];
const CAND_OKUR: &[&str] = &["送"];
const CAND_TABER: &[&str] = &["食べ"];

const BUILTIN_SKK_DICTIONARY: &[SkkDictEntry] = &[
    SkkDictEntry {
        key: "かんじ",
        candidates: CAND_KANJI,
    },
    SkkDictEntry {
        key: "にほん",
        candidates: CAND_NIHON,
    },
    SkkDictEntry {
        key: "にほんご",
        candidates: CAND_NIHONGO,
    },
    SkkDictEntry {
        key: "きょう",
        candidates: CAND_KYOU,
    },
    SkkDictEntry {
        key: "あした",
        candidates: CAND_ASHITA,
    },
    SkkDictEntry {
        key: "わたし",
        candidates: CAND_WATASHI,
    },
    SkkDictEntry {
        key: "なまえ",
        candidates: CAND_NAMAE,
    },
    SkkDictEntry {
        key: "ことば",
        candidates: CAND_KOTOBA,
    },
    SkkDictEntry {
        key: "へんかん",
        candidates: CAND_HENKAN,
    },
    SkkDictEntry {
        key: "じしょ",
        candidates: CAND_JISHO,
    },
    SkkDictEntry {
        key: "とうきょう",
        candidates: CAND_TOUKYOU,
    },
    SkkDictEntry {
        key: "おおさか",
        candidates: CAND_OOSAKA,
    },
    SkkDictEntry {
        key: "せかい",
        candidates: CAND_SEKAI,
    },
    SkkDictEntry {
        key: "さくら",
        candidates: CAND_SAKURA,
    },
    SkkDictEntry {
        key: "ひと",
        candidates: CAND_HITO,
    },
    SkkDictEntry {
        key: "やま",
        candidates: CAND_YAMA,
    },
    SkkDictEntry {
        key: "かわ",
        candidates: CAND_KAWA,
    },
    SkkDictEntry {
        key: "みず",
        candidates: CAND_MIZU,
    },
    SkkDictEntry {
        key: "ひ",
        candidates: CAND_HI,
    },
    SkkDictEntry {
        key: "つき",
        candidates: CAND_TSUKI,
    },
    SkkDictEntry {
        key: "いk",
        candidates: CAND_IK,
    },
    SkkDictEntry {
        key: "かk",
        candidates: CAND_KAK,
    },
    SkkDictEntry {
        key: "よm",
        candidates: CAND_YOM,
    },
    SkkDictEntry {
        key: "うごk",
        candidates: CAND_UGOK,
    },
    SkkDictEntry {
        key: "おくr",
        candidates: CAND_OKUR,
    },
    SkkDictEntry {
        key: "たべr",
        candidates: CAND_TABER,
    },
];

fn lookup_builtin_dictionary(key: &str) -> Option<&'static [&'static str]> {
    BUILTIN_SKK_DICTIONARY
        .iter()
        .find(|entry| entry.key == key)
        .map(|entry| entry.candidates)
}

fn load_skk_dictionary() -> Vec<SkkDictionaryEntry> {
    for path in SKK_DICTIONARY_PATHS {
        match load_skk_dictionary_file(path) {
            Some(dictionary) if !dictionary.is_empty() => {
                println!(
                    "[simple_ime] loaded SKK dictionary '{}' entries={}",
                    path,
                    dictionary.len()
                );
                return dictionary;
            }
            Some(_) => {
                println!("[simple_ime] ignored empty SKK dictionary '{}'", path);
            }
            None => {}
        }
    }
    println!("[simple_ime] no UTF-8 SKK dictionary found; using built-in fallback");
    Vec::new()
}

fn load_skk_dictionary_file(path: &str) -> Option<Vec<SkkDictionaryEntry>> {
    let text = read_dictionary_text(path)?;
    let mut dictionary = parse_skk_dictionary(&text);
    dictionary.sort_by(|a, b| a.key.as_str().cmp(b.key.as_str()));
    merge_duplicate_entries(&mut dictionary);
    Some(dictionary)
}

fn read_dictionary_text(path: &str) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 4096];

    loop {
        let len = match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(len) => len,
            Err(err) => {
                println!(
                    "[simple_ime] failed to read SKK dictionary '{}': {:?}",
                    path, err
                );
                return None;
            }
        };
        if bytes.len() + len > MAX_SKK_DICTIONARY_BYTES {
            println!(
                "[simple_ime] SKK dictionary '{}' is larger than {} bytes",
                path, MAX_SKK_DICTIONARY_BYTES
            );
            return None;
        }
        bytes.extend_from_slice(&buffer[..len]);
    }

    match String::from_utf8(bytes) {
        Ok(text) => Some(text),
        Err(_) => {
            println!(
                "[simple_ime] SKK dictionary '{}' is not UTF-8; fetch script converts upstream EUC-JP",
                path
            );
            None
        }
    }
}

fn parse_skk_dictionary(text: &str) -> Vec<SkkDictionaryEntry> {
    let mut dictionary = Vec::new();
    for line in text.lines() {
        if let Some(entry) = parse_skk_dictionary_line(line) {
            dictionary.push(entry);
        }
    }
    dictionary
}

fn parse_skk_dictionary_line(line: &str) -> Option<SkkDictionaryEntry> {
    let line = line.trim();
    if line.is_empty() || line.starts_with(';') {
        return None;
    }

    let (key, body) = line.split_once(' ')?;
    if key.is_empty() {
        return None;
    }

    let mut candidates = Vec::new();
    for raw_candidate in body.split('/') {
        if let Some(candidate) = parse_skk_candidate(raw_candidate) {
            candidates.push(candidate);
        }
    }

    if candidates.is_empty() {
        None
    } else {
        Some(SkkDictionaryEntry {
            key: String::from(key),
            candidates,
        })
    }
}

fn parse_skk_candidate(raw_candidate: &str) -> Option<String> {
    let candidate = raw_candidate
        .split_once(';')
        .map_or(raw_candidate, |(text, _)| text);
    let candidate = candidate.trim();
    if candidate.is_empty() || candidate.starts_with('[') {
        None
    } else {
        Some(String::from(candidate))
    }
}

fn merge_duplicate_entries(dictionary: &mut Vec<SkkDictionaryEntry>) {
    let mut index = 0;
    while index + 1 < dictionary.len() {
        if dictionary[index].key == dictionary[index + 1].key {
            let duplicate = dictionary.remove(index + 1);
            for candidate in duplicate.candidates {
                if !dictionary[index].candidates.contains(&candidate) {
                    dictionary[index].candidates.push(candidate);
                }
            }
        } else {
            index += 1;
        }
    }
}

const ROMAJI_KANA: &[(&str, &str)] = &[
    ("kya", "きゃ"),
    ("kyu", "きゅ"),
    ("kyo", "きょ"),
    ("sha", "しゃ"),
    ("shu", "しゅ"),
    ("sho", "しょ"),
    ("sya", "しゃ"),
    ("syu", "しゅ"),
    ("syo", "しょ"),
    ("cha", "ちゃ"),
    ("chu", "ちゅ"),
    ("cho", "ちょ"),
    ("tya", "ちゃ"),
    ("tyu", "ちゅ"),
    ("tyo", "ちょ"),
    ("nya", "にゃ"),
    ("nyu", "にゅ"),
    ("nyo", "にょ"),
    ("hya", "ひゃ"),
    ("hyu", "ひゅ"),
    ("hyo", "ひょ"),
    ("mya", "みゃ"),
    ("myu", "みゅ"),
    ("myo", "みょ"),
    ("rya", "りゃ"),
    ("ryu", "りゅ"),
    ("ryo", "りょ"),
    ("gya", "ぎゃ"),
    ("gyu", "ぎゅ"),
    ("gyo", "ぎょ"),
    ("ja", "じゃ"),
    ("ji", "じ"),
    ("ju", "じゅ"),
    ("jo", "じょ"),
    ("jya", "じゃ"),
    ("jyu", "じゅ"),
    ("jyo", "じょ"),
    ("bya", "びゃ"),
    ("byu", "びゅ"),
    ("byo", "びょ"),
    ("pya", "ぴゃ"),
    ("pyu", "ぴゅ"),
    ("pyo", "ぴょ"),
    ("shi", "し"),
    ("chi", "ち"),
    ("tsu", "つ"),
    ("fu", "ふ"),
    ("ka", "か"),
    ("ki", "き"),
    ("ku", "く"),
    ("ke", "け"),
    ("ko", "こ"),
    ("sa", "さ"),
    ("si", "し"),
    ("su", "す"),
    ("se", "せ"),
    ("so", "そ"),
    ("ta", "た"),
    ("ti", "ち"),
    ("tu", "つ"),
    ("te", "て"),
    ("to", "と"),
    ("na", "な"),
    ("ni", "に"),
    ("nu", "ぬ"),
    ("ne", "ね"),
    ("no", "の"),
    ("ha", "は"),
    ("hi", "ひ"),
    ("hu", "ふ"),
    ("he", "へ"),
    ("ho", "ほ"),
    ("ma", "ま"),
    ("mi", "み"),
    ("mu", "む"),
    ("me", "め"),
    ("mo", "も"),
    ("ya", "や"),
    ("yu", "ゆ"),
    ("yo", "よ"),
    ("ra", "ら"),
    ("ri", "り"),
    ("ru", "る"),
    ("re", "れ"),
    ("ro", "ろ"),
    ("wa", "わ"),
    ("wo", "を"),
    ("ga", "が"),
    ("gi", "ぎ"),
    ("gu", "ぐ"),
    ("ge", "げ"),
    ("go", "ご"),
    ("za", "ざ"),
    ("zi", "じ"),
    ("zu", "ず"),
    ("ze", "ぜ"),
    ("zo", "ぞ"),
    ("da", "だ"),
    ("di", "ぢ"),
    ("du", "づ"),
    ("de", "で"),
    ("do", "ど"),
    ("ba", "ば"),
    ("bi", "び"),
    ("bu", "ぶ"),
    ("be", "べ"),
    ("bo", "ぼ"),
    ("pa", "ぱ"),
    ("pi", "ぴ"),
    ("pu", "ぷ"),
    ("pe", "ぺ"),
    ("po", "ぽ"),
    ("va", "ゔぁ"),
    ("vi", "ゔぃ"),
    ("vu", "ゔ"),
    ("ve", "ゔぇ"),
    ("vo", "ゔぉ"),
    ("a", "あ"),
    ("i", "い"),
    ("u", "う"),
    ("e", "え"),
    ("o", "お"),
];

fn preedit_spans(text: &str, phase: SkkPhase) -> Vec<u8> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut spans = Vec::new();
    spans.extend_from_slice(&0u32.to_le_bytes());
    spans.extend_from_slice(&(text.len() as u32).to_le_bytes());
    let style = match phase {
        SkkPhase::Candidate => sws_protocol::preedit_style::HIGHLIGHT,
        SkkPhase::Preedit => sws_protocol::preedit_style::UNDERLINE,
        SkkPhase::Direct => sws_protocol::preedit_style::UNDERLINE,
    };
    spans.extend_from_slice(&style.to_le_bytes());
    spans
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    println!("[simple_ime] connecting to SWS...");

    let mut conn = match Connection::connect_default() {
        Ok(conn) => conn,
        Err(err) => {
            println!("[simple_ime] failed to connect to SWS: {:?}", err);
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
            println!("[simple_ime] failed to register IME: {:?}", err);
            return 1;
        }
    };

    if let Err(err) = conn.set_active_input_method(ime_id) {
        println!("[simple_ime] failed to activate IME {}: {:?}", ime_id, err);
        return 1;
    }

    println!("[simple_ime] registered {} as id={}", IME_NAME, ime_id);

    let mut ime = SimpleIme::new(scale_milli);
    loop {
        match conn.dispatch() {
            Ok(_) => {
                while let Some(event) = conn.poll_event() {
                    if let Err(err) = ime.handle_event(&mut conn, event) {
                        println!("[simple_ime] event handling failed: {:?}", err);
                        return 1;
                    }
                }
            }
            Err(Error::WouldBlock) => {}
            Err(Error::Disconnected) => {
                println!("[simple_ime] disconnected from SWS");
                return 1;
            }
            Err(err) => {
                println!("[simple_ime] dispatch failed: {:?}", err);
                return 1;
            }
        }

        let _ = thread::sleep(Duration::from_millis(10));
    }
}
