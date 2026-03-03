//! Linux ABI per-device translation layer.
//!
//! This module contains translation helpers for Linux-specific ioctls and other
//! per-device quirks. The kernel core remains ABI-neutral; any Linux/POSIX
//! specifics are mapped here onto Scarlet-native device controls.

#[cfg(feature = "hypervisor")]
pub mod kvm;
pub mod tty;
