//! Shared code for `userprogram` binaries.
//!
//! This crate exists to share `no_std`-friendly code between multiple user
//! programs (binaries) in this package.

#![no_std]

extern crate scarlet_std as std;

/// Scarlet Window Server (SWS) IPC protocol (shared by server and clients).
///
/// The implementation lives in `src/sws/protocol.rs` and is re-exported here so
/// that both the `sws` server binary and the `sws_client` binary can reference
/// the same source of truth.
#[path = "sws/protocol.rs"]
pub mod sws_protocol;
