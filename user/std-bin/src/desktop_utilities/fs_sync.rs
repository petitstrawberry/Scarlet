//! Durable synchronization through the supported Scarlet standard library.
//!
//! Requires Scarlet Rust 26d00a8 or newer: older toolchains implemented fsync as
//! a no-op and are suitable only for host tests/type checks, not native use.
use std::io;

pub fn sync_file(file: &std::fs::File) -> io::Result<()> {
    file.sync_all()
}
