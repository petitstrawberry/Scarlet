#[path = "../../../user/bin/src/terminal/keyboard.rs"]
mod keyboard;

use scarlet_ui::{
    Color, Event, Size, State, StateId, TextGrid, TextGridBuffer, TextGridCell, View, ViewExt,
    pipeline::RenderingPipeline,
};
use scarlet_ui::{KeyCode, KeyEvent, event::KeyModifiers};
use std::cell::RefCell;
use std::rc::Rc;

fn press(keycode: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
    KeyEvent::Pressed { keycode, modifiers }
}

#[test]
fn ctrl_c_writes_one_interrupt_byte_without_a_char_event() {
    // SWS delivers a physical key and modifiers. Shortcut presses deliberately
    // have no Char event, so the PTY writer must translate the Pressed event.
    let modifiers = KeyModifiers {
        control: true,
        ..Default::default()
    };
    let mut sent = Vec::new();
    for event in [
        press(KeyCode::Char('c'), modifiers),
        KeyEvent::Released {
            keycode: KeyCode::Char('c'),
            modifiers,
        },
    ] {
        keyboard::write_key(event, |bytes| sent.extend_from_slice(bytes));
    }
    eprintln!("Ctrl+C PTY bytes: {sent:02x?}; expected [03]");
    assert_eq!(
        sent, b"\x03",
        "Ctrl+C must send ETX to the PTY exactly once"
    );
}

#[test]
fn terminal_text_grid_dispatch_sends_ctrl_c_to_the_writer() {
    let sent = Rc::new(RefCell::new(Vec::new()));
    let output = sent.clone();
    let grid = State::new(
        StateId::new(900),
        TextGridBuffer::new(80, 24, TextGridCell::blank(Color::WHITE, Color::BLACK)),
    );
    let view = TextGrid::new(grid).on_key(move |event| {
        keyboard::write_key(event, |bytes| output.borrow_mut().extend_from_slice(bytes))
    });
    let mut pipeline = RenderingPipeline::new();
    pipeline.set_root(view.create_element());
    pipeline.layout_initial();
    pipeline.resize(Size::new(800.0, 480.0));
    let modifiers = KeyModifiers {
        control: true,
        ..Default::default()
    };
    for event in [
        Event::ModifiersChanged(modifiers),
        Event::Keyboard(press(KeyCode::Char('c'), modifiers)),
        Event::Keyboard(KeyEvent::Released {
            keycode: KeyCode::Char('c'),
            modifiers,
        }),
        Event::ModifiersChanged(KeyModifiers::default()),
    ] {
        pipeline.handle_event(&event);
    }
    assert_eq!(
        *sent.borrow(),
        b"\x03",
        "mounted Terminal grid must send Ctrl+C"
    );
}

#[test]
fn control_letters_and_terminal_control_symbols_write_bytes() {
    let modifiers = KeyModifiers {
        control: true,
        ..Default::default()
    };
    for letter in b'a'..=b'z' {
        for shift in [false, true] {
            let mut sent = Vec::new();
            keyboard::write_key(
                press(
                    KeyCode::Char(letter as char),
                    KeyModifiers { shift, ..modifiers },
                ),
                |bytes| sent.extend_from_slice(bytes),
            );
            assert_eq!(sent, [letter - b'a' + 1]);
        }
    }
    for (key, expected) in [
        (KeyCode::Space, 0),
        (KeyCode::Char('['), 0x1b),
        (KeyCode::Char('\\'), 0x1c),
        (KeyCode::Char(']'), 0x1d),
        (KeyCode::Char('6'), 0x1e),
        (KeyCode::Char('-'), 0x1f),
    ] {
        let mut sent = Vec::new();
        keyboard::write_key(press(key, modifiers), |bytes| sent.extend_from_slice(bytes));
        assert_eq!(sent, [expected]);
    }
}

#[test]
fn plain_text_and_existing_terminal_keys_are_not_duplicated() {
    let modifiers = KeyModifiers::default();
    let mut sent = Vec::new();
    for event in [
        press(KeyCode::Char('c'), modifiers),
        KeyEvent::Char { c: 'c' },
        KeyEvent::Released {
            keycode: KeyCode::Char('c'),
            modifiers,
        },
        KeyEvent::Char { c: 'あ' },
        press(KeyCode::Enter, modifiers),
        press(KeyCode::Tab, modifiers),
        press(KeyCode::Backspace, modifiers),
        press(KeyCode::Left, modifiers),
    ] {
        keyboard::write_key(event, |bytes| sent.extend_from_slice(bytes));
    }
    assert_eq!(sent, "cあ\r\t\x7f\x1b[D".as_bytes());
}

#[test]
fn alt_control_prefixes_escape_and_view_shortcuts_do_not_write() {
    let mut sent = Vec::new();
    keyboard::write_key(
        press(
            KeyCode::Char('c'),
            KeyModifiers {
                control: true,
                alt: true,
                ..Default::default()
            },
        ),
        |bytes| sent.extend_from_slice(bytes),
    );
    assert_eq!(sent, b"\x1b\x03");
    for key in [
        KeyCode::PageUp,
        KeyCode::PageDown,
        KeyCode::F(11),
        KeyCode::F(12),
    ] {
        assert!(!keyboard::write_key(
            press(key, KeyModifiers::default()),
            |_| panic!("view action wrote to PTY")
        ));
    }
}
