//! Translate private seat actions into the broker's existing one-byte input stream.
use crate::greeter_ui::{Action, Screen};
use crate::keyboard_layout::{Action as KeyAction, Layer};
use sws_protocol::input_panel::SHIFT;

#[path = "soft_keyboard/modifiers.rs"]
mod modifiers;

pub struct PanelInput {
    modifiers: modifiers::Modifiers,
    clock: std::time::Instant,
}
impl PanelInput {
    pub fn new() -> Self {
        Self {
            modifiers: Default::default(),
            clock: std::time::Instant::now(),
        }
    }
    pub fn activate(&mut self, screen: &mut Screen, action: Action) -> Option<u8> {
        match action {
            Action::User | Action::Password => {
                screen.keyboard = true;
                return Some(if action == Action::User { 17 } else { 18 });
            }
            Action::Accounts => return Some(19),
            Action::Previous => return Some(20),
            Action::Next => return Some(21),
            Action::Manual | Action::Choose => {
                screen.keyboard = true;
                self.modifiers.change_layer();
                screen.shift = false;
                screen.layer = Layer::Letters;
                return Some(if action == Action::Choose { b'\n' } else { 22 });
            }
            Action::Submit => return Some(b'\n'),
            Action::Keyboard | Action::Key(KeyAction::Hide) => screen.keyboard = !screen.keyboard,
            Action::Key(KeyAction::SetLayer(
                layer @ (Layer::Letters | Layer::Symbols | Layer::MoreSymbols),
            )) => {
                screen.layer = layer;
                self.modifiers.change_layer();
                screen.shift = false;
            }
            Action::Key(KeyAction::Modifier(SHIFT)) => {
                let time = self.clock.elapsed().as_nanos().clamp(1, u64::MAX as u128) as u64;
                self.modifiers.toggle(SHIFT, time);
                screen.shift = self.modifiers.flags & SHIFT != 0;
            }
            Action::Key(KeyAction::Key(code, modifiers)) => {
                let shift = self.modifiers.flags & SHIFT != 0;
                self.modifiers.consume_stroke();
                screen.shift = self.modifiers.flags & SHIFT != 0;
                return key_byte(code, shift || modifiers & SHIFT != 0, false);
            }
            _ => {}
        }
        None
    }
}

// US evdev layout for the trusted login prompt. IMEs belong to the desktop.
pub fn key_byte(code: u16, shift: bool, caps: bool) -> Option<u8> {
    let (normal, shifted, start): (&[u8], &[u8], u16) = match code {
        1 => return Some(27),
        14 => return Some(8),
        15 => return Some(b'\t'),
        28 | 96 => return Some(b'\n'),
        57 => return Some(b' '),
        2..=13 => (b"1234567890-=", b"!@#$%^&*()_+", 2),
        16..=27 => (b"qwertyuiop[]", b"QWERTYUIOP{}", 16),
        30..=41 => (b"asdfghjkl;'\x60", b"ASDFGHJKL:\"~", 30),
        43 => return Some(if shift { b'|' } else { b'\\' }),
        44..=53 => (b"zxcvbnm,./", b"ZXCVBNM<>?", 44),
        _ => return None,
    };
    let index = usize::from(code - start);
    Some(if shift ^ (caps && normal[index].is_ascii_alphabetic()) {
        shifted[index]
    } else {
        normal[index]
    })
}
