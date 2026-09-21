//! Optional input-panel role. It shares TextInput contexts without becoming an IME.
//! Keys are atomic strokes addressed to an activation generation, never a global injector.
use crate::{ProtocolError, read_u32};
use std::vec::Vec;

pub const SHIFT: u32 = 1;
pub const CTRL: u32 = 2;
pub const ALT: u32 = 4;
pub const MODIFIERS: u32 = SHIFT | CTRL | ALT;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Context {
    pub context_id: u32,
    pub window_id: u32,
    pub generation: u32,
    pub content_hint: u32,
    pub content_purpose: u32,
}
impl Context {
    pub fn active(self) -> bool {
        self.context_id != 0
    }
    pub fn encode(self) -> Vec<u8> {
        words(&[
            self.context_id,
            self.window_id,
            self.generation,
            self.content_hint,
            self.content_purpose,
        ])
    }
    pub fn parse(bytes: &[u8]) -> Result<Self, ProtocolError> {
        exact(bytes, 20)?;
        Ok(Self {
            context_id: read_u32(bytes, 0)?,
            window_id: read_u32(bytes, 4)?,
            generation: read_u32(bytes, 8)?,
            content_hint: read_u32(bytes, 12)?,
            content_purpose: read_u32(bytes, 16)?,
        })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Occlusion {
    pub window_id: u32,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}
impl Occlusion {
    pub fn encode(self) -> Vec<u8> {
        words(&[
            self.window_id,
            self.x as u32,
            self.y as u32,
            self.width,
            self.height,
        ])
    }
    pub fn parse(bytes: &[u8]) -> Result<Self, ProtocolError> {
        exact(bytes, 20)?;
        Ok(Self {
            window_id: read_u32(bytes, 0)?,
            x: read_u32(bytes, 4)? as i32,
            y: read_u32(bytes, 8)? as i32,
            width: read_u32(bytes, 12)?,
            height: read_u32(bytes, 16)?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    /// Editor-owned request; does not grant provider privileges.
    Show {
        context_id: u32,
    },
    Register {
        window_id: u32,
    },
    SetVisible {
        context_id: u32,
        generation: u32,
        visible: bool,
    },
    Key {
        context_id: u32,
        generation: u32,
        code: u16,
        modifiers: u32,
    },
}
impl Request {
    pub fn encode(self) -> (u32, Vec<u8>) {
        use crate::client_msg::*;
        match self {
            Self::Show { context_id } => (TEXT_INPUT_SHOW_PANEL, words(&[context_id])),
            Self::Register { window_id } => (INPUT_PANEL_REGISTER, words(&[window_id])),
            Self::SetVisible {
                context_id,
                generation,
                visible,
            } => (
                INPUT_PANEL_SET_VISIBLE,
                words(&[context_id, generation, u32::from(visible)]),
            ),
            Self::Key {
                context_id,
                generation,
                code,
                modifiers,
            } => (
                INPUT_PANEL_KEY,
                words(&[context_id, generation, code as u32, modifiers]),
            ),
        }
    }
    pub fn parse(kind: u32, bytes: &[u8]) -> Result<Self, ProtocolError> {
        use crate::client_msg::*;
        match kind {
            TEXT_INPUT_SHOW_PANEL => {
                exact(bytes, 4)?;
                Ok(Self::Show {
                    context_id: read_u32(bytes, 0)?,
                })
            }
            INPUT_PANEL_REGISTER => {
                exact(bytes, 4)?;
                Ok(Self::Register {
                    window_id: read_u32(bytes, 0)?,
                })
            }
            INPUT_PANEL_SET_VISIBLE => {
                exact(bytes, 12)?;
                let visible = read_u32(bytes, 8)?;
                if visible > 1 {
                    return Err(ProtocolError::MalformedPayload);
                }
                Ok(Self::SetVisible {
                    context_id: read_u32(bytes, 0)?,
                    generation: read_u32(bytes, 4)?,
                    visible: visible != 0,
                })
            }
            INPUT_PANEL_KEY => {
                exact(bytes, 16)?;
                let code = read_u32(bytes, 8)?;
                let modifiers = read_u32(bytes, 12)?;
                // Linux-compatible keyboard codes; pointer/gamepad buttons are not keys.
                if code == 0 || code >= 0x100 || modifiers & !MODIFIERS != 0 {
                    return Err(ProtocolError::MalformedPayload);
                }
                Ok(Self::Key {
                    context_id: read_u32(bytes, 0)?,
                    generation: read_u32(bytes, 4)?,
                    code: code as u16,
                    modifiers,
                })
            }
            _ => Err(ProtocolError::MalformedPayload),
        }
    }
}
fn exact(bytes: &[u8], len: usize) -> Result<(), ProtocolError> {
    if bytes.len() == len {
        Ok(())
    } else {
        Err(ProtocolError::MalformedPayload)
    }
}
fn words(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_buttons_unknown_modifiers_and_trailing_bytes() {
        for request in [
            Request::Key {
                context_id: 1,
                generation: 2,
                code: 0x130,
                modifiers: 0,
            },
            Request::Key {
                context_id: 1,
                generation: 2,
                code: 30,
                modifiers: 8,
            },
        ] {
            let (kind, bytes) = request.encode();
            assert!(Request::parse(kind, &bytes).is_err());
        }
        let (kind, mut bytes) = Request::SetVisible {
            context_id: 1,
            generation: 2,
            visible: true,
        }
        .encode();
        bytes.push(0);
        assert!(Request::parse(kind, &bytes).is_err());
    }
    #[test]
    fn wire_round_trips() {
        for request in [
            Request::Register { window_id: 7 },
            Request::SetVisible {
                context_id: 2,
                generation: 9,
                visible: true,
            },
            Request::Key {
                context_id: 2,
                generation: 9,
                code: 30,
                modifiers: CTRL | SHIFT,
            },
        ] {
            let (kind, bytes) = request.encode();
            assert_eq!(Request::parse(kind, &bytes).unwrap(), request);
        }
        let context = Context {
            context_id: 2,
            window_id: 7,
            generation: 9,
            content_hint: 3,
            content_purpose: 8,
        };
        assert_eq!(Context::parse(&context.encode()).unwrap(), context);
        let area = Occlusion {
            window_id: 7,
            x: -10,
            y: 440,
            width: 1280,
            height: 280,
        };
        assert_eq!(Occlusion::parse(&area.encode()).unwrap(), area);
    }
}
