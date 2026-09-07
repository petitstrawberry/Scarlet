//! Scarlet Native process-control extensions.
//!
//! Portable process creation and per-child waiting belong to
//! [`std::process`](https://doc.rust-lang.org/std/process/) when the `std`
//! feature is available. This module contains the Native operations that do
//! not have a portable Rust standard-library equivalent.

use scarlet_sys::{Syscall, syscall0, syscall1, syscall3};

/// Return immediately when no requested child has changed state.
pub const WAIT_NOHANG: i32 = 0x1;

/// System shutdown operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ShutdownType {
    /// Power off the machine.
    PowerOff = 0,
    /// Reboot the machine.
    Reboot = 1,
}

/// Wait for a child process to change state.
///
/// # Arguments
///
/// * `pid` - Namespace-local child PID, or `-1` to wait for any child.
/// * `options` - A bitmask of `WAIT_*` options such as [`WAIT_NOHANG`].
///
/// # Returns
///
/// A `(pid, status)` pair. With [`WAIT_NOHANG`], PID `0` means no child is
/// ready. PID `-1` reports a syscall failure.
pub fn waitpid(pid: i32, options: i32) -> (i32, i32) {
    let mut status = 0i32;
    // SAFETY: status is an exclusive i32 output valid until return; pid and options are scalar values.
    let result = unsafe {
        syscall3(
            Syscall::Waitpid,
            pid as usize,
            (&mut status as *mut i32) as usize,
            options as usize,
        )
    };
    (result as i32, status)
}

/// Request a platform shutdown.
///
/// # Arguments
///
/// * `shutdown_type` - Whether to power off or reboot.
///
/// # Panics
///
/// Panics if the kernel unexpectedly returns from the shutdown syscall.
pub fn shutdown(shutdown_type: ShutdownType) -> ! {
    // SAFETY: This typed shutdown request has no pointer arguments and never resumes normal execution.
    unsafe { syscall1(Syscall::Shutdown, shutdown_type as usize) };
    panic!("shutdown syscall unexpectedly returned")
}

/// Query the number of tasks currently known to the kernel.
///
/// # Returns
///
/// The current task count, or `None` when the kernel rejects the query.
pub fn task_count() -> Option<usize> {
    // SAFETY: This fixed query has no arguments or userspace memory effects.
    let count = unsafe { syscall0(Syscall::GetTaskInfoCount) };
    (count != usize::MAX).then_some(count)
}
