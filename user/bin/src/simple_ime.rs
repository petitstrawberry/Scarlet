//! Simple external IME service for SWS text-input protocol.
//!
//! This binary intentionally keeps conversion outside SWS. SWS only brokers
//! text-input state and key events between applications and this process.

#![no_std]
#![no_main]

extern crate alloc;
extern crate scarlet_std as std;

use alloc::string::String;
use alloc::vec::Vec;
use core::time::Duration;
use std::println;
use std::thread;
use sws_client::{Connection, Error, Event, event_type, key_code};
use sws_protocol::{ime_capabilities, ime_state, ime_status_flags, ime_trigger};

const IME_NAME: &str = "simple-kana";
const MODE_DIRECT_ID: u32 = 0;
const MODE_SIMPLE_KANA_ID: u32 = 1;
const MODE_DIRECT_LABEL: &str = "Direct";
const MODE_SIMPLE_KANA_LABEL: &str = "Simple Kana";

struct SimpleIme {
    active_context_id: Option<u32>,
    grabbing: bool,
    pending: Vec<u8>,
    eaten_keys: Vec<u16>,
}

impl SimpleIme {
    fn new() -> Self {
        Self {
            active_context_id: None,
            grabbing: false,
            pending: Vec::new(),
            eaten_keys: Vec::new(),
        }
    }

    fn reset_context(&mut self) {
        self.pending.clear();
        self.eaten_keys.clear();
    }

