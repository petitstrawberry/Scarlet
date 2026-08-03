//! sbus data types

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::convert::From;

/// Method argument
pub enum Argument {
    String(String),
    Int(i32),
    UInt(u32),
    Boolean(bool),
}

impl Argument {
    /// Serialize argument to bytes
    pub fn serialize(&self, payload: &mut Vec<u8>) -> Result<(), &'static str> {
        match self {
            Argument::String(s) => {
                payload.push(0); // Type: String
                let bytes = s.as_bytes();
                payload.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
                payload.extend_from_slice(bytes);
            }
            Argument::Int(i) => {
                payload.push(1); // Type: Int
                payload.extend_from_slice(&i.to_le_bytes());
            }
            Argument::UInt(u) => {
                payload.push(2); // Type: UInt
                payload.extend_from_slice(&u.to_le_bytes());
            }
            Argument::Boolean(b) => {
                payload.push(3); // Type: Boolean
                payload.push(if *b { 1 } else { 0 });
            }
        }
        Ok(())
    }
}

impl From<&str> for Argument {
    fn from(s: &str) -> Self {
        Argument::String(s.to_string())
    }
}

impl From<String> for Argument {
    fn from(s: String) -> Self {
        Argument::String(s)
    }
}

impl From<i32> for Argument {
    fn from(i: i32) -> Self {
        Argument::Int(i)
    }
}

impl From<u32> for Argument {
    fn from(u: u32) -> Self {
        Argument::UInt(u)
    }
}

impl From<bool> for Argument {
    fn from(b: bool) -> Self {
        Argument::Boolean(b)
    }
}

/// Method result
pub enum MethodResult {
    Unit,
    Int(i32),
    UInt(u32),
    Boolean(bool),
    String(String),
}

impl MethodResult {
    pub fn into_arguments(self) -> Vec<Argument> {
        match self {
            MethodResult::Unit => {
                let v = Vec::new();
                v
            }
            MethodResult::Int(i) => {
                let mut v = Vec::new();
                v.push(Argument::Int(i));
                v
            }
            MethodResult::UInt(u) => {
                let mut v = Vec::new();
                v.push(Argument::UInt(u));
                v
            }
            MethodResult::Boolean(b) => {
                let mut v = Vec::new();
                v.push(Argument::Boolean(b));
                v
            }
            MethodResult::String(s) => {
                let mut v = Vec::new();
                v.push(Argument::String(s));
                v
            }
        }
    }
}

impl From<()> for MethodResult {
    fn from(_: ()) -> Self {
        MethodResult::Unit
    }
}

impl From<i32> for MethodResult {
    fn from(i: i32) -> Self {
        MethodResult::Int(i)
    }
}

impl From<u32> for MethodResult {
    fn from(u: u32) -> Self {
        MethodResult::UInt(u)
    }
}

impl From<bool> for MethodResult {
    fn from(b: bool) -> Self {
        MethodResult::Boolean(b)
    }
}

impl From<String> for MethodResult {
    fn from(s: String) -> Self {
        MethodResult::String(s)
    }
}

/// Service information
#[derive(Debug, Clone)]
pub struct ServiceInfo {
    pub bus_name: String,
    pub pid: i32,
    pub client_id: usize,
}
