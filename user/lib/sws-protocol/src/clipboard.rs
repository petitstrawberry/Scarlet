//! Bounded UTF-8 clipboard transport. Contents belong to the SWS session.
use crate::ProtocolError;
use std::vec::Vec;

pub const MAX_TEXT_BYTES: usize = 256 * 1024;
pub const OK: u32 = 0;
pub const DENIED: u32 = 1;
pub const EMPTY: u32 = 2;
pub const BUSY: u32 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request<'a> {
    Get { window_id: u32 },
    Set { window_id: u32, text: &'a str },
}

impl<'a> Request<'a> {
    pub fn window_id(self) -> u32 {
        match self {
            Self::Get { window_id } | Self::Set { window_id, .. } => window_id,
        }
    }
    pub fn parse(kind: u32, payload: &'a [u8]) -> Result<Self, ProtocolError> {
        let window_id = crate::read_u32(payload, 0)?;
        match kind {
            crate::client_msg::GET_CLIPBOARD_TEXT if payload.len() == 4 => {
                Ok(Self::Get { window_id })
            }
            crate::client_msg::SET_CLIPBOARD_TEXT if payload.len() >= 8 => {
                let len = crate::read_u32(payload, 4)? as usize;
                if len > MAX_TEXT_BYTES || payload.len() != 8 + len {
                    return Err(ProtocolError::MalformedPayload);
                }
                let text = core::str::from_utf8(&payload[8..])
                    .map_err(|_| ProtocolError::MalformedPayload)?;
                Ok(Self::Set { window_id, text })
            }
            _ => Err(ProtocolError::MalformedPayload),
        }
    }
    pub fn encode(self) -> Result<(u32, Vec<u8>), ProtocolError> {
        let mut payload = self.window_id().to_le_bytes().to_vec();
        let kind = match self {
            Self::Get { .. } => crate::client_msg::GET_CLIPBOARD_TEXT,
            Self::Set { text, .. } => {
                if text.len() > MAX_TEXT_BYTES {
                    return Err(ProtocolError::MalformedPayload);
                }
                payload.extend_from_slice(&(text.len() as u32).to_le_bytes());
                payload.extend_from_slice(text.as_bytes());
                crate::client_msg::SET_CLIPBOARD_TEXT
            }
        };
        Ok((kind, payload))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Response {
    pub status: u32,
    pub text_len: u32,
}
impl Response {
    pub fn parse(payload: &[u8]) -> Result<Self, ProtocolError> {
        let status = crate::read_u32(payload, 0)?;
        let text_len = crate::read_u32(payload, 4)?;
        if !matches!(status, OK | DENIED | EMPTY | BUSY)
            || text_len as usize > MAX_TEXT_BYTES
            || payload.len() != 8 + text_len as usize
            || (status != OK && text_len != 0)
            || core::str::from_utf8(&payload[8..]).is_err()
        {
            return Err(ProtocolError::MalformedPayload);
        }
        Ok(Self { status, text_len })
    }
    pub fn encode(status: u32, text: &str) -> Result<Vec<u8>, ProtocolError> {
        if text.len() > MAX_TEXT_BYTES {
            return Err(ProtocolError::MalformedPayload);
        }
        let mut payload = status.to_le_bytes().to_vec();
        payload.extend_from_slice(&(text.len() as u32).to_le_bytes());
        payload.extend_from_slice(text.as_bytes());
        Self::parse(&payload)?;
        Ok(payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unicode_empty_and_exact_bound_round_trip() {
        for text in [
            "".into(),
            "日本 e\u{301} 👩‍💻\n\0".into(),
            "x".repeat(MAX_TEXT_BYTES),
        ] {
            let request = Request::Set {
                window_id: 7,
                text: &text,
            };
            let (kind, payload) = request.encode().unwrap();
            assert_eq!(
                crate::parse_client_message(kind, &payload).unwrap(),
                crate::ClientMessageRef::Clipboard(request)
            );
            let response = Response::encode(OK, &text).unwrap();
            assert_eq!(
                Response::parse(&response).unwrap().text_len as usize,
                text.len()
            );
            assert_eq!(&response[8..], text.as_bytes());
        }
        assert_eq!(
            Response::parse(&Response::encode(EMPTY, "").unwrap())
                .unwrap()
                .status,
            EMPTY
        );
    }
    #[test]
    fn malformed_or_oversized_text_is_rejected_without_truncation() {
        assert!(
            Request::Set {
                window_id: 1,
                text: &"x".repeat(MAX_TEXT_BYTES + 1)
            }
            .encode()
            .is_err()
        );
        let (kind, mut bytes) = Request::Set {
            window_id: 1,
            text: "a",
        }
        .encode()
        .unwrap();
        bytes[8] = 0xff;
        assert!(Request::parse(kind, &bytes).is_err());
        for length in 0..8 {
            assert!(Response::parse(&bytes[..length]).is_err());
        }
        assert!(Response::encode(DENIED, "secret").is_err());
        let mut response = Response::encode(OK, "text").unwrap();
        response.push(0);
        assert!(Response::parse(&response).is_err());
    }
}
