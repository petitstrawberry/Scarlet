#![cfg(feature = "std")]

use sws_protocol::{
    ClientMessageRef, LEGACY_MAX_MENU_TITLES_BYTES, MAX_MENU_TITLES_BYTES, ProtocolError,
    ServerMessage, client_msg, menu_titles_for_client, parse_client_message, parse_server_message,
    payload_active_app, payload_active_app_changed, payload_focus_changed,
    payload_register_system_shell, server_msg,
};

const KINDS: [u32; 3] = [
    server_msg::FOCUS_CHANGED,
    server_msg::ACTIVE_APP_CHANGED,
    server_msg::ACTIVE_APP,
];

fn build(kind: u32, menu: &[u8]) -> Result<Vec<u8>, ProtocolError> {
    match kind {
        server_msg::FOCUS_CHANGED => payload_focus_changed(42, b"app", b"App", b"Title", menu),
        server_msg::ACTIVE_APP_CHANGED => {
            payload_active_app_changed(42, b"app", b"App", b"Title", menu)
        }
        server_msg::ACTIVE_APP => payload_active_app(b"app", b"App", menu),
        _ => unreachable!(),
    }
}

fn menu(kind: u32, payload: &[u8]) -> Vec<u8> {
    let (bytes, len) = match parse_server_message(kind, payload).unwrap() {
        ServerMessage::FocusChanged {
            menu_titles,
            menu_titles_len,
            ..
        }
        | ServerMessage::ActiveAppChanged {
            menu_titles,
            menu_titles_len,
            ..
        }
        | ServerMessage::ActiveApp {
            menu_titles,
            menu_titles_len,
            ..
        } => (menu_titles, menu_titles_len),
        _ => panic!("wrong message"),
    };
    assert_eq!(bytes.len(), len as usize);
    bytes
}

