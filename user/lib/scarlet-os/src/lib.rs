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

/// Scarlet native input event device APIs.
pub mod input;

/// Scarlet IPC and shared-memory APIs.
pub mod ipc;

/// Scarlet Native network configuration APIs.
pub mod network;

/// Poll/select-style readiness APIs for Scarlet handles.
pub mod poll;

/// Scarlet Native process-control APIs not exposed by portable Rust `std`.
pub mod process;

/// Scarlet Native socket APIs.
pub mod socket;

/// Safe current-task scheduler control APIs.
pub mod scheduler;

/// Scarlet time APIs.
pub mod time;

pub use handle::{Handle, RawHandle};
pub use input::InputDevice;
pub use ipc::SharedMemory;
pub use socket::Socket;
