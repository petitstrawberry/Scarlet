//! WASI (WebAssembly System Interface) ABI Module
//!
//! This module implements the WASI Preview 1 ABI for the Scarlet kernel.
//! It provides a 1-to-1 mapping to WASI-defined system calls, enabling
//! WebAssembly modules to interact with the system.
//!
//! WASI Preview 1 defines a capability-based system interface with functions
//! for file I/O, environment access, and process control.

pub mod preview1;
