//! stemd IPC protocol definitions
//!
//! This module defines the wire protocol for communicating with stemd over
//! its Unix domain socket at /tmp/stemd.sock.
//!
//! # Wire Format
//!
//! All messages start with a 1-byte command type, followed by a command-specific payload.
//!
//! ## Commands
//!
//! ### STATUS (0x00)
//! - Payload: empty
//! - Response: "stemd is running\n"
//!
//! ### LAUNCH_OR_FOCUS (0x01)
//! - Payload:
//!   - app_id_len (u32, little-endian)
//!   - app_id_bytes (variable)
//!   - exec_path_len (u32, little-endian)
//!   - exec_path_bytes (variable)
//! - If exec_path is empty, stemd will look up the app from registered .desktop files
//! - Response: "OK: Launched or focused\n" or "ERROR: <message>\n"
//!
//! ### LAUNCH (0x05)
//! Same payload as LAUNCH_OR_FOCUS, but always starts a new process.

use std::vec::Vec;

/// Command types for stemd IPC
pub mod cmd {
    pub const STATUS: u8 = 0x00;
    pub const LAUNCH_OR_FOCUS: u8 = 0x01;
    pub const REGISTER_APP: u8 = 0x02;
    pub const UNREGISTER_APP: u8 = 0x03;
    pub const SHUTDOWN: u8 = 0x04;
    pub const LAUNCH: u8 = 0x05;
}

/// Build LAUNCH_OR_FOCUS command payload
/// Format: app_id_len(4) + app_id + exec_path_len(4) + exec_path
/// If exec_path is empty, stemd will look up the app from registered applications
pub fn payload_launch_or_focus(app_id: &[u8], exec_path: &[u8]) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend_from_slice(&(app_id.len() as u32).to_le_bytes());
    payload.extend_from_slice(app_id);
    payload.extend_from_slice(&(exec_path.len() as u32).to_le_bytes());
    payload.extend_from_slice(exec_path);
    payload
}
