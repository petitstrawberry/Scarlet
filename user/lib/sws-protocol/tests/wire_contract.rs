//! Golden wire fixtures for the current SWS matched server/client contract.
//!
//! These test literal bytes in both directions, not just encoder/decoder round
//! trips. They do not promise compatibility with independently upgraded SWS
//! clients or establish compositor resource-release behavior.

#![cfg(feature = "std")]

use sws_protocol::{
    ClientMessageRef, MessageHeader, ProtocolError, SGFX_MAX_DAMAGE_RECTS, SWS_PROTOCOL_VERSION,
    ServerMessage, SgfxDamageRect, capabilities, client_msg, encode_routed_frame,
    parse_client_message, parse_server_message, parse_sgfx_damage_rects, payload_capabilities,
    payload_commit_sgfx_frame, payload_sgfx_buffer_released, server_msg,
};

const COMMIT: [u8; 44] = [
    1, 2, 3, 4, // window
    5, 6, 7, 8, // buffer
    9, 10, 11, 12, // generation
    13, 14, 15, 16, // epoch
    17, 18, 19, 20, 21, 22, 23, 24, // serial
    1, 0, 0, 0, // rectangle count
    254, 255, 255, 255, // x = -2
    3, 0, 0, 0, // y
    16, 0, 0, 0, // width
    32, 0, 0, 0, // height
];

#[test]
fn request_routing_has_a_fixed_eight_byte_header() {
    assert_eq!(MessageHeader::SIZE, 8);
    assert_eq!(MessageHeader::FLAG_IS_RESPONSE, 1);
    let bytes = [0x34, 0x12, 1, 0xa5, 4, 3, 2, 1];
    let header = MessageHeader::from_le_bytes(bytes);
    assert_eq!(header.msg_type_u32(), 0x1234);
    assert_eq!(header.request_id, 0xa5);
    assert_eq!(header.payload_size, 0x0102_0304);
    assert!(header.is_response());
    assert_eq!(
        MessageHeader::response(0x1234, 0xa5, 0x0102_0304).to_le_bytes(),
        bytes
    );
    assert_eq!(
        encode_routed_frame(0x1234, 1, 0xa5, &[0xfe, 0xdc]),
        [0x34, 0x12, 1, 0xa5, 2, 0, 0, 0, 0xfe, 0xdc],
    );
}

#[test]
fn capability_version_bits_and_payload_are_independent_of_package_version() {
    assert_eq!(SWS_PROTOCOL_VERSION, 9);
    assert_eq!(client_msg::GET_CAPABILITIES, 32);
    assert_eq!(server_msg::CAPABILITIES, 25);
    assert_eq!(
        [
            capabilities::SGFX_SHARED_IMAGE,
            capabilities::POINTER_LOCK,
            capabilities::CURSOR_ICONS,
            capabilities::CURSOR_THEMES,
            capabilities::INPUT_ENVIRONMENT,
            capabilities::WINDOW_GEOMETRY,
            capabilities::SYSTEM_MODE_OVERRIDES,
            capabilities::CONFIGURED_WINDOW_CREATION,
            capabilities::WORKSPACE_SHELL,
            capabilities::FRAME_CALLBACKS,
            capabilities::EXTENSION_BUFFER_OBJECTS,
        ],
        [1, 2, 4, 8, 16, 32, 64, 128, 256, 512, 1024],
    );
    // Unknown feature bits survive decoding; they must not become known support.
    let bytes = [9, 0, 0, 0, 1, 4, 0, 0, 0, 0, 0, 128, 4, 3, 2, 1, 2, 0, 0, 0];
    let flags = 0x8000_0000_0000_0401;
    assert_eq!(payload_capabilities(9, flags, 0x0102_0304, 2), bytes);
    assert_eq!(
        parse_server_message(25, &bytes),
        Ok(ServerMessage::Capabilities {
            protocol_version: 9,
            capabilities: flags,
            compositor_epoch: 0x0102_0304,
            compositor_backend: 2,
        }),
    );
    assert_eq!(
        parse_client_message(32, &[]),
        Ok(ClientMessageRef::GetCapabilities {})
    );
    assert_eq!(
        parse_client_message(32, &[0]),
        Err(ProtocolError::MalformedPayload)
    );
    for len in 0..bytes.len() {
        assert_eq!(
            parse_server_message(25, &bytes[..len]),
            Err(ProtocolError::MalformedPayload)
        );
    }
    let mut oversized = bytes.to_vec();
    oversized.push(0);
    assert_eq!(
        parse_server_message(25, &oversized),
        Err(ProtocolError::MalformedPayload)
    );
}