    fn handle_event(&mut self, conn: &mut Connection, event: Event) -> Result<(), Error> {
        match event {
            Event::ImeActivate(state) => {
                self.reset_context();
                self.active_context_id = Some(state.context_id);
                self.grabbing = false;
                conn.ime_set_preedit(state.context_id, 0, 0, "", &[])?;
                conn.ime_hide_candidates(state.context_id)?;
                self.emit_status(conn, state.context_id)?;
                println!(
                    "[simple_ime] activated context={} window={}",
                    state.context_id, state.window_id
                );
            }
            Event::ImeDeactivate { context_id, .. } => {
                if self.active_context_id == Some(context_id) {
                    self.active_context_id = None;
                    self.grabbing = false;
                    self.reset_context();
                }
                println!("[simple_ime] deactivated context={}", context_id);
            }
            Event::ImeContextState(state) => {
                self.active_context_id = Some(state.context_id);
                println!(
                    "[simple_ime] context-state context={} serial={} cursor=({}, {}) purpose={} pending='{}' grabbing={}",
                    state.context_id,
                    state.serial,
                    state.cursor_x,
                    state.cursor_y,
                    state.content_purpose,
                    self.pending_text(),
                    self.grabbing
                );
            }
            Event::ImeReset { context_id, .. } => {
                if self.active_context_id == Some(context_id) {
                    println!("[simple_ime] reset context={}", context_id);
                    self.grabbing = false;
                    self.reset_context();
                    conn.ime_set_preedit(context_id, 0, 0, "", &[])?;
                    conn.ime_hide_candidates(context_id)?;
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
                    "[simple_ime] key-event context={} serial={} {}({}) type={} value={} grabbing={} pending='{}'",
                    context_id,
                    key_serial,
                    key_name(code),
                    code,
                    type_,
                    value,
                    self.grabbing,
                    self.pending_text()
                );
                let handled = self.handle_key(conn, context_id, type_, code, value)?;
                conn.ime_key_handled(key_serial, handled)?;
                println!(
                    "[simple_ime] key-handled serial={} handled={} pending='{}'",
                    key_serial,
                    handled,
                    self.pending_text()
                );
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

        let handled = if let Some(ch) = letter_from_key(code) {
            self.pending.push(ch as u8);
            println!(
                "[simple_ime] append '{}' -> pending='{}'",
                ch,
                self.pending_text()
            );
            self.commit_ready_text(conn, context_id)?;
            self.update_preedit(conn, context_id)?;
            true
        } else {
            match code {
                key_code::KEY_BACKSPACE => {
                    if self.pending.pop().is_some() {
                        self.update_preedit(conn, context_id)?;
                        true
                    } else {
                        false
                    }
                }
                key_code::KEY_ENTER | key_code::KEY_SPACE => {
                    if self.pending.is_empty() {
                        false
                    } else {
                        self.flush_pending(conn, context_id)?;
                        true
                    }
                }
                key_code::KEY_COMMA => {
                    self.flush_pending(conn, context_id)?;
                    println!("[simple_ime] commit punctuation '、'");
                    conn.ime_commit_text(context_id, "、")?;
                    true
                }
                key_code::KEY_DOT => {
                    self.flush_pending(conn, context_id)?;
                    println!("[simple_ime] commit punctuation '。'");
                    conn.ime_commit_text(context_id, "。")?;
                    true
                }
                key_code::KEY_SLASH => {
                    self.flush_pending(conn, context_id)?;
                    println!("[simple_ime] commit punctuation '・'");
                    conn.ime_commit_text(context_id, "・")?;
                    true
                }
                _ => false,
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
            self.flush_pending(conn, context_id)?;
            conn.ime_release_keyboard(context_id)?;
            self.grabbing = false;
            self.reset_context();
            conn.ime_set_preedit(context_id, 0, 0, "", &[])?;
            conn.ime_hide_candidates(context_id)?;
            self.emit_status(conn, context_id)?;
            println!("[simple_ime] released keyboard context={}", context_id);
            return Ok(());
        }

        println!("[simple_ime] toggle: grab requested context={}", context_id);
        self.reset_context();
        self.grabbing = true;
        conn.ime_grab_keyboard(context_id)?;
        conn.ime_set_preedit(context_id, 0, 0, "", &[])?;
        conn.ime_hide_candidates(context_id)?;
        self.emit_status(conn, context_id)?;
        println!("[simple_ime] grabbed keyboard context={}", context_id);
        Ok(())
    }

    fn commit_ready_text(&mut self, conn: &mut Connection, context_id: u32) -> Result<(), Error> {
        let text = drain_committable(&mut self.pending);
        if !text.is_empty() {
            println!(
                "[simple_ime] commit-ready '{}' remaining='{}'",
                text,
                self.pending_text()
            );
            conn.ime_commit_text(context_id, &text)?;
        }
        Ok(())
    }

    fn flush_pending(&mut self, conn: &mut Connection, context_id: u32) -> Result<(), Error> {
        let mut text = drain_committable(&mut self.pending);
        if self.pending == b"n" {
            text.push_str("ん");
        } else if !self.pending.is_empty()
            && let Ok(raw) = core::str::from_utf8(&self.pending)
        {
            text.push_str(raw);
        }
        self.pending.clear();

        if !text.is_empty() {
            println!("[simple_ime] flush commit '{}'", text);
            conn.ime_commit_text(context_id, &text)?;
        }
        self.update_preedit(conn, context_id)
    }

    fn update_preedit(&self, conn: &mut Connection, context_id: u32) -> Result<(), Error> {
        let preedit = core::str::from_utf8(&self.pending).unwrap_or("");
        println!("[simple_ime] preedit '{}'", preedit);
        let spans = preedit_spans(preedit);
        conn.ime_set_preedit(context_id, preedit.len() as u32, 0, preedit, &spans)?;
        if preedit.is_empty() {
            conn.ime_hide_candidates(context_id)?;
        } else {
            let candidates = candidate_list_blob(preedit);
            conn.ime_set_candidates(context_id, 0, 0, 1, 0, &candidates)?;
        }
        self.emit_status(conn, context_id)?;
        Ok(())
    }

    fn emit_status(&self, conn: &mut Connection, context_id: u32) -> Result<(), Error> {
        let has_preedit = !self.pending.is_empty();
        let state = if has_preedit {
            ime_state::CANDIDATES
        } else {
            ime_state::DIRECT
        };
        let (mode_id, mode_label) = if self.grabbing {
            (MODE_SIMPLE_KANA_ID, MODE_SIMPLE_KANA_LABEL)
        } else {
            (MODE_DIRECT_ID, MODE_DIRECT_LABEL)
        };
        let mut flags = 0;
        if self.grabbing {
            flags |= ime_status_flags::MODE_ACTIVE;
        }
        if has_preedit {
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

    fn remove_eaten_key(&mut self, code: u16) -> bool {
        let Some(index) = self.eaten_keys.iter().position(|key| *key == code) else {
            return false;
        };
        self.eaten_keys.remove(index);
        true
    }
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

fn key_name(code: u16) -> &'static str {
    match code {
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
        key_code::KEY_COMMA => "KEY_COMMA",
        key_code::KEY_DOT => "KEY_DOT",
        key_code::KEY_SLASH => "KEY_SLASH",
        _ => "KEY_UNKNOWN",
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

fn candidate_list_blob(text: &str) -> Vec<u8> {
    let mut blob = Vec::new();
    blob.extend_from_slice(&1u32.to_le_bytes());
    append_candidate(&mut blob, 0, "", text, "", "", 0);
    blob
}

fn append_candidate(
    blob: &mut Vec<u8>,
    id: u32,
    label: &str,
    text: &str,
    annotation: &str,
    comment: &str,
    flags: u32,
) {
    blob.extend_from_slice(&id.to_le_bytes());
    append_string(blob, label);
    append_string(blob, text);
    append_string(blob, annotation);
    append_string(blob, comment);
    blob.extend_from_slice(&flags.to_le_bytes());
}

fn append_string(blob: &mut Vec<u8>, value: &str) {
    blob.extend_from_slice(&(value.len() as u32).to_le_bytes());
    blob.extend_from_slice(value.as_bytes());
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

    let capabilities = ime_capabilities::KEYBOARD_GRAB
        | ime_capabilities::STYLED_PREEDIT
        | ime_capabilities::CANDIDATE_LIST
        | ime_capabilities::STATUS;
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

    let mut ime = SimpleIme::new();
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
