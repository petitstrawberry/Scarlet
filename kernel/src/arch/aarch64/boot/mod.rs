//! AArch64 boot and startup code
//!
//! Boot sequence and startup routines for AArch64 architecture.
//! This module provides the entry points and initialization code for
//! starting the kernel on AArch64 systems.

mod entry;

// Re-export the entry points for the linker
pub use entry::*;

/// Initialize AArch64-specific boot components
/// 
/// This function is called during kernel initialization to set up
/// architecture-specific boot components and configurations.
pub fn boot_init() {
    // Boot initialization is primarily handled by the entry points
    // Additional arch-specific boot setup can be added here
}