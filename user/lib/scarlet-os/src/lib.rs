//! Typed Scarlet Native OS wrappers and explicit unsafe low-level operations.
//!
//! This crate owns Scarlet-specific userland APIs that should remain available
//! to both `no_std` applications and applications using the Scarlet Rust `std`
//! targets, through an explicit Scarlet crate rather than portable `std` APIs.
//! The crate itself is `no_std`; its `std` feature selects integration with an
//! application's standard library. The default `rt-env` feature separately
//! enables the `scarlet-rt` environment dependency.

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]

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

/// Scarlet native sensor metadata and event-stream APIs.
pub mod sensor;

/// Scarlet time APIs.
pub mod time;

pub use handle::{Handle, RawHandle};
pub use input::InputDevice;
pub use ipc::SharedMemory;
pub use socket::Socket;
