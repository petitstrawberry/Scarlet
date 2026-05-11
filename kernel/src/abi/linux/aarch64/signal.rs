//! AArch64 arch-specific signal handling
//!
//! Provides `setup_signal_handler` and `sys_rt_sigreturn` stubs for AArch64.
//! Signal types and generic logic live in `generic::signal`.

use crate::abi::linux::generic::signal::LinuxSignal;
use crate::abi::linux::aarch64::LinuxAarch64Abi;
use crate::arch::Trapframe;

/// Set up user-space signal handler execution with context save/restore.
///
/// TODO: Implement proper AArch64 signal frame layout.
pub fn setup_signal_handler(trapframe: &mut Trapframe, handler_addr: usize, signal: LinuxSignal) {
    trapframe.set_pc(handler_addr as u64);
    // TODO: Save context to signal frame on user stack
    // TODO: Set up signal return trampoline (SVC #0 or similar)
    let _ = signal;
}

/// Linux sys_rt_sigreturn — Restore context saved by `setup_signal_handler`.
///
/// TODO: Implement proper AArch64 signal frame restoration.
pub fn sys_rt_sigreturn(_abi: &mut LinuxAarch64Abi, _trapframe: &mut Trapframe) -> usize {
    // TODO: Restore registers and PC from signal frame
    0
}

/// Arch-specific fallback syscall table for AArch64.
pub fn dispatch_arch_syscall(
    abi: &mut LinuxAarch64Abi,
    trapframe: &mut Trapframe,
    syscall_number: usize,
) -> Option<usize> {
    match syscall_number {
        139 => Some(sys_rt_sigreturn(abi, trapframe)),
        _ => None,
    }
}
