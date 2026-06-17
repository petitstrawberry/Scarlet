//! Safe-ish Scarlet Native OS wrappers.
//!
//! This crate owns Scarlet-specific userland APIs that should remain available
//! to `no_std` applications and to future Rust `std` applications through an
//! explicit Scarlet crate.

#![no_std]

#[cfg(feature = "std")]
extern crate std;

pub mod ffi;

/// Handle ownership and capability views.
pub mod handle;

/// Hypervisor control APIs.
pub mod hypervisor;

/// Scarlet IPC and shared-memory APIs.
pub mod ipc;

/// Poll/select-style readiness APIs for Scarlet handles.
pub mod poll;

/// Scarlet Native socket APIs.
pub mod socket;

/// Scarlet time APIs.
pub mod time;

pub use handle::{Handle, RawHandle};
pub use ipc::SharedMemory;
pub use socket::Socket;
