//! Scarlet Bus (sbus) - A simple inter-process communication bus
//!
//! sbus is a simplified D-Bus-like IPC mechanism for Scarlet OS.
//! It provides message routing, service discovery, and method invocation.

#![no_std]

extern crate alloc;

mod message;
mod types;

pub use message::*;
pub use types::*;

/// Message types
pub mod msg {
    pub const HELLO: u32 = 0x01;
    pub const REGISTER_SERVICE: u32 = 0x02;
    pub const UNREGISTER_SERVICE: u32 = 0x03;
    pub const CALL_METHOD: u32 = 0x10;
    pub const METHOD_RETURN: u32 = 0x11;
    pub const METHOD_ERROR: u32 = 0x12;
    pub const SIGNAL: u32 = 0x20;
}

/// Protocol version
pub const PROTOCOL_VERSION: u32 = 1;

/// Default sbus socket path
pub const DEFAULT_SOCKET_PATH: &str = "/tmp/sbus.sock";
