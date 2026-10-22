//! Tests the production session store independently of compositor/GPU hardware.
#[path = "../../user/std-bin/src/sws/clipboard.rs"]
pub mod clipboard;
#[path = "../../user/std-bin/src/sws/window_title.rs"]
mod window_title;

#[cfg(test)]
mod tests {
    use sws_protocol::{clipboard::*, *};
    #[test]
    fn routed_frame_round_trip_preserves_request_identity_and_unicode() {
        let request = Request::Set {
            window_id: 42,
            text: "日本 e\u{301} 👩‍💻",
        };
        let (kind, payload) = request.encode().unwrap();
        let header = MessageHeader::request(kind, 17, payload.len() as u32);
        let header = MessageHeader::from_le_bytes(header.to_le_bytes());
        assert_eq!(header.request_id, 17);
        assert_eq!(
            parse_client_message(header.msg_type_u32(), &payload).unwrap(),
            ClientMessageRef::Clipboard(request)
        );
        let response = Response::encode(OK, "日本 e\u{301} 👩‍💻").unwrap();
        assert!(matches!(
            parse_server_message(server_msg::CLIPBOARD_TEXT, &response).unwrap(),
            ServerMessage::ClipboardText(_)
        ));
    }
    #[test]
    fn strict_wire_limits_and_invalid_utf8() {
        let text = "x".repeat(MAX_TEXT_BYTES);
        assert!(
            Request::Set {
                window_id: 42,
                text: &text
            }
            .encode()
            .is_ok()
        );
        assert!(
            Request::Set {
                window_id: 42,
                text: &(text + "x")
            }
            .encode()
            .is_err()
        );
        let mut response = Response::encode(OK, "x").unwrap();
        response[8] = 0xff;
        assert!(parse_server_message(server_msg::CLIPBOARD_TEXT, &response).is_err());
        assert!(Request::parse(client_msg::GET_CLIPBOARD_TEXT, &[0; 5]).is_err());
    }
}
