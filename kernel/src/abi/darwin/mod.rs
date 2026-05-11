//! Darwin (macOS) ABI module
//!
//! Provides kernel-native execution of Darwin aarch64 binaries on Scarlet OS.
//! Darwin uses a dual syscall interface:
//! - BSD syscalls (SVC #0x80): POSIX-compatible file I/O, process, network
//! - Mach traps (SVC #0x81): IPC, port management, VM operations
//!
//! # Architecture
//!
//! BSD syscalls map directly to Scarlet's VFS, TaskManager, and NetworkManager.
//! Mach traps are emulated using Scarlet's Event System, SharedMemory, and HandleTable.
//!
//! # Scope
//!
//! Phase 1: Static C binaries only (no dyld, no ObjC runtime).

#[cfg(target_arch = "aarch64")]
pub mod aarch64;
pub mod error;
pub mod path;
