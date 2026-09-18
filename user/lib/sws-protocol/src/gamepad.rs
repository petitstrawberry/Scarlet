//! Focus-routed gamepad snapshots. Axes use -32767..32767, triggers 0..32767,
//! and hats -1..1. Button bits follow BTN_GAMEPAD offsets, with five auxiliary
//! BTN_TRIGGER_HAPPY buttons at bits 16..20. RESET discards cached ownership.

use crate::ProtocolError;
pub const RESET: u16 = 1;
pub const BUTTON_MASK: u32 = 0x001f_7fff;
pub const PAYLOAD_SIZE: usize = 36;
pub const fn button_bit(code: u16) -> Option<u32> {
    match code {
        0x130..=0x13e => Some(1 << (code - 0x130)),
        0x2c0..=0x2c4 => Some(1 << (16 + code - 0x2c0)),
        _ => None,
    }
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct State {
    pub device_id: u32,
    pub buttons: u32,
    pub left_x: i16,
    pub left_y: i16,
    pub right_x: i16,
    pub right_y: i16,
    pub left_trigger: u16,
    pub right_trigger: u16,
    pub hat_x: i8,
    pub hat_y: i8,
    pub flags: u16,
    pub time_ns: u64,
}
impl State {
    pub fn payload(self, window_id: u32) -> [u8; PAYLOAD_SIZE] {
        let mut p = [0; PAYLOAD_SIZE];
        p[0..4].copy_from_slice(&window_id.to_le_bytes());
        p[4..8].copy_from_slice(&self.device_id.to_le_bytes());
        p[8..12].copy_from_slice(&self.buttons.to_le_bytes());
        for (n, value) in [self.left_x, self.left_y, self.right_x, self.right_y]
            .iter()
            .enumerate()
        {
            p[12 + n * 2..14 + n * 2].copy_from_slice(&value.to_le_bytes());
        }
        p[20..22].copy_from_slice(&self.left_trigger.to_le_bytes());
        p[22..24].copy_from_slice(&self.right_trigger.to_le_bytes());
        p[24] = self.hat_x as u8;
        p[25] = self.hat_y as u8;
        p[26..28].copy_from_slice(&self.flags.to_le_bytes());
        p[28..36].copy_from_slice(&self.time_ns.to_le_bytes());
        p
    }
    pub fn parse(p: &[u8]) -> Result<(u32, Self), ProtocolError> {
        if p.len() != PAYLOAD_SIZE {
            return Err(ProtocolError::MalformedPayload);
        }
        let axis = |n| i16::from_le_bytes([p[n], p[n + 1]]);
        let state = Self {
            device_id: crate::read_u32(p, 4)?,
            buttons: crate::read_u32(p, 8)?,
            left_x: axis(12),
            left_y: axis(14),
            right_x: axis(16),
            right_y: axis(18),
            left_trigger: u16::from_le_bytes([p[20], p[21]]),
            right_trigger: u16::from_le_bytes([p[22], p[23]]),
            hat_x: p[24] as i8,
            hat_y: p[25] as i8,
            flags: u16::from_le_bytes([p[26], p[27]]),
            time_ns: u64::from_le_bytes(p[28..36].try_into().unwrap()),
        };
        if state.buttons & !BUTTON_MASK != 0
            || state.flags & !RESET != 0
            || [state.left_x, state.left_y, state.right_x, state.right_y].contains(&i16::MIN)
            || state.left_trigger > 32767
            || state.right_trigger > 32767
            || !(-1..=1).contains(&state.hat_x)
            || !(-1..=1).contains(&state.hat_y)
        {
            return Err(ProtocolError::MalformedPayload);
        }
        Ok((crate::read_u32(p, 0)?, state))
    }
}
pub fn input_payload(window_id: u32, enabled: bool, navigation: bool) -> [u8; 12] {
    let mut p = [0; 12];
    p[..4].copy_from_slice(&window_id.to_le_bytes());
    p[4..8].copy_from_slice(&u32::from(enabled).to_le_bytes());
    p[8..12].copy_from_slice(&u32::from(navigation).to_le_bytes());
    p
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn subscription_requires_exact_payload_and_boolean_flags() {
        let payload = input_payload(42, true, false);
        let message =
            crate::parse_client_message(crate::client_msg::SET_GAMEPAD_INPUT, &payload).unwrap();
        assert!(matches!(
            message,
            crate::ClientMessageRef::SetGamepadInput {
                window_id: 42,
                enabled: true,
                navigation: false
            }
        ));
        assert!(
            crate::parse_client_message(crate::client_msg::SET_GAMEPAD_INPUT, &[0; 11]).is_err()
        );
        for offset in [4, 8] {
            let mut payload = input_payload(42, true, true);
            payload[offset] = 2;
            assert!(
                crate::parse_client_message(crate::client_msg::SET_GAMEPAD_INPUT, &payload)
                    .is_err()
            );
        }
    }
    #[test]
    fn snapshot_round_trip_and_malformed_input() {
        let s = State {
            device_id: 7,
            buttons: button_bit(0x131).unwrap(),
            left_x: -32767,
            right_y: 32767,
            left_trigger: 32767,
            hat_x: -1,
            time_ns: u64::MAX,
            ..State::default()
        };
        assert_eq!(State::parse(&s.payload(42)), Ok((42, s)));
        assert!(State::parse(&s.payload(42)[..35]).is_err());
        let mut p = s.payload(42);
        p[24] = 2;
        assert!(State::parse(&p).is_err());
        let mut p = s.payload(42);
        p[26] = 2;
        assert!(State::parse(&p).is_err());
        let mut p = s.payload(42);
        p[12..14].copy_from_slice(&i16::MIN.to_le_bytes());
        assert!(State::parse(&p).is_err());
        let mut p = s.payload(42);
        p[11] = 0x80;
        assert!(State::parse(&p).is_err());
    }
}
