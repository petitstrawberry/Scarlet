//! Native execution and memory primitives for the user-mode kernel.

#[cfg(not(all(
    target_arch = "aarch64",
    any(target_os = "linux", target_os = "macos")
)))]
compile_error!("The hosted runtime supports AArch64 Linux and macOS");

pub mod context;
pub mod cpu_binding;
pub mod exclusive;
mod fetched_instruction;
mod platform_register;
mod platform_vm;
pub use platform_vm::synchronize_instructions;
pub mod ram;
pub mod run_gate;
pub mod smp;
pub mod worker;
mod worker_channel;
pub mod worker_context;
pub mod worker_memory;
mod worker_thread;
