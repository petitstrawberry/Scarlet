//! Encode UI keyboard events for the PTY. View-only actions stay in TerminalApp.
use scarlet_ui::{KeyCode, KeyEvent};

pub(super) fn font_size_delta(event: KeyEvent) -> Option<f32> {
    let KeyEvent::Pressed { keycode, modifiers } = event else {
        return None;
    };
    if !modifiers.control || !modifiers.shift || modifiers.alt || modifiers.super_key {
        return None;
    }
    match keycode {
        KeyCode::Char('=' | '+') => Some(1.0),
        KeyCode::Char('-' | '_') => Some(-1.0),
        _ => None,
    }
}

pub(super) fn write_key(event: KeyEvent, mut write: impl FnMut(&[u8])) -> bool {
    match event {
        KeyEvent::Char { c } => {
            let mut encoded = [0u8; 4];
            write(c.encode_utf8(&mut encoded).as_bytes());
        }
        KeyEvent::Pressed {
            keycode: KeyCode::F(number),
            modifiers,
        } => {
            let mut encoded = [0u8; 8];
            let parameter = 1
                + u8::from(modifiers.shift)
                + 2 * u8::from(modifiers.alt)
                + 4 * u8::from(modifiers.control);
            let (prefix, suffix): (&[u8], u8) = match number {
                1..=4 if parameter == 1 => (b"\x1bO", b'P' + number - 1),
                1..=4 => (b"\x1b[1", b'P' + number - 1),
                5 => (b"\x1b[15", b'~'),
                6 => (b"\x1b[17", b'~'),
                7 => (b"\x1b[18", b'~'),
                8 => (b"\x1b[19", b'~'),
                9 => (b"\x1b[20", b'~'),
                10 => (b"\x1b[21", b'~'),
                11 => (b"\x1b[23", b'~'),
                12 => (b"\x1b[24", b'~'),
                _ => return false,
            };
            encoded[..prefix.len()].copy_from_slice(prefix);
            let mut length = prefix.len();
            if parameter != 1 {
                encoded[length] = b';';
                encoded[length + 1] = b'0' + parameter;
                length += 2;
            }
            encoded[length] = suffix;
            write(&encoded[..length + 1]);
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
