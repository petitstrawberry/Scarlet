//! Encode UI keyboard events for the PTY. View-only actions stay in TerminalApp.
use scarlet_ui::{KeyCode, KeyEvent};

pub(super) fn write_key(event: KeyEvent, mut write: impl FnMut(&[u8])) -> bool {
    match event {
        KeyEvent::Char { c } => {
            let mut encoded = [0u8; 4];
            write(c.encode_utf8(&mut encoded).as_bytes());
        }
        KeyEvent::Pressed { keycode, modifiers }
            if modifiers.control && !modifiers.super_key && control_byte(keycode).is_some() =>
        {
            // Shortcut events preserve the physical letter and carry Ctrl in
            // modifiers; SWS does not emit a corresponding Char event.
            if modifiers.alt {
                write(b"\x1b");
            }
            write(&[control_byte(keycode).unwrap()]);
        }
        KeyEvent::Pressed { keycode, .. } => match keycode {
            KeyCode::Char(c) if c.is_control() => {
                let mut encoded = [0u8; 4];
                write(c.encode_utf8(&mut encoded).as_bytes());
            }
            KeyCode::Enter => write(b"\r"),
            KeyCode::Tab => write(b"\t"),
            KeyCode::Backspace => write(&[0x7f]),
            KeyCode::Escape => write(&[0x1b]),
            KeyCode::Left => write(b"\x1b[D"),
            KeyCode::Right => write(b"\x1b[C"),
            KeyCode::Up => write(b"\x1b[A"),
            KeyCode::Down => write(b"\x1b[B"),
            KeyCode::Home => write(b"\x1b[H"),
            KeyCode::End => write(b"\x1b[F"),
            KeyCode::Insert => write(b"\x1b[2~"),
            KeyCode::Delete => write(b"\x1b[3~"),
            KeyCode::F(1) => write(b"\x1bOP"),
            KeyCode::F(2) => write(b"\x1bOQ"),
            KeyCode::F(3) => write(b"\x1bOR"),
            KeyCode::F(4) => write(b"\x1bOS"),
            _ => return false,
        },
        KeyEvent::Released { .. } => {}
    }
    true
}

fn control_byte(keycode: KeyCode) -> Option<u8> {
    match keycode {
        KeyCode::Char(c) if c.is_ascii_alphabetic() => {
            Some(c.to_ascii_lowercase() as u8 - b'a' + 1)
        }
        KeyCode::Space | KeyCode::Char(' ' | '@' | '2') => Some(0x00),
        KeyCode::Char('[') => Some(0x1b),
        KeyCode::Char('\\') => Some(0x1c),
        KeyCode::Char(']') => Some(0x1d),
        KeyCode::Char('^' | '6') => Some(0x1e),
        KeyCode::Char('_' | '-' | '7') => Some(0x1f),
        KeyCode::Char('?' | '8') => Some(0x7f),
        _ => None,
    }
}
