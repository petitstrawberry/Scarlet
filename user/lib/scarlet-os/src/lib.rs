//! Safe-ish Scarlet Native OS wrappers.
//!
//! This crate is the M0 landing point for Scarlet-specific userland APIs that
//! should remain available to `no_std` applications and to future Rust `std`
//! applications through an explicit Scarlet crate.
//!
//! Today it is a compatibility facade over the wrappers that still live in
//! `scarlet-std`. Moving implementations here without breaking existing
//! applications is the next migration step.

#![no_std]

/// Handle ownership and capability views.
pub mod handle {
    pub use scarlet_std::handle::*;
}

/// Hypervisor control APIs.
pub mod hypervisor {
    pub use scarlet_std::hypervisor::*;
}

/// Scarlet IPC and shared-memory APIs.
pub mod ipc {
    pub use scarlet_std::ipc::*;
}

/// Poll/select-style readiness APIs for Scarlet handles.
pub mod poll {
    pub use scarlet_std::poll::*;
}

/// Pseudo-terminal APIs.
pub mod pty {
    pub use scarlet_std::pty::*;
}

/// Scarlet Native socket APIs.
pub mod socket {
    pub use scarlet_std::socket::*;
}

/// Task and process-control APIs.
pub mod task {
    pub use scarlet_std::task::*;
}

/// Thread APIs.
pub mod thread {
    pub use scarlet_std::thread::*;
}

/// Terminal control APIs.
pub mod tty {
    pub use scarlet_std::tty::*;
}

pub use handle::{Handle, RawHandle};
pub use ipc::SharedMemory;
pub use socket::Socket;
