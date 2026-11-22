//! Native execution and memory primitives for the user-mode kernel.

#[cfg(not(all(
    target_arch = "aarch64",
    any(target_os = "linux", target_os = "macos", target_os = "ios")
)))]
compile_error!("The hosted runtime supports AArch64 Linux, macOS, and iOS");

pub mod context;
pub mod cpu_binding;
#[cfg(not(target_os = "ios"))]
pub mod exclusive;
#[cfg(not(target_os = "ios"))]
mod fetched_instruction;
#[cfg(target_os = "ios")]
pub mod in_process;
#[cfg(not(target_os = "ios"))]
mod platform_register;
mod platform_vm;
pub use platform_vm::synchronize_instructions;
pub mod ram;
pub mod run_gate;
#[cfg(target_os = "ios")]
pub mod signed_image;
pub mod smp;
#[cfg(not(target_os = "ios"))]
pub mod worker;
#[cfg(not(target_os = "ios"))]
mod worker_channel;
pub mod worker_context;
#[cfg(not(target_os = "ios"))]
pub mod worker_memory;
#[cfg(not(target_os = "ios"))]
mod worker_thread;