#[test]
fn complete_unicode_json_over_2048_bytes_round_trips_in_all_messages() {
    let items = (0..120)
        .map(|id| format!(r#"{{"id":"tool-{id}","label":"日本語の道具 🖌️ {id}"}}"#))
        .collect::<Vec<_>>()
        .join(",");
    let json = format!(r#"{{"menus":[{{"label":"すべての道具","items":[{items}]}}]}}"#);
    assert!(json.len() > LEGACY_MAX_MENU_TITLES_BYTES);
    assert!(json.len() < MAX_MENU_TITLES_BYTES);
    for kind in KINDS {
        let bytes = menu(kind, &build(kind, json.as_bytes()).unwrap());
        assert_eq!(std::str::from_utf8(&bytes).unwrap(), json);
        assert!(
            bytes
                .windows(b"tool-119".len())
                .any(|part| part == b"tool-119")
        );
    }
}

#[test]
fn exact_64k_and_empty_menus_round_trip_but_oversized_builders_fail() {
    // A complete JSON string exactly at the limit, ending with a multibyte character.
    let exact = format!("\"{}日\"", "x".repeat(MAX_MENU_TITLES_BYTES - 5));
    assert_eq!(exact.len(), MAX_MENU_TITLES_BYTES);
    for kind in KINDS {
        for bytes in [b"".as_slice(), exact.as_bytes()] {
            assert_eq!(menu(kind, &build(kind, bytes).unwrap()), bytes);
        }
        assert_eq!(
            build(kind, &vec![b'x'; MAX_MENU_TITLES_BYTES + 1]),
            Err(ProtocolError::MalformedPayload)
        );
    }
    // Menu storage no longer grows the enum's stack footprint by the menu bound.
    assert!(std::mem::size_of::<ServerMessage>() < MAX_MENU_TITLES_BYTES);
}

#[test]
fn truncated_trailing_oversized_and_overflowing_lengths_are_errors() {
    for kind in KINDS {
        let valid = build(kind, br#"{"menus":[]}"#).unwrap();
        for len in 0..valid.len() {
            assert_eq!(
                parse_server_message(kind, &valid[..len]),
                Err(ProtocolError::MalformedPayload)
            );
        }
        let mut trailing = valid.clone();
        trailing.push(0);
        assert_eq!(
            parse_server_message(kind, &trailing),
            Err(ProtocolError::MalformedPayload)
        );

        let limits: &[usize] = if kind == server_msg::ACTIVE_APP {
            &[128, 128, MAX_MENU_TITLES_BYTES]
        } else {
            &[128, 128, 256, MAX_MENU_TITLES_BYTES]
        };
        for (bad_field, limit) in limits.iter().enumerate() {
            for declared in [(*limit + 1) as u32, u32::MAX] {
                // Test with both missing bytes and enough bytes to reach the old
                // unchecked fixed-array copy (including a 65537-byte menu).
                for with_body in [false, true] {
                    if with_body && declared == u32::MAX {
                        continue;
                    }
                    let mut malformed = Vec::new();
                    if kind != server_msg::ACTIVE_APP {
                        malformed.extend_from_slice(&42u32.to_le_bytes());
                    }
                    for field in 0..limits.len() {
                        if field == bad_field {
                            malformed.extend_from_slice(&declared.to_le_bytes());
                            if with_body {
                                malformed.resize(malformed.len() + declared as usize, b'x');
                            }
                        } else {
                            malformed.extend_from_slice(&0u32.to_le_bytes());
                        }
                    }
                    assert_eq!(
                        parse_server_message(kind, &malformed),
                        Err(ProtocolError::MalformedPayload)
                    );
                }
            }
        }
    }
}

#[test]
fn legacy_wire_layout_and_complete_small_menus_are_preserved() {
    let bytes = [
        42, 0, 0, 0, // window
        1, 0, 0, 0, b'a', // app ID
        1, 0, 0, 0, b'A', // app name
        1, 0, 0, 0, b'T', // title
        2, 0, 0, 0, b'[', b']', // menu
    ];
    assert_eq!(
        payload_focus_changed(42, b"a", b"A", b"T", b"[]").unwrap(),
        bytes
    );
    assert_eq!(
        payload_active_app_changed(42, b"a", b"A", b"T", b"[]").unwrap(),
        bytes
    );
    assert_eq!(
        payload_active_app(b"a", b"A", b"[]").unwrap(),
        [1, 0, 0, 0, b'a', 1, 0, 0, 0, b'A', 2, 0, 0, 0, b'[', b']',]
    );
    for len in [
        0,
        2048,
        2049,
        MAX_MENU_TITLES_BYTES,
        MAX_MENU_TITLES_BYTES + 1,
    ] {
        let original = vec![b'x'; len];
        for opted_in in [false, true] {
            let limit = if opted_in {
                MAX_MENU_TITLES_BYTES
            } else {
                2048
            };
            let selected = menu_titles_for_client(&original, opted_in);
            assert_eq!(selected, if len <= limit { &original[..] } else { b"" });
            for kind in KINDS {
                let decoded = menu(kind, &build(kind, selected).unwrap());
                assert_eq!(decoded, selected);
                if !opted_in {
                    // The original parser's fixed-array copy remains safe.
                    let mut old_buffer = [0u8; 2048];
                    old_buffer[..decoded.len()].copy_from_slice(&decoded);
                }
            }
        }
    }
}

#[test]
fn full_menus_require_explicit_shell_registration_opt_in() {
    assert!(payload_register_system_shell(false).is_empty());
    for opted_in in [false, true] {
        assert_eq!(
            parse_client_message(
                client_msg::REGISTER_SYSTEM_SHELL,
                &payload_register_system_shell(opted_in)
            ),
            Ok(ClientMessageRef::RegisterSystemShell {
                full_app_menus: opted_in
            })
        );
    }
    assert_eq!(
        parse_client_message(client_msg::REGISTER_SYSTEM_SHELL, &[0; 4]),
        Ok(ClientMessageRef::RegisterSystemShell {
            full_app_menus: false
        })
    );
    for invalid in [
        &[1][..],
        &[1, 0],
        &[1, 0, 0],
        &[2, 0, 0, 0],
        &[1, 0, 0, 0, 0],
        &[255; 4],
    ] {
        assert_eq!(
            parse_client_message(client_msg::REGISTER_SYSTEM_SHELL, invalid),
            Err(ProtocolError::MalformedPayload)
        );
    }
}
