//! sbus message types and serialization

use crate::{Argument, msg};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// sbus message header (16 bytes)
#[repr(C)]
pub struct MessageHeader {
    pub msg_type: u32,
    pub serial: u32,
    pub payload_length: u32,
    pub flags: u32,
}

impl Clone for MessageHeader {
    fn clone(&self) -> Self {
        *self
    }
}

impl Copy for MessageHeader {}

impl MessageHeader {
    pub const SIZE: usize = 16;

    pub fn to_le_bytes(&self) -> [u8; 16] {
        let mut bytes = [0u8; 16];
        bytes[0..4].copy_from_slice(&self.msg_type.to_le_bytes());
        bytes[4..8].copy_from_slice(&self.serial.to_le_bytes());
        bytes[8..12].copy_from_slice(&self.payload_length.to_le_bytes());
        bytes[12..16].copy_from_slice(&self.flags.to_le_bytes());
        bytes
    }

    pub fn from_le_bytes(bytes: [u8; 16]) -> Self {
        Self {
            msg_type: u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
            serial: u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
            payload_length: u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
            flags: u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]),
        }
    }
}

/// sbus message
pub enum Message {
    Hello {
        client_name: String,
    },
    RegisterService {
        bus_name: String,
    },
    UnregisterService {
        bus_name: String,
    },
    CallMethod {
        destination: String,
        path: String,
        interface: String,
        method: String,
        args: Vec<Argument>,
    },
    MethodReturn {
        serial: u32,
        result: Vec<Argument>,
    },
    MethodError {
        serial: u32,
        error_name: String,
        message: String,
    },
    Signal {
        sender: String,
        path: String,
        interface: String,
        signal: String,
        args: Vec<Argument>,
    },
}

impl Message {
    /// Get the message type
    pub fn msg_type(&self) -> u32 {
        match self {
            Message::Hello { .. } => msg::HELLO,
            Message::RegisterService { .. } => msg::REGISTER_SERVICE,
            Message::UnregisterService { .. } => msg::UNREGISTER_SERVICE,
            Message::CallMethod { .. } => msg::CALL_METHOD,
            Message::MethodReturn { .. } => msg::METHOD_RETURN,
            Message::MethodError { .. } => msg::METHOD_ERROR,
            Message::Signal { .. } => msg::SIGNAL,
        }
    }

    /// Serialize message to bytes
    pub fn to_bytes(&self) -> Result<Vec<u8>, &'static str> {
        let mut payload = Vec::new();
        self.serialize_payload(&mut payload)?;

        let header = MessageHeader {
            msg_type: self.msg_type(),
            serial: 0, // Will be set by Connection
            payload_length: payload.len() as u32,
            flags: 0,
        };

        let mut bytes = Vec::with_capacity(16 + payload.len());
        bytes.extend_from_slice(&header.to_le_bytes());
        bytes.extend(payload);

        Ok(bytes)
    }

    /// Serialize the payload
    fn serialize_payload(&self, payload: &mut Vec<u8>) -> Result<(), &'static str> {
        match self {
            Message::Hello { client_name } => {
                serialize_string(client_name, payload);
            }
            Message::RegisterService { bus_name } => {
                serialize_string(bus_name, payload);
            }
            Message::UnregisterService { bus_name } => {
                serialize_string(bus_name, payload);
            }
            Message::CallMethod {
                destination,
                path,
                interface,
                method,
                args,
            } => {
                serialize_string(destination, payload);
                serialize_string(path, payload);
                serialize_string(interface, payload);
                serialize_string(method, payload);
                serialize_arguments(args, payload)?;
            }
            Message::MethodReturn { serial, result } => {
                payload.extend_from_slice(&serial.to_le_bytes());
                serialize_arguments(result, payload)?;
            }
            Message::MethodError {
                serial,
                error_name,
                message,
            } => {
                payload.extend_from_slice(&serial.to_le_bytes());
                serialize_string(error_name, payload);
                serialize_string(message, payload);
            }
            Message::Signal {
                sender,
                path,
                interface,
                signal,
                args,
            } => {
                serialize_string(sender, payload);
                serialize_string(path, payload);
                serialize_string(interface, payload);
                serialize_string(signal, payload);
                serialize_arguments(args, payload)?;
            }
        }
        Ok(())
    }
}

/// Serialize a string (length + bytes)
fn serialize_string(s: &str, payload: &mut Vec<u8>) {
    let bytes = s.as_bytes();
    payload.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    payload.extend_from_slice(bytes);
}

/// Serialize arguments
fn serialize_arguments(args: &[Argument], payload: &mut Vec<u8>) -> Result<(), &'static str> {
    payload.extend_from_slice(&(args.len() as u32).to_le_bytes());
    for arg in args {
        arg.serialize(payload)?;
    }
    Ok(())
}

