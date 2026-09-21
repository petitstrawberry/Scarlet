//! Runtime owned by the resident `scarlet-ld` interpreter.
//!
//! Applications import the C symbols described in `scarlet_dl.h`; they must
//! not statically link another copy of this runtime. Startup dependencies and
//! runtime loads consequently use the same process-lifetime namespace.

#![deny(unsafe_op_in_unsafe_fn)]

#[cfg(any(target_os = "scarlet", test))]
mod paths;
#[cfg(any(target_os = "scarlet", test))]
#[cfg_attr(not(target_os = "scarlet"), allow(dead_code))]
mod platform;
#[cfg(any(target_os = "scarlet", test))]
#[cfg_attr(not(target_os = "scarlet"), allow(dead_code))]
mod process;
#[cfg(target_os = "scarlet")]
mod runtime;

#[cfg(target_os = "scarlet")]
pub use runtime::initialize;
