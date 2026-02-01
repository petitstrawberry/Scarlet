//! # Scarlet Standard Library
//!
//! This no_std library provides the core functionality for user-space programs
//! running on the Scarlet.
//!
//! ## Features
//!
//! - Re-exports of core and alloc library components for use in a no_std environment
//! - System call interface for interacting with the Scarlet kernel
//! - Architecture-specific functionality
//! - Custom memory allocator implementation
//!
//! ## Handle Model (RawHandle / Handle / Capabilities)
//!
//! Scarlet user space interacts with the kernel primarily through *handles*.
//!
//! - [`handle::RawHandle`] is the raw integer handle value returned by syscalls.
//!   By itself it does **not** provide ownership or type information.
//! - [`handle::Handle`] is the owning RAII wrapper. Dropping it closes the underlying
//!   kernel object handle.
//! - When a `Handle` is created (e.g. [`handle::Handle::open`] or
//!   [`handle::Handle::from_raw`]), user space queries the kernel for
//!   [`handle::introspection::KernelObjectInfo`] and caches it inside the `Handle`.
//!   This cached info is then used to validate conversions such as
//!   [`handle::Handle::as_socket`] without additional syscalls.
//!
//! ## High-level Wrappers (File / Socket / SharedMemory)
//!
//! Higher-level types in this crate (e.g. [`fs::File`], [`socket::Socket`],
//! [`ipc::SharedMemory`]) own a [`handle::Handle`] internally.
//!
//! - Wrapper constructors from an existing handle are *fallible* and perform a type
//!   check using the cached object info.
//! - Capability views (e.g. `StreamOps`, `FileObject`) borrow `&Handle` and are
//!   obtained via `Handle::as_*` methods.
//!
#![no_std]
#![no_main]
#![feature(alloc_error_handler)]
#![feature(async_iterator)]
#![feature(new_range_api)]

mod core_exports {
    extern crate core;

    pub use core::any;
    pub use core::array;
    pub use core::async_iter;
    pub use core::cell;
    pub use core::char;
    pub use core::clone;
    pub use core::cmp;
    pub use core::convert;
    pub use core::default;
    pub use core::future;
    pub use core::hint;
    pub use core::i8;
    pub use core::i16;
    pub use core::i32;
    pub use core::i64;
    pub use core::i128;
    pub use core::isize;
    pub use core::iter;
    pub use core::marker;
    pub use core::mem;
    pub use core::ops;
    pub use core::option;
    pub use core::panic;
    pub use core::pin;
    pub use core::ptr;
    pub use core::range;
    pub use core::result;
    pub use core::time;
    pub use core::u8;
    pub use core::u16;
    pub use core::u32;
    pub use core::u64;
    pub use core::u128;
    pub use core::usize;
}

mod alloc_exports {
    extern crate alloc;

    pub use alloc::borrow;
    pub use alloc::boxed;
    pub use alloc::fmt;
    pub use alloc::format;
    pub use alloc::rc;
    pub use alloc::slice;
    pub use alloc::str;
    pub use alloc::string;
    pub use alloc::vec;
}

mod allocator;
mod arch;
pub mod collections;
pub mod env;
pub mod ffi;
pub mod fs;
pub mod handle;
pub mod io;
pub mod ipc;
pub mod network;
pub mod socket;
pub mod sync;
pub mod syscall;
pub mod task;
pub mod thread;

// Re-export LocalKey type for convenience
// Note: thread_local! macro is automatically exported at crate root by #[macro_export]
pub use thread::LocalKey;

/// Debug/profiler utilities
pub mod profiler {
    use crate::syscall::{Syscall, syscall0};

    /// Dump profiler statistics from the kernel
    ///
    /// This function calls the kernel's profiler dump system call to output
    /// performance statistics collected during execution. Only available
    /// when the kernel is built with profiler support.
    pub fn dump_profiler_stats() {
        syscall0(Syscall::ProfilerDump);
    }
}

pub use alloc_exports::*;
pub use core_exports::*;

#[panic_handler]
pub fn panic(_info: &core::panic::PanicInfo) -> ! {
    crate::println!("Panic occurred: {:?}", _info);
    loop {}
}

#[alloc_error_handler]
fn alloc_error_handler(_layout: core::alloc::Layout) -> ! {
    loop {}
}
