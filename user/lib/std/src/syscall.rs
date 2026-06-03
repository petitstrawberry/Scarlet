//! Scarlet Native syscall facade.
//!
//! The raw ABI definitions and syscall assembly live in `scarlet-abi` and
//! `scarlet-sys`. This module preserves the historical `scarlet_std::syscall`
//! API for existing userland code.

pub use scarlet_abi::Syscall;
pub use scarlet_sys::{syscall0, syscall1, syscall2, syscall3, syscall4, syscall5, syscall6};