/// Deserialize a message from bytes
pub fn from_bytes(bytes: Vec<u8>) -> Result<Message, &'static str> {
    if bytes.len() < 16 {
        return Err("Message too short");
    }

    let mut header_bytes = [0u8; 16];
    header_bytes.copy_from_slice(&bytes[0..16]);
    let header = MessageHeader::from_le_bytes(header_bytes);

    let payload = &bytes[16..];

    match header.msg_type {
        msg::HELLO => {
            let (client_name, _) = deserialize_string(payload)?;
            Ok(Message::Hello { client_name })
        }
        msg::REGISTER_SERVICE => {
            let (bus_name, _) = deserialize_string(payload)?;
            Ok(Message::RegisterService { bus_name })
        }
        msg::CALL_METHOD => {
            let mut offset = 0;
            let (destination, o) = deserialize_string_at(payload, offset)?;
            offset = o;
            let (path, o) = deserialize_string_at(payload, offset)?;
            offset = o;
            let (interface, o) = deserialize_string_at(payload, offset)?;
            offset = o;
            let (method, o) = deserialize_string_at(payload, offset)?;
            offset = o;
            let (args, _) = deserialize_arguments_at(payload, offset)?;

            Ok(Message::CallMethod {
                destination,
                path,
                interface,
                method,
                args,
            })
        }
        msg::METHOD_RETURN => {
            let mut offset = 0;
            let serial = deserialize_u32_at(payload, offset)?;
            offset += 4;
            let (result, _) = deserialize_arguments_at(payload, offset)?;

            Ok(Message::MethodReturn { serial, result })
        }
        msg::METHOD_ERROR => {
            let mut offset = 0;
            let serial = deserialize_u32_at(payload, offset)?;
            offset += 4;
            let (error_name, o) = deserialize_string_at(payload, offset)?;
            offset = o;
            let (message, _) = deserialize_string_at(payload, offset)?;

            Ok(Message::MethodError {
                serial,
                error_name,
                message,
            })
        }
        msg::SIGNAL => {
            let mut offset = 0;
            let (sender, o) = deserialize_string_at(payload, offset)?;
            offset = o;
            let (path, o) = deserialize_string_at(payload, offset)?;
            offset = o;
            let (interface, o) = deserialize_string_at(payload, offset)?;
            offset = o;
            let (signal, o) = deserialize_string_at(payload, offset)?;
            offset = o;
            let (args, _) = deserialize_arguments_at(payload, offset)?;

            Ok(Message::Signal {
                sender,
                path,
                interface,
                signal,
                args,
            })
        }
        _ => Err("Unknown message type"),
    }
}

/// Deserialize a string from bytes
fn deserialize_string(bytes: &[u8]) -> Result<(String, usize), &'static str> {
    deserialize_string_at(bytes, 0)
}

/// Deserialize a string at offset
fn deserialize_string_at(bytes: &[u8], offset: usize) -> Result<(String, usize), &'static str> {
    if bytes.len() < offset + 4 {
        return Err("String length missing");
    }

    let len = u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ]) as usize;

    if bytes.len() < offset + 4 + len {
        return Err("String data missing");
    }

    let string_bytes = &bytes[offset + 4..offset + 4 + len];
    let s = core::str::from_utf8(string_bytes)
        .map_err(|_| "Invalid UTF-8")?
        .to_string();

    Ok((s, offset + 4 + len))
}

/// Deserialize a u32 at offset
fn deserialize_u32_at(bytes: &[u8], offset: usize) -> Result<u32, &'static str> {
    if bytes.len() < offset + 4 {
        return Err("U32 data missing");
    }

    let value = u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ]);

    Ok(value)
}

/// Deserialize arguments at offset
fn deserialize_arguments_at(
    bytes: &[u8],
    offset: usize,
) -> Result<(Vec<Argument>, usize), &'static str> {
    if bytes.len() < offset + 4 {
        return Err("Arguments count missing");
    }

    let count = u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ]) as usize;

    let mut args = Vec::new();
    let mut current_offset = offset + 4;

    for _ in 0..count {
        if bytes.len() < current_offset + 1 {
            return Err("Argument type missing");
        }

        let arg_type = bytes[current_offset];
        current_offset += 1;

        let arg = match arg_type {
            0 => {
                // String
                let (s, o) = deserialize_string_at(bytes, current_offset)?;
                current_offset = o;
                Argument::String(s)
            }
            1 => {
                // Int
                if bytes.len() < current_offset + 4 {
                    return Err("Int argument missing");
                }
                let val = i32::from_le_bytes([
                    bytes[current_offset],
                    bytes[current_offset + 1],
                    bytes[current_offset + 2],
                    bytes[current_offset + 3],
                ]);
                current_offset += 4;
                Argument::Int(val)
            }
            2 => {
                // UInt
                if bytes.len() < current_offset + 4 {
                    return Err("UInt argument missing");
                }
                let val = u32::from_le_bytes([
                    bytes[current_offset],
                    bytes[current_offset + 1],
                    bytes[current_offset + 2],
                    bytes[current_offset + 3],
                ]);
                current_offset += 4;
                Argument::UInt(val)
            }
            3 => {
                // Boolean
                if bytes.len() < current_offset + 1 {
                    return Err("Boolean argument missing");
                }
                let val = bytes[current_offset] != 0;
                current_offset += 1;
                Argument::Boolean(val)
            }
            _ => return Err("Unknown argument type"),
        };

        args.push(arg);
    }

    Ok((args, current_offset))
}
