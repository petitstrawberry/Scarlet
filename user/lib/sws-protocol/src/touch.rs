//! Bounded, target-local direct-touch frames. Coordinates are in surface pixels.

use crate::ProtocolError;
use std::vec::Vec;

pub const MAX_CHANGES: usize = 128;
const HEADER_SIZE: usize = 28;
const CHANGE_SIZE: usize = 28;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Phase {
    Down = 0,
    Move = 1,
    Up = 2,
    Cancel = 3,
}

impl Phase {
    fn parse(raw: u8) -> Result<Self, ProtocolError> {
        match raw {
            0 => Ok(Self::Down),
            1 => Ok(Self::Move),
            2 => Ok(Self::Up),
            3 => Ok(Self::Cancel),
            _ => Err(ProtocolError::MalformedPayload),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Change {
    /// Unique within the compositor lifetime, including device reconnects.
    pub id: u64,
    pub phase: Phase,
    pub x: i32,
    pub y: i32,
    pub pressure: Option<i32>,
    pub touch_major: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub seat_id: u32,
    pub serial: u64,
    pub time_ns: u64,
    pub changes: Vec<Change>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub window_id: u32,
    pub seat_id: u32,
    pub serial: u64,
    pub time_ns: u64,
    pub change_count: u16,
}

pub fn subscription_payload(window_id: u32, enabled: bool) -> [u8; 8] {
    let mut payload = [0; 8];
    payload[..4].copy_from_slice(&window_id.to_le_bytes());
    payload[4..].copy_from_slice(&u32::from(enabled).to_le_bytes());
    payload
}

pub fn payload(window_id: u32, frame: &Frame) -> Result<Vec<u8>, ProtocolError> {
    if frame.changes.is_empty() || frame.changes.len() > MAX_CHANGES {
        return Err(ProtocolError::MalformedPayload);
    }
    let mut payload = Vec::with_capacity(HEADER_SIZE + frame.changes.len() * CHANGE_SIZE);
    payload.extend_from_slice(&window_id.to_le_bytes());
    payload.extend_from_slice(&frame.seat_id.to_le_bytes());
    payload.extend_from_slice(&frame.serial.to_le_bytes());
    payload.extend_from_slice(&frame.time_ns.to_le_bytes());
    payload.extend_from_slice(&(frame.changes.len() as u16).to_le_bytes());
    payload.extend_from_slice(&0u16.to_le_bytes());
    for change in &frame.changes {
        if change.id == 0
            || change.pressure.is_some_and(|value| value < 0)
            || change.touch_major.is_some_and(|value| value < 0)
        {
            return Err(ProtocolError::MalformedPayload);
        }
        payload.extend_from_slice(&change.id.to_le_bytes());
        payload.push(change.phase as u8);
        payload.extend_from_slice(&[0; 3]);
        payload.extend_from_slice(&change.x.to_le_bytes());
        payload.extend_from_slice(&change.y.to_le_bytes());
        payload.extend_from_slice(&change.pressure.unwrap_or(-1).to_le_bytes());
        payload.extend_from_slice(&change.touch_major.unwrap_or(-1).to_le_bytes());
    }
    Ok(payload)
}

pub fn parse_header(payload: &[u8]) -> Result<Header, ProtocolError> {
    if payload.len() < HEADER_SIZE {
        return Err(ProtocolError::MalformedPayload);
    }
    let count = u16::from_le_bytes([payload[24], payload[25]]);
    if count == 0
        || usize::from(count) > MAX_CHANGES
        || payload.len() != HEADER_SIZE + usize::from(count) * CHANGE_SIZE
        || payload[26..28] != [0, 0]
    {
        return Err(ProtocolError::MalformedPayload);
    }
    for index in 0..usize::from(count) {
        let base = HEADER_SIZE + index * CHANGE_SIZE;
        if payload[base..base + 8] == [0; 8]
            || Phase::parse(payload[base + 8]).is_err()
            || payload[base + 9..base + 12] != [0; 3]
        {
            return Err(ProtocolError::MalformedPayload);
        }
        for offset in [20, 24] {
            let value = i32::from_le_bytes(
                payload[base + offset..base + offset + 4]
                    .try_into()
                    .unwrap(),
            );
            if value < -1 {
                return Err(ProtocolError::MalformedPayload);
            }
        }
    }
    Ok(Header {
        window_id: u32::from_le_bytes(payload[0..4].try_into().unwrap()),
        seat_id: u32::from_le_bytes(payload[4..8].try_into().unwrap()),
        serial: u64::from_le_bytes(payload[8..16].try_into().unwrap()),
        time_ns: u64::from_le_bytes(payload[16..24].try_into().unwrap()),
        change_count: count,
    })
}

pub fn parse(payload: &[u8]) -> Result<(u32, Frame), ProtocolError> {
    let header = parse_header(payload)?;
    let mut changes = Vec::with_capacity(usize::from(header.change_count));
    for index in 0..usize::from(header.change_count) {
        let base = HEADER_SIZE + index * CHANGE_SIZE;
        let axis = |offset| {
            i32::from_le_bytes(
                payload[base + offset..base + offset + 4]
                    .try_into()
                    .unwrap(),
            )
        };
        let optional = |offset| match axis(offset) {
            -1 => None,
            value => Some(value),
        };
        changes.push(Change {
            id: u64::from_le_bytes(payload[base..base + 8].try_into().unwrap()),
            phase: Phase::parse(payload[base + 8])?,
            x: axis(12),
            y: axis(16),
            pressure: optional(20),
            touch_major: optional(24),
        });
    }
    Ok((
        header.window_id,
        Frame {
            seat_id: header.seat_id,
            serial: header.serial,
            time_ns: header.time_ns,
            changes,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_reject_malformed_frames() {
        let frame = Frame {
            seat_id: 2,
            serial: 9,
            time_ns: 10,
            changes: std::vec![Change {
                id: 55,
                phase: Phase::Down,
                x: -4,
                y: 20,
                pressure: None,
                touch_major: Some(7),
            }],
        };
        let encoded = payload(42, &frame).unwrap();
        assert_eq!(parse(&encoded), Ok((42, frame)));
        assert!(parse(&encoded[..encoded.len() - 1]).is_err());
        let mut invalid = encoded.clone();
        invalid[HEADER_SIZE + 8] = 4;
        assert!(parse(&invalid).is_err());
        let mut invalid = encoded.clone();
        invalid[24] = 129;
        assert!(parse(&invalid).is_err());
        let mut invalid = encoded;
        invalid[HEADER_SIZE + 9] = 1;
        assert!(parse(&invalid).is_err());
    }

    #[test]
    fn subscription_is_exact_and_boolean() {
        let payload = subscription_payload(42, true);
        assert!(matches!(
            crate::parse_client_message(crate::client_msg::SET_TOUCH_INPUT, &payload),
            Ok(crate::ClientMessageRef::SetTouchInput {
                window_id: 42,
                enabled: true,
            })
        ));
        assert!(
            crate::parse_client_message(crate::client_msg::SET_TOUCH_INPUT, &payload[..7]).is_err()
        );
        let mut invalid = payload;
        invalid[4] = 2;
        assert!(crate::parse_client_message(crate::client_msg::SET_TOUCH_INPUT, &invalid).is_err());
    }
}
