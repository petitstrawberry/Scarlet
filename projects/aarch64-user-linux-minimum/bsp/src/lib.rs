//! Device adapters for the AArch64 Linux hosted BSP.

#[cfg(not(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu")))]
compile_error!("This BSP requires AArch64 Linux with a GNU userspace");

#[path = "../../../shared/hosted/block.rs"]
pub mod block;
#[path = "../../../shared/hosted/root_image.rs"]
pub mod root_image;
#[path = "../../../shared/hosted/terminal.rs"]
pub mod terminal;
