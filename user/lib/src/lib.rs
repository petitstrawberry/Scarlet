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
    pub use core::pin;
    pub use core::ptr;
    pub use core::range;
    pub use core::result;
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
    pub use alloc::collections;
    pub use alloc::fmt;
    pub use alloc::format;
    pub use alloc::rc;
    pub use alloc::slice;
    pub use alloc::str;
    pub use alloc::string;
    pub use alloc::vec;
}

mod arch;
mod allocator;
pub mod syscall;
pub mod io;
pub mod task;
pub mod fs;
pub mod ffi;
pub mod env;

pub use core_exports::*;
pub use alloc_exports::*;

#[panic_handler]
pub fn panic(_info: &core::panic::PanicInfo) -> ! {
    crate::println!("Panic occurred: {:?}", _info);
    loop {}
}

#[alloc_error_handler]
fn alloc_error_handler(layout: core::alloc::Layout) -> ! {
    loop {}
}