#[test]
fn shared_frame_commit_preserves_the_full_buffer_use_identity() {
    assert_eq!(client_msg::COMMIT_SGFX_FRAME, 34);
    let damage = [SgfxDamageRect {
        x: -2,
        y: 3,
        width: 16,
        height: 32,
    }];
    assert_eq!(
        payload_commit_sgfx_frame(
            0x0403_0201,
            0x0807_0605,
            0x0c0b_0a09,
            0x100f_0e0d,
            0x1817_1615_1413_1211,
            &damage,
        )
        .unwrap(),
        COMMIT,
    );
    let ClientMessageRef::CommitSgfxFrame {
        window_id,
        buffer_id,
        generation,
        compositor_epoch,
        commit_serial,
        damage_rects,
    } = parse_client_message(34, &COMMIT).unwrap()
    else {
        panic!("expected shared SGFX commit");
    };
    assert_eq!((window_id, buffer_id), (0x0403_0201, 0x0807_0605));
    assert_eq!((generation, compositor_epoch), (0x0c0b_0a09, 0x100f_0e0d));
    assert_eq!(commit_serial, 0x1817_1615_1413_1211);
    assert_eq!(parse_sgfx_damage_rects(damage_rects).unwrap(), damage);
}

#[test]
fn malformed_shared_frame_commits_are_rejected() {
    for len in 0..COMMIT.len() {
        assert_eq!(
            parse_client_message(34, &COMMIT[..len]),
            Err(ProtocolError::MalformedPayload)
        );
    }
    let mut oversized = COMMIT.to_vec();
    oversized.push(0);
    assert_eq!(
        parse_client_message(34, &oversized),
        Err(ProtocolError::MalformedPayload)
    );

    let mut zero_serial = COMMIT;
    zero_serial[16..24].fill(0);
    assert_eq!(
        parse_client_message(34, &zero_serial),
        Err(ProtocolError::MalformedPayload)
    );
    for count in [0_u32, 2, 17, u32::MAX] {
        let mut bad_count = COMMIT;
        bad_count[24..28].copy_from_slice(&count.to_le_bytes());
        assert_eq!(
            parse_client_message(34, &bad_count),
            Err(ProtocolError::MalformedPayload)
        );
    }
}

#[test]
fn damage_count_limits_apply_to_both_encoding_and_decoding() {
    assert_eq!(SGFX_MAX_DAMAGE_RECTS, 16);
    let damage = [SgfxDamageRect {
        x: 0,
        y: 0,
        width: 1,
        height: 1,
    }; 17];
    let maximum = payload_commit_sgfx_frame(1, 2, 3, 4, 5, &damage[..16]).unwrap();
    assert!(parse_client_message(34, &maximum).is_ok());
    for invalid in [&damage[..0], &damage[..17]] {
        assert_eq!(
            payload_commit_sgfx_frame(1, 2, 3, 4, 5, invalid),
            Err(ProtocolError::MalformedPayload)
        );
    }
    assert_eq!(
        payload_commit_sgfx_frame(1, 2, 3, 4, 0, &damage[..1]),
        Err(ProtocolError::MalformedPayload)
    );

    let mut excessive = maximum;
    excessive[24..28].copy_from_slice(&17_u32.to_le_bytes());
    excessive.extend_from_slice(&COMMIT[28..]);
    assert_eq!(
        parse_client_message(34, &excessive),
        Err(ProtocolError::MalformedPayload)
    );
    for bytes in [&[][..], &COMMIT[28..43], &excessive[28..]] {
        assert_eq!(
            parse_sgfx_damage_rects(bytes),
            Err(ProtocolError::MalformedPayload)
        );
    }
}

#[test]
fn release_identifies_one_exact_buffer_use_not_just_a_slot() {
    assert_eq!(server_msg::SGFX_BUFFER_RELEASED, 28);
    let bytes = &COMMIT[..24];
    assert_eq!(
        payload_sgfx_buffer_released(
            0x0403_0201,
            0x0807_0605,
            0x0c0b_0a09,
            0x100f_0e0d,
            0x1817_1615_1413_1211,
        ),
        bytes,
    );
    assert_eq!(
        parse_server_message(28, bytes),
        Ok(ServerMessage::SgfxBufferReleased {
            window_id: 0x0403_0201,
            buffer_id: 0x0807_0605,
            generation: 0x0c0b_0a09,
            compositor_epoch: 0x100f_0e0d,
            commit_serial: 0x1817_1615_1413_1211,
        }),
    );
    for len in 0..24 {
        assert_eq!(
            parse_server_message(28, &bytes[..len]),
            Err(ProtocolError::MalformedPayload)
        );
    }
    assert_eq!(
        parse_server_message(28, &COMMIT[..25]),
        Err(ProtocolError::MalformedPayload)
    );
    let mut zero_serial = [0; 24];
    zero_serial[..16].copy_from_slice(&bytes[..16]);
    assert_eq!(
        parse_server_message(28, &zero_serial),
        Err(ProtocolError::MalformedPayload)
    );
}
