//! AArch64 kernel-specific code
//!
//! Kernel-specific functionality for AArch64 architecture.

// TODO: Implement AArch64 kernel functionality
// This includes CPU management and kernel utilities

pub fn get_cpu() -> &'static crate::arch::Aarch64 {
    super::get_cpu()
}
