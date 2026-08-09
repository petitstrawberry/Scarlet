//! Scarlet Native runtime support.
//!
//! This crate owns the pieces that make Scarlet Native executables start and
//! stop correctly.
//!
//! The default feature set is intentionally empty so Rust's upstream `std`
//! port can reuse argument/environment and exit glue without importing a global
//! allocator, entry symbol, panic handler, or allocation error handler.

#![no_std]
#![cfg_attr(feature = "panic", feature(alloc_error_handler))]
#![deny(unsafe_op_in_unsafe_fn)]

#[cfg(feature = "allocator")]
pub mod allocator;
#[cfg(feature = "entry")]
mod arch;
pub mod env;

#[cfg(feature = "allocator")]
pub use allocator::{brk, sbrk};
#[cfg(feature = "entry")]
pub use arch::{arch_set_tls_pointer, arch_tls_pointer};

#[cfg(feature = "panic")]
use core::fmt::{self, Write};

use scarlet_sys::{Syscall, syscall1};

#[cfg(feature = "panic")]
struct RuntimeConsole;

#[cfg(feature = "panic")]
impl Write for RuntimeConsole {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        for byte in value.bytes() {
            let _ = scarlet_sys::syscall1(Syscall::Putchar, byte as usize);
        }
        Ok(())
    }
}

/// Exit the current process using Scarlet Native `ExitGroup`.
///
/// # Arguments
///
/// * `code` - Process exit status.
pub fn exit(code: i32) -> ! {
    let _ = syscall1(Syscall::ExitGroup, code as usize);

    // ExitGroup must not return. If a mismatched or broken kernel does return,
    // stay quiescent instead of turning a failed process teardown into a
    // permanent 100% CPU task.
    loop {
        let _ = syscall1(Syscall::Sleep, 1_000_000_000);
    }
}

#[cfg(feature = "panic")]
#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    let _ = writeln!(RuntimeConsole, "Panic occurred: {info:?}");
    exit(101)
}

#[cfg(feature = "panic")]
#[alloc_error_handler]
fn alloc_error_handler(layout: core::alloc::Layout) -> ! {
    let _ = writeln!(RuntimeConsole, "Allocation failed: {layout:?}");
    exit(102)
}
